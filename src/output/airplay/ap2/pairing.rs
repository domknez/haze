//! HomeKit transient pairing (SRP-6a, SHA-512, 3072-bit group, PIN 3939) and the
//! ChaCha20-Poly1305 framing used on the encrypted RTSP control channel.
//! Mirrors the client in OwnTone's pair_ap (pair_homekit.c), which interoperates
//! with Apple and third-party AirPlay 2 receivers.

use chacha20poly1305::aead::{AeadInOut, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use num_bigint::BigUint;
use sha2::{Digest, Sha512};
use std::io::{self, Read};
use std::sync::OnceLock;

// ---------------------------------------------------------------- TLV8

pub mod tlv {
    pub const METHOD: u8 = 0;
    pub const SALT: u8 = 2;
    pub const PUBLIC_KEY: u8 = 3;
    pub const PROOF: u8 = 4;
    pub const STATE: u8 = 6;
    pub const ERROR: u8 = 7;
    pub const FLAGS: u8 = 19;

    pub fn encode(items: &[(u8, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(t, v) in items {
            if v.is_empty() {
                out.extend([t, 0]);
            }
            for chunk in v.chunks(255) {
                out.push(t);
                out.push(chunk.len() as u8);
                out.extend_from_slice(chunk);
            }
        }
        out
    }

    /// Consecutive items of the same type are fragments of one value.
    pub fn decode(mut data: &[u8]) -> Vec<(u8, Vec<u8>)> {
        let mut out: Vec<(u8, Vec<u8>)> = Vec::new();
        let mut prev: Option<u8> = None;
        while data.len() >= 2 {
            let (t, len) = (data[0], data[1] as usize);
            let v = &data[2..(2 + len).min(data.len())];
            match out.last_mut() {
                Some((lt, lv)) if prev == Some(t) && *lt == t => lv.extend_from_slice(v),
                _ => out.push((t, v.to_vec())),
            }
            prev = Some(t);
            data = &data[(2 + len).min(data.len())..];
        }
        out
    }

    pub fn get(items: &[(u8, Vec<u8>)], t: u8) -> Option<&[u8]> {
        items.iter().find(|(k, _)| *k == t).map(|(_, v)| v.as_slice())
    }
}

// ---------------------------------------------------------------- SRP-6a

const USERNAME: &str = "Pair-Setup";
pub const TRANSIENT_PIN: &str = "3939";
const G: u32 = 5;

fn n() -> &'static BigUint {
    static N: OnceLock<BigUint> = OnceLock::new();
    N.get_or_init(|| BigUint::parse_bytes(include_str!("srp_n3072.hex").trim().as_bytes(), 16).expect("N"))
}

const N_LEN: usize = 384;

fn h(parts: &[&[u8]]) -> [u8; 64] {
    let mut d = Sha512::new();
    parts.iter().for_each(|p| d.update(p));
    d.finalize().into()
}

fn pad(x: &BigUint) -> Vec<u8> {
    let b = x.to_bytes_be();
    let mut out = vec![0; N_LEN.saturating_sub(b.len())];
    out.extend_from_slice(&b);
    out
}

/// Big-endian bytes without leading zeros (OpenSSL BN_bn2bin), as the reference hashes them.
fn strip(b: &[u8]) -> &[u8] {
    let i = b.iter().position(|&x| x != 0).unwrap_or(b.len());
    &b[i..]
}

pub struct SrpClient {
    a: BigUint,
    a_pub: BigUint,
}

pub struct SrpProof {
    pub a_pub: Vec<u8>,
    pub m1: [u8; 64],
    pub expected_m2: [u8; 64],
    pub session_key: [u8; 64],
}

impl SrpClient {
    pub fn new() -> Self {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret).expect("OS RNG");
        Self::with_secret(&secret)
    }

    fn with_secret(secret: &[u8]) -> Self {
        let a = BigUint::from_bytes_be(secret);
        let a_pub = BigUint::from(G).modpow(&a, n());
        Self { a, a_pub }
    }

    pub fn process(&self, salt: &[u8], b_pub: &[u8], pin: &str) -> Result<SrpProof, String> {
        let n = n();
        let g = BigUint::from(G);
        let b = BigUint::from_bytes_be(b_pub);
        if (&b % n) == BigUint::ZERO {
            return Err("SRP: invalid server key".into());
        }
        let k = BigUint::from_bytes_be(&h(&[&pad(n), &pad(&g)]));
        let u = BigUint::from_bytes_be(&h(&[&pad(&self.a_pub), &pad(&b)]));
        if u == BigUint::ZERO {
            return Err("SRP: invalid scrambler".into());
        }
        let inner = h(&[format!("{USERNAME}:{pin}").as_bytes()]);
        let x = BigUint::from_bytes_be(&h(&[strip(salt), &inner]));
        // S = (B - k*g^x) ^ (a + u*x) mod N, kept non-negative.
        let kgx = (k * g.modpow(&x, n)) % n;
        let base = ((b % n) + n - kgx) % n;
        let s = base.modpow(&(&self.a + u * x), n);
        let session_key = h(&[&s.to_bytes_be()]);

        let hn = h(&[&n.to_bytes_be()]);
        let hg = h(&[&g.to_bytes_be()]);
        let hxor: Vec<u8> = hn.iter().zip(hg).map(|(a, b)| a ^ b).collect();
        let a_bytes = self.a_pub.to_bytes_be();
        let m1 = h(&[&hxor, &h(&[USERNAME.as_bytes()]), strip(salt), &a_bytes, strip(b_pub), &session_key]);
        let expected_m2 = h(&[&a_bytes, &m1, &session_key]);
        Ok(SrpProof { a_pub: a_bytes, m1, expected_m2, session_key })
    }
}

// ---------------------------------------------------------------- channel cipher

pub fn hkdf32(ikm: &[u8], salt: &str, info: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    hkdf::Hkdf::<Sha512>::new(Some(salt.as_bytes()), ikm).expand(info.as_bytes(), &mut out).expect("hkdf length");
    out
}

const BLOCK_MAX: usize = 0x400;

fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    Nonce::from(n)
}

pub struct Encryptor {
    cipher: ChaCha20Poly1305,
    counter: u64,
}

impl Encryptor {
    pub fn new(key: &[u8; 32]) -> Self {
        Self { cipher: ChaCha20Poly1305::new(&Key::from(*key)), counter: 0 }
    }

    /// Frame: [u16 LE length][ciphertext][16-byte tag], length is the AAD.
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len() + plain.len().div_ceil(BLOCK_MAX) * 18);
        for block in plain.chunks(BLOCK_MAX) {
            let len = (block.len() as u16).to_le_bytes();
            let mut buf = block.to_vec();
            let tag = self.cipher.encrypt_inout_detached(&nonce(self.counter), &len, buf.as_mut_slice().into()).expect("encrypt");
            self.counter += 1;
            out.extend_from_slice(&len);
            out.extend_from_slice(&buf);
            out.extend_from_slice(&tag);
        }
        out
    }
}

/// Reader that transparently decrypts framed blocks once a key is set.
pub struct DecryptReader<R> {
    inner: R,
    cipher: Option<(ChaCha20Poly1305, u64)>,
    plain: Vec<u8>,
    pos: usize,
}

impl<R: Read> DecryptReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner, cipher: None, plain: Vec::new(), pos: 0 }
    }

    pub fn enable(&mut self, key: &[u8; 32]) {
        self.cipher = Some((ChaCha20Poly1305::new(&Key::from(*key)), 0));
    }
}

impl<R: Read> Read for DecryptReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let Some((cipher, counter)) = self.cipher.as_mut() else { return self.inner.read(out) };
        if self.pos == self.plain.len() {
            let mut len = [0u8; 2];
            match self.inner.read_exact(&mut len) {
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(0),
                r => r?,
            }
            let n = u16::from_le_bytes(len) as usize;
            let mut buf = vec![0u8; n + 16];
            self.inner.read_exact(&mut buf)?;
            let tag = Tag::try_from(&buf[n..]).expect("16-byte tag");
            buf.truncate(n);
            cipher
                .decrypt_inout_detached(&nonce(*counter), &len, buf.as_mut_slice().into(), &tag)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control channel decrypt failed"))?;
            *counter += 1;
            self.plain = buf;
            self.pos = 0;
        }
        let k = out.len().min(self.plain.len() - self.pos);
        out[..k].copy_from_slice(&self.plain[self.pos..self.pos + k]);
        self.pos += k;
        Ok(k)
    }
}

/// Encrypt one realtime audio RTP packet in place of its payload:
/// header (clear) | ciphertext | tag | last 8 nonce bytes. AAD = timestamp+SSRC.
pub fn seal_audio(cipher: &ChaCha20Poly1305, header: &[u8; 12], payload: &[u8], seq: u16) -> Vec<u8> {
    let mut n = [0u8; 12];
    n[4..6].copy_from_slice(&seq.to_le_bytes());
    let mut out = Vec::with_capacity(12 + payload.len() + 24);
    out.extend_from_slice(header);
    let mut buf = payload.to_vec();
    let tag = cipher.encrypt_inout_detached(&Nonce::from(n), &header[4..12], buf.as_mut_slice().into()).expect("encrypt");
    out.extend_from_slice(&buf);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&n[4..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_is_rfc5054_3072() {
        assert_eq!(n().bits(), 3072);
    }

    #[test]
    fn tlv_fragments() {
        let big = vec![9u8; 384];
        let enc = tlv::encode(&[(tlv::STATE, &[3]), (tlv::PUBLIC_KEY, &big), (tlv::PROOF, &[1; 64])]);
        let dec = tlv::decode(&enc);
        assert_eq!(tlv::get(&dec, tlv::PUBLIC_KEY), Some(&big[..]));
        assert_eq!(tlv::get(&dec, tlv::STATE), Some(&[3u8][..]));
        assert_eq!(tlv::get(&dec, tlv::PROOF).map(<[u8]>::len), Some(64));
    }

    /// Plays the server side of SRP-6a with the same conventions and checks both
    /// sides agree on the session key and proofs.
    #[test]
    fn srp_agrees_with_server_math() {
        let n = n();
        let g = BigUint::from(G);
        let salt = [0x42u8; 16];
        let inner = h(&[format!("{USERNAME}:{TRANSIENT_PIN}").as_bytes()]);
        let x = BigUint::from_bytes_be(&h(&[&salt, &inner]));
        let v = g.modpow(&x, n);
        let k = BigUint::from_bytes_be(&h(&[&pad(n), &pad(&g)]));
        let b_secret = BigUint::from_bytes_be(&[0x17; 32]);
        let b_pub = (k * &v + g.modpow(&b_secret, n)) % n;

        let client = SrpClient::with_secret(&[0x33; 32]);
        let proof = client.process(&salt, &b_pub.to_bytes_be(), TRANSIENT_PIN).unwrap();

        let a_pub = BigUint::from_bytes_be(&proof.a_pub);
        let u = BigUint::from_bytes_be(&h(&[&pad(&a_pub), &pad(&b_pub)]));
        let s_server = (a_pub * v.modpow(&u, n)).modpow(&b_secret, n);
        assert_eq!(h(&[&s_server.to_bytes_be()]), proof.session_key);
    }

    #[test]
    fn channel_roundtrip() {
        let key = hkdf32(&[1; 64], "Control-Salt", "Control-Write-Encryption-Key");
        let mut enc = Encryptor::new(&key);
        let msg: Vec<u8> = (0..3000).map(|i| i as u8).collect();
        let mut wire = enc.seal(b"hello");
        wire.extend(enc.seal(&msg));
        let mut r = DecryptReader::new(&wire[..]);
        r.enable(&key);
        let mut out = Vec::new();
        r.read_to_end(&mut out).unwrap();
        assert_eq!(&out[..5], b"hello");
        assert_eq!(&out[5..], &msg[..]);
    }
}
