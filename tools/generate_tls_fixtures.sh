#!/bin/sh
# Regenerate rustic-net's TLS test certificates (test-only keys; never
# trusted outside the tests). Needs OpenSSL 3.4 or later for
# -not_before/-not_after. Run from the repository root.
set -eu
out=crates/rustic-net/tests/fixtures/tls
mkdir -p "$out"
cd "$out"
key() { openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "$1.key" 2>/dev/null; }
leaf() { # name issuer-prefix san not_before not_after
  key "$1"
  openssl req -new -key "$1.key" -subj "/CN=$1" -out "$1.csr"
  printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=%s\n' "$3" > "$1.ext"
  openssl x509 -req -in "$1.csr" -CA "$2.pem" -CAkey "$2.key" -extfile "$1.ext" \
    -not_before "$4" -not_after "$5" -set_serial "0x$(openssl rand -hex 8)" -out "$1.pem" 2>/dev/null
  rm "$1.csr" "$1.ext"
}
ca() { # name
  key "$1"
  openssl req -x509 -new -key "$1.key" -subj "/CN=Rustic test $1" \
    -addext "basicConstraints=critical,CA:TRUE" -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -not_before 20250101000000Z -not_after 20500101000000Z -out "$1.pem"
}
ca ca
ca untrusted-ca
leaf good ca "DNS:a.test,DNS:localhost,IP:127.0.0.1" 20250101000000Z 20490101000000Z
leaf expired ca "DNS:a.test" 20200101000000Z 20210101000000Z
leaf wrong-host ca "DNS:other.test" 20250101000000Z 20490101000000Z
leaf untrusted untrusted-ca "DNS:a.test" 20250101000000Z 20490101000000Z
rm -f untrusted-ca.key ca.key
for f in *.key; do openssl pkcs8 -topk8 -nocrypt -in "$f" -out "$f.tmp" && mv "$f.tmp" "$f"; done
ls
