#!/bin/sh
# Make the key that signs releases for haze's self-updater, once.
#
# The private key goes to the UPDATE_SIGNING_KEY secret (the Release workflow signs
# SHA256SUMS.txt with it) and to a backup file only you can read: keep that file
# somewhere safe (a password manager). Without it no later release can be signed,
# and copies of haze in use would refuse every update.
#
# Prints the public key, which goes into src/update.rs (UPDATE_KEY).
set -e
repo=${REPO:-domknez/haze}
backup=${BACKUP:-$HOME/.haze-release/update-signing-key.pem}
if [ -e "$backup" ]; then
    echo "$backup already exists; not making another key" >&2
    exit 1
fi
mkdir -p "$(dirname "$backup")"
umask 077
openssl genpkey -algorithm ed25519 -out "$backup"
gh secret set UPDATE_SIGNING_KEY -R "$repo" < "$backup"
echo "private key: $backup (back it up, then you may delete it here)"
printf 'public key: '
openssl pkey -in "$backup" -pubout -outform DER | tail -c 32 | od -An -tx1 | tr -d ' \n'
echo
