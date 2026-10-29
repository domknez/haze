//! Subtitles: text cues decoded from the stream's subtitle track or read from an
//! `.srt` file, and the ones on screen at a moment.

use ffmpeg_next as ffmpeg;

#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    /// Stream time, in seconds.
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// The cues of the track shown, in time order.
#[derive(Default)]
pub struct Cues {
    cues: Vec<Cue>,
}

/// Cues kept from a track as it plays (about an hour of dialogue); a file's are all
/// kept.
const KEPT: usize = 1500;
/// How long a cue without an end stays up.
const UNENDING: f64 = 5.0;

impl Cues {
    pub fn add(&mut self, cue: Cue) {
        let at = self.cues.partition_point(|c| c.start <= cue.start);
        if self.cues.get(at.wrapping_sub(1)).is_some_and(|c| *c == cue) {
            return;
        }
        self.cues.insert(at, cue);
        if self.cues.len() > KEPT {
            self.cues.remove(0);
        }
    }

    pub fn replace(&mut self, mut cues: Vec<Cue>) {
        cues.sort_by(|a, b| a.start.total_cmp(&b.start));
        self.cues = cues;
    }

    pub fn clear(&mut self) {
        self.cues.clear();
    }

    /// What's on screen at `t`: every cue showing then, one per line.
    pub fn at(&self, t: f64) -> Option<String> {
        let upto = self.cues.partition_point(|c| c.start <= t);
        let on: Vec<&str> = self.cues[..upto].iter().rev().take(8).filter(|c| t < c.end).map(|c| c.text.as_str()).collect();
        (!on.is_empty()).then(|| on.into_iter().rev().collect::<Vec<_>>().join("\n"))
    }
}

/// Subtitle codecs that carry text (picture subtitles, DVB and Blu-ray, aren't shown).
pub fn is_text(id: ffmpeg::codec::Id) -> bool {
    use ffmpeg::codec::Id;
    matches!(id, Id::SUBRIP | Id::SRT | Id::ASS | Id::SSA | Id::MOV_TEXT | Id::WEBVTT | Id::TEXT | Id::SUBVIEWER | Id::MICRODVD)
}

/// The cues in a decoded subtitle that arrived in a packet at `pts` seconds.
pub fn cues(sub: &ffmpeg::Subtitle, pts: f64, duration: Option<f64>) -> Vec<Cue> {
    let start = pts + f64::from(sub.start()) / 1000.0;
    let end = match sub.end() {
        0 | u32::MAX => pts + duration.filter(|d| *d > 0.0).unwrap_or(UNENDING),
        e => pts + f64::from(e) / 1000.0,
    };
    sub.rects()
        .filter_map(|r| match r {
            ffmpeg::subtitle::Rect::Text(t) => Some(t.get().to_string()),
            ffmpeg::subtitle::Rect::Ass(a) => Some(ass_text(a.get())),
            _ => None,
        })
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|text| Cue { start, end, text })
        .collect()
}

/// The words of an ASS event as ffmpeg gives it
/// (`ReadOrder,Layer,Style,Name,MarginL,MarginR,MarginV,Effect,Text`), without its
/// `{\…}` styling.
fn ass_text(event: &str) -> String {
    let event = event.strip_prefix("Dialogue:").unwrap_or(event);
    let text = event.splitn(9, ',').nth(8).unwrap_or(event);
    let mut out = String::with_capacity(text.len());
    let mut styling = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => styling = true,
            '}' if styling => styling = false,
            _ if styling => {}
            '\\' => match chars.peek() {
                Some('N' | 'n') => {
                    out.push('\n');
                    chars.next();
                }
                Some('h') => {
                    out.push(' ');
                    chars.next();
                }
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    out
}

/// A subtitle file's text: UTF-8, or else Windows-1250, in which Central European
/// subtitles (č ć đ š ž) are often saved; its upper half covers Western letters too.
pub fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| if b < 0x80 { b as char } else { CP1250[usize::from(b - 0x80)] }).collect(),
    }
}

#[rustfmt::skip]
const CP1250: [char; 128] = [
    '€', '?', '‚', '?', '„', '…', '†', '‡', '?', '‰', 'Š', '‹', 'Ś', 'Ť', 'Ž', 'Ź',
    '?', '‘', '’', '“', '”', '•', '–', '—', '?', '™', 'š', '›', 'ś', 'ť', 'ž', 'ź',
    '\u{a0}', 'ˇ', '˘', 'Ł', '¤', 'Ą', '¦', '§', '¨', '©', 'Ş', '«', '¬', '\u{ad}', '®', 'Ż',
    '°', '±', '˛', 'ł', '´', 'µ', '¶', '·', '¸', 'ą', 'ş', '»', 'Ľ', '˝', 'ľ', 'ż',
    'Ŕ', 'Á', 'Â', 'Ă', 'Ä', 'Ĺ', 'Ć', 'Ç', 'Č', 'É', 'Ę', 'Ë', 'Ě', 'Í', 'Î', 'Ď',
    'Đ', 'Ń', 'Ň', 'Ó', 'Ô', 'Ő', 'Ö', '×', 'Ř', 'Ů', 'Ú', 'Ű', 'Ü', 'Ý', 'Ţ', 'ß',
    'ŕ', 'á', 'â', 'ă', 'ä', 'ĺ', 'ć', 'ç', 'č', 'é', 'ę', 'ë', 'ě', 'í', 'î', 'ď',
    'đ', 'ń', 'ň', 'ó', 'ô', 'ő', 'ö', '÷', 'ř', 'ů', 'ú', 'ű', 'ü', 'ý', 'ţ', '˙',
];

/// The cues of an `.srt` (or WebVTT) file, with their times from the start.
pub fn parse_srt(text: &str) -> Vec<Cue> {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n");
    let mut cues = Vec::new();
    for block in text.split("\n\n") {
        let mut lines = block.lines().skip_while(|l| !l.contains("-->"));
        let Some(timing) = lines.next() else { continue };
        let Some((from, to)) = timing.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (srt_time(from), srt_time(to.split_whitespace().next().unwrap_or(""))) else { continue };
        let words = lines.map(strip_tags).collect::<Vec<_>>().join("\n");
        if !words.trim().is_empty() {
            cues.push(Cue { start, end, text: words.trim().to_string() });
        }
    }
    cues
}

/// `00:01:02,500` (or `01:02.500`) in seconds.
fn srt_time(s: &str) -> Option<f64> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<f64>().ok()?, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        [m, s] => (0.0, m.parse::<f64>().ok()?, s.parse::<f64>().ok()?),
        _ => return None,
    };
    Some(h * 3600.0 + m * 60.0 + sec)
}

/// `<i>`, `<font …>` and the like left out.
fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut tag = false;
    for c in line.chars() {
        match c {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cue(start: f64, end: f64, text: &str) -> Cue {
        Cue { start, end, text: text.into() }
    }

    #[test]
    fn shows_the_cues_on_at_a_moment() {
        let mut c = Cues::default();
        c.add(cue(10.0, 12.0, "second"));
        c.add(cue(5.0, 8.0, "first"));
        c.add(cue(11.0, 13.0, "overlap"));
        c.add(cue(5.0, 8.0, "first"));
        assert_eq!(c.at(4.0), None);
        assert_eq!(c.at(6.0).as_deref(), Some("first"));
        assert_eq!(c.at(9.0), None);
        assert_eq!(c.at(11.5).as_deref(), Some("second\noverlap"));
    }

    #[test]
    fn reads_ass_events() {
        assert_eq!(ass_text(r"0,0,Default,,0,0,0,,{\i1}Hello,{\i0} there\Nfriend"), "Hello, there\nfriend");
        assert_eq!(ass_text(r"Dialogue: 0,0,Default,,0,0,0,,A\hB"), "A B");
    }

    #[test]
    fn reads_central_european_files() {
        assert_eq!(decode_text(b"\xe8a\xe6a \xf0ak \x9aum \x9eir \xc8"), "čaća đak šum žir Č");
        assert_eq!(decode_text("već".as_bytes()), "već");
    }

    #[test]
    fn reads_srt_files() {
        let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\n<i>Hello</i>\r\nthere\r\n\r\n2\r\n00:01:00,000 --> 00:01:03,000 X1:0\r\nAgain\r\n";
        assert_eq!(parse_srt(srt), [cue(1.0, 2.5, "Hello\nthere"), cue(60.0, 63.0, "Again")]);
        let vtt = "WEBVTT\n\n00:05.000 --> 00:06.000\nShort\n";
        assert_eq!(parse_srt(vtt), [cue(5.0, 6.0, "Short")]);
    }
}
