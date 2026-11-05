# Rearm

Rust application server for Laravel 12 (PHP 8.5+): HTTP/1.1 + optional HTTP/3, Brotli, multipart uploads, long-lived PHP workers, elastic pools, and fork-based scale-up.

## Layout

| Path | Role |
|------|------|
| [`crates/rearm-server`](crates/rearm-server) | `rearm-server` binary |
| [`crates/rearm-protocol`](crates/rearm-protocol) | MessagePack framing |
| [`packages/octane-driver`](packages/octane-driver) | PHP Octane worker / Artisan commands (`k1roshio/octane-driver-rearm`) |
| [`docker/rearm`](docker/rearm) | Runtime image (binary + PHP CLI) |
| [`certs/`](certs) | TLS helper for HTTP/3 (`./certs/generate.sh`) |

## Requirements

- Rust (stable) to build the server, **or** Docker
- PHP 8.5+ with `pcntl`, `sockets` (and `ffi` if using fork scale)
- A Laravel 12 app with [`laravel/octane`](https://github.com/laravel/octane)

## Quick start (Composer package)

Packagist package name: **`k1roshio/octane-driver-rearm`** (publish from a repo whose *root* is this package — see [`docs/PACKAGIST.md`](docs/PACKAGIST.md)).

**Full deploy template** (Octane config, all `REARM_*` knobs, systemd, tuning): [`docs/DEPLOY.md`](docs/DEPLOY.md).

```bash
composer require k1roshio/octane-driver-rearm
php artisan rearm:install-binary   # downloads Linux prebuild into bin/rearm-server
# Add config/octane.php → 'server' => 'rearm' and a 'rearm' => […] section
php artisan octane:start --server=rearm
# or: php artisan rearm:start
```

Defaults live in the package (`config/rearm.php`) and merge into `config('octane.rearm')`. Override via `.env` (`REARM_*`) or CLI flags.

## Install the binary

Preferred (Linux x86_64 / aarch64) — from [GitHub Releases](https://github.com/k1roshio/rearm-php/releases):

```bash
php artisan rearm:install-binary
# or without Laravel: vendor/bin/rearm-install
```

Puts `bin/rearm-server` in the app root (auto-detected on start). Optional: `REARM_BIN=/path/to/rearm-server`.

Build from source instead:

```bash
cargo build --release -p rearm-server
# → target/release/rearm-server
```

Publish a release: push a tag `v*` on this repo (see `.github/workflows/release.yml`).

## Docker

```bash
./certs/generate.sh          # optional, for HTTP/3
docker compose build
# Mount your Laravel app at /var/www/app (see docker-compose.yml)
docker compose up
```

HTTP: `http://127.0.0.1:8080` · HTTP/3: set `REARM_HTTP3=true` and mount certs.

## Configuration

Rust reads env / CLI (see `rearm-server --help`). Full Octane + scaling reference: [`docs/DEPLOY.md`](docs/DEPLOY.md).

| Variable | Meaning |
|----------|---------|
| `REARM_HTTP_ADDR` | HTTP/1.1 listen |
| `REARM_HTTP3` | Opt-in QUIC (`false` by default) |
| `REARM_WORKERS` / `REARM_WORKERS_MAX` | Warm min / elastic max PHP workers |
| `REARM_SCALE_UP_*` | Waiters, queue ms, cooldown, batch — when to grow |
| `REARM_SCALE_DOWN_*` | Idle ms, grace ms, batch — when to shrink |
| `REARM_LONG_PATHS` (+ `REARM_LONG_WORKERS*`) | Optional slow-route pool split |
| `REARM_FORK_SCALE` | Warm donor + `pcntl_fork` scale-up |
| `REARM_MEMORY_LIMIT_MB` / `REARM_MAX_REQUESTS` | Per-worker recycle |
| `REARM_APP_ROOT` / `REARM_WORKER_SCRIPT` | Laravel root / worker entrypoint |

Metrics: `GET /__rearm/metrics`

## Testing

```bash
# Rust unit tests
cargo test --workspace

# PHP Protocol / cross-compat fixtures (needs composer install in the package)
cd packages/octane-driver && composer install && composer test

# Regenerate Rust→PHP wire fixtures (optional)
cargo run -p rearm-protocol --example write_fixtures

# End-to-end smoke: rearm-server + echo-worker (Linux/WSL)
./scripts/integration-smoke.sh
```

CI (`.github/workflows/ci.yml`) runs all three on Ubuntu.

## License

MIT © [k1roshio](https://github.com/k1roshio) — creator of Rearm. See [`LICENSE`](LICENSE).
