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

```bash
composer require k1roshio/octane-driver-rearm
# Install / place the rearm-server binary on PATH (or set REARM_BIN)
# Add config/octane.php → 'server' => 'rearm' and a 'rearm' => […] section
php artisan octane:start --server=rearm
# or: php artisan rearm:start
```

Defaults live in the package (`config/rearm.php`) and merge into `config('octane.rearm')`. Override via `.env` (`REARM_*`) or CLI flags.

## Build the binary

```bash
cargo build --release -p rearm-server
# → target/release/rearm-server
```

## Docker

```bash
./certs/generate.sh          # optional, for HTTP/3
docker compose build
# Mount your Laravel app at /var/www/app (see docker-compose.yml)
docker compose up
```

HTTP: `http://127.0.0.1:8080` · HTTP/3: set `REARM_HTTP3=true` and mount certs.

## Configuration

Rust reads env / CLI (see `rearm-server --help`). Common variables:

| Variable | Meaning |
|----------|---------|
| `REARM_HTTP_ADDR` | HTTP/1.1 listen (`0.0.0.0:8080`) |
| `REARM_HTTP3` | Opt-in QUIC (`false` by default) |
| `REARM_WORKERS` / `REARM_WORKERS_MAX` | Elastic PHP pool |
| `REARM_LONG_PATHS` | Split long pool allowlist (comma-separated) |
| `REARM_FORK_SCALE` | Warm donor + `pcntl_fork` scale-up |
| `REARM_APP_ROOT` | Laravel application path |
| `REARM_WORKER_SCRIPT` | Path to `packages/octane-driver/bin/worker.php` |

Metrics: `GET /__rearm/metrics`

## License

MIT © [k1roshio](https://github.com/k1roshio) — creator of Rearm. See [`LICENSE`](LICENSE).
