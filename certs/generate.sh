#!/usr/bin/env bash
# Generate self-signed TLS cert for Rearm HTTP/3 (QUIC) + HTTPS localhost
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "${ROOT}"

DAYS="${CERT_DAYS:-825}"
CN="${CERT_CN:-localhost}"
OUT_KEY="${OUT_KEY:-server.key}"
OUT_CRT="${OUT_CRT:-server.crt}"
OUT_PEM="${OUT_PEM:-server.pem}"

CONF="$(mktemp)"
trap 'rm -f "${CONF}"' EXIT

cat >"${CONF}" <<EOF
[req]
default_bits       = 2048
prompt             = no
default_md         = sha256
distinguished_name = dn
x509_extensions    = v3_req

[dn]
C  = US
ST = Local
L  = Local
O  = Rearm
OU = k1roshio
CN = ${CN}

[v3_req]
basicConstraints = CA:FALSE
keyUsage = digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names

[alt_names]
DNS.1 = localhost
DNS.2 = rearm
DNS.3 = bench-rearm
DNS.4 = *.local
IP.1  = 127.0.0.1
IP.2  = ::1
EOF

openssl req -x509 -nodes -newkey rsa:2048 \
  -keyout "${OUT_KEY}" \
  -out "${OUT_CRT}" \
  -days "${DAYS}" \
  -config "${CONF}" \
  -extensions v3_req

cat "${OUT_CRT}" "${OUT_KEY}" >"${OUT_PEM}"
chmod 600 "${OUT_KEY}" "${OUT_PEM}"
chmod 644 "${OUT_CRT}"

echo "Wrote ${ROOT}/${OUT_KEY}"
echo "Wrote ${ROOT}/${OUT_CRT}"
echo "Wrote ${ROOT}/${OUT_PEM}"
echo "SANs: localhost, rearm, bench-rearm, 127.0.0.1"
