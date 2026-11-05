#!/usr/bin/env bash
# End-to-end smoke: rearm-server + echo-worker.php over Unix sockets.
# Requires Linux/WSL, PHP with sockets, curl, and a Rust toolchain.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PKG="${ROOT}/packages/octane-driver"
ECHO_WORKER="${PKG}/bin/echo-worker.php"
WORKDIR="$(mktemp -d /tmp/rearm-smoke.XXXXXX)"
PORT="${REARM_SMOKE_PORT:-18080}"
SERVER_PID=""
LOG="${WORKDIR}/server.log"

# Non-login shells (CI / bare bash) often miss ~/.cargo/bin.
if ! command -v cargo >/dev/null 2>&1; then
  # shellcheck disable=SC1091
  [[ -f "${HOME}/.cargo/env" ]] && source "${HOME}/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found on PATH" >&2
  exit 1
fi

cleanup() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "${SERVER_PID}" 2>/dev/null; then
    kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
  fi
  rm -rf "${WORKDIR}"
}
trap cleanup EXIT

echo "==> workdir ${WORKDIR}"

if [[ ! -f "${PKG}/vendor/autoload.php" ]]; then
  echo "==> composer install (package)"
  (cd "${PKG}" && composer install --no-interaction --ignore-platform-reqs)
fi

echo "==> cargo build -p rearm-server"
(cd "${ROOT}" && cargo build -p rearm-server)

BIN="${ROOT}/target/debug/rearm-server"
[[ -x "${BIN}" ]] || BIN="${ROOT}/target/release/rearm-server"
if [[ ! -x "${BIN}" ]]; then
  echo "rearm-server binary not found" >&2
  exit 1
fi

mkdir -p "${WORKDIR}/app/public" "${WORKDIR}/uploads" "${WORKDIR}/socks"
# Minimal public dir so static-file lookups do not explode.
echo "ok" >"${WORKDIR}/app/public/index.html"

export REARM_HTTP_ADDR="127.0.0.1:${PORT}"
export REARM_HTTP3=false
export REARM_WORKERS=1
export REARM_WORKERS_MAX=1
export REARM_FORK_SCALE=false
export REARM_APP_ROOT="${WORKDIR}/app"
export REARM_UPLOAD_DIR="${WORKDIR}/uploads"
export REARM_SOCK_DIR="${WORKDIR}/socks"
export REARM_WORKER_SCRIPT="${ECHO_WORKER}"
export REARM_PHP_BIN="${REARM_PHP_BIN:-php}"
export REARM_COMPRESS=off
export REARM_MAX_REQUESTS=0

echo "==> starting rearm-server on ${REARM_HTTP_ADDR}"
"${BIN}" >"${LOG}" 2>&1 &
SERVER_PID=$!

ready=0
for _ in $(seq 1 60); do
  if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
    echo "server died during boot" >&2
    cat "${LOG}" >&2 || true
    exit 1
  fi
  if curl -sf "http://127.0.0.1:${PORT}/__rearm/health" >/dev/null 2>&1; then
    ready=1
    break
  fi
  sleep 0.25
done

if [[ "${ready}" -ne 1 ]]; then
  echo "health check timed out" >&2
  cat "${LOG}" >&2 || true
  exit 1
fi

echo "==> GET /__rearm/health"
health="$(curl -sf "http://127.0.0.1:${PORT}/__rearm/health")"
echo "${health}" | grep -q . 

echo "==> GET /__rearm/metrics"
metrics="$(curl -sf "http://127.0.0.1:${PORT}/__rearm/metrics")" || {
  echo "metrics request failed" >&2
  cat "${LOG}" >&2 || true
  exit 1
}
if ! echo "${metrics}" | grep -Eq '"service"[[:space:]]*:[[:space:]]*"rearm-server"'; then
  echo "metrics missing service=rearm-server: ${metrics}" >&2
  exit 1
fi

echo "==> GET /ping (echo-worker)"
pong="$(curl -sf "http://127.0.0.1:${PORT}/ping")" || {
  echo "ping request failed" >&2
  cat "${LOG}" >&2 || true
  exit 1
}
if ! echo "${pong}" | grep -Eq '"pong"[[:space:]]*:[[:space:]]*true'; then
  echo "unexpected ping body: ${pong}" >&2
  exit 1
fi
echo "${pong}" | grep -Eq '"method"[[:space:]]*:[[:space:]]*"GET"'
# PHP json_encode may escape slashes as \/
echo "${pong}" | grep -Eq '"uri"[[:space:]]*:[[:space:]]*"\\?/ping"'

echo "SMOKE OK"
