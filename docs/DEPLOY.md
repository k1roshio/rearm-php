# Deploy template — Laravel on Rearm

English guide for running an existing Laravel 12 / Octane app on **Rearm** instead of Swoole/RoadRunner/FrankenPHP.

Rearm is two pieces:

1. **`rearm-server`** — Rust binary (HTTP/1.1, optional HTTP/3, worker supervisor)
2. **`k1roshio/octane-driver-rearm`** — PHP Octane driver + MessagePack worker

You do **not** vendor this monorepo into the app. Install the Composer package and a binary (download or build).

---

## Requirements

| Requirement | Notes |
|-------------|--------|
| **OS** | Linux or WSL2 (Unix domain sockets). Native Windows PHP is not supported. |
| **PHP** | **8.5+** with **`ext-pcntl`** and **`ext-sockets`** (declared in `composer.json`) |
| **Optional** | **`ext-ffi`** — recommended when `REARM_FORK_SCALE=true` (default) |
| **Laravel** | 12.x + [`laravel/octane`](https://github.com/laravel/octane) ^2 |
| **App code** | Standard Laravel / Octane request lifecycle. Swoole-only APIs (`Coroutine`, `go()`, Swoole tables/websockets) will not work. |

Opcache tip when using fork scale: disable JIT for CLI workers (`opcache.jit=0`) — JIT + `pcntl_fork` often segfaults. Enable `ffi.enable=true` if you use FFI to close inherited FDs after fork. See [`docker/rearm/`](../docker/rearm/) for examples.

---

## 1. Install the PHP package

From your Laravel application root:

```bash
composer require k1roshio/octane-driver-rearm
```

The service provider auto-discovers. Defaults merge into `config('octane.rearm')` from the package’s `config/rearm.php`.

Optional publish stub:

```bash
php artisan vendor:publish --tag=rearm-config
# → config/octane-rearm.php  (copy the 'rearm' key into config/octane.php)
```

---

## 2. Install the `rearm-server` binary

### Preferred — GitHub Releases (Linux x86_64 / aarch64)

```bash
php artisan rearm:install-binary
# Flags:
#   --version=v0.1.0   specific tag (default: latest)
#   --dir=/custom/bin  install directory (default: {app}/bin)
#   --force            overwrite existing binary
#   --repo=k1roshio/rearm-php
```

Without Laravel bootstrap:

```bash
vendor/bin/rearm-install
# or: vendor/bin/rearm-install --version=v0.1.0 --force
```

This writes **`bin/rearm-server`** under the app root. Octane start **auto-detects** that path — you do **not** need `REARM_BIN` unless the binary lives elsewhere.

If this repository is **private**, set a token with `contents:read` (or classic repo scope):

```bash
export GITHUB_TOKEN=ghp_...
php artisan rearm:install-binary
```

### Alternative — build from source

```bash
git clone https://github.com/k1roshio/rearm-php.git
cd rearm-php
cargo build --release -p rearm-server
# → target/release/rearm-server
```

Then either put it on `PATH` as `rearm-server`, or:

```env
REARM_BIN=/absolute/path/to/rearm-server
```

### Alternative — Docker

Use this monorepo’s image ([`docker-compose.yml`](../docker-compose.yml)): mount your Laravel app at `/var/www/app` and set `REARM_*` env vars. The binary is baked into the image.

---

## 3. Configure Octane

In `config/octane.php`:

```php
<?php

return [

    'server' => env('OCTANE_SERVER', 'rearm'),

    // ... existing Octane keys (warm, listeners, etc.) ...

    'rearm' => [

        /*
        | Path or name of the rearm-server binary.
        | Leave default when using `php artisan rearm:install-binary` (bin/rearm-server).
        */
        'bin' => env('REARM_BIN', 'rearm-server'),

        /*
        | HTTP/1.1 listen address (host:port).
        */
        'http_addr' => env('REARM_HTTP_ADDR', '0.0.0.0:8000'),

        /*
        | Opt-in HTTP/3 (QUIC). Requires PEM cert + key when true.
        */
        'http3' => env('REARM_HTTP3', false),
        'http3_addr' => env('REARM_HTTP3_ADDR', '0.0.0.0:4433'),
        'cert' => env('REARM_CERT'),
        'key' => env('REARM_KEY'),

        /*
        | Elastic PHP worker pool (default / “short” pool).
        */
        'workers' => env('REARM_WORKERS', 8),
        'workers_max' => env('REARM_WORKERS_MAX', 16),

        /*
        | Optional second pool for slow routes.
        | Empty long_paths = single shared pool.
        | Patterns: exact `/io-slow`, prefix `/report*` or `/export` (+ children).
        */
        'long_paths' => env('REARM_LONG_PATHS', ''),
        'long_workers' => env('REARM_LONG_WORKERS', 4),
        'long_workers_max' => env('REARM_LONG_WORKERS_MAX', 6),

        /*
        | Elastic scale-up / scale-down / recycle.
        */
        'scale_up_waiters' => env('REARM_SCALE_UP_WAITERS', 2),
        'scale_up_queue_ms' => env('REARM_SCALE_UP_QUEUE_MS', 25),
        'scale_up_cooldown_ms' => env('REARM_SCALE_UP_COOLDOWN_MS', 250),
        'scale_up_batch' => env('REARM_SCALE_UP_BATCH', 4),
        'scale_down_idle_ms' => env('REARM_SCALE_DOWN_IDLE_MS', 5_000),
        'scale_down_grace_ms' => env('REARM_SCALE_DOWN_GRACE_MS', 15_000),
        'scale_down_batch' => env('REARM_SCALE_DOWN_BATCH', 1),
        'memory_limit_mb' => env('REARM_MEMORY_LIMIT_MB', 512),
        'max_requests' => env('REARM_MAX_REQUESTS', 0),
        'fork_scale' => env('REARM_FORK_SCALE', true),

        /*
        | Runtime paths (usually leave empty — derived from the Laravel app).
        */
        'php_bin' => env('REARM_PHP_BIN', 'php'),
        'worker_script' => env('REARM_WORKER_SCRIPT'), // default: package bin/worker.php
        'app_root' => env('REARM_APP_ROOT'),           // default: base_path()
        'upload_dir' => env('REARM_UPLOAD_DIR'),
        'sock_dir' => env('REARM_SOCK_DIR', '/tmp/rearm-socks'),
        'compress' => env('REARM_COMPRESS', 'brotli'), // brotli | gzip | off

        /*
        | Honoured from worker response header X-Rearm-Defer-Ms (internal).
        */
        'defer_max_ms' => env('REARM_DEFER_MAX_MS', 300_000),
        'defer_chunk_ms' => env('REARM_DEFER_CHUNK_MS', 250),

    ],

];
```

Artisan CLI overrides when starting:

```bash
php artisan octane:rearm --host=0.0.0.0 --port=8080 --workers=8 --workers-max=16 --bin=/path/to/rearm-server
```

---

## 4. Environment (`.env`)

### Recommended starter (bench-style)

Values below match a typical production-ish / fullbench profile: short pool **8→16**, optional long pool **4→6**, fork scale-up, Brotli. Copy into `.env` and adjust paths/sizes for your box.

```env
OCTANE_SERVER=rearm

# Listen
REARM_HTTP_ADDR=0.0.0.0:8000

# Short / default pool (warm → elastic max)
REARM_WORKERS=8
REARM_WORKERS_MAX=16

# Long pool (leave LONG_PATHS empty for a single shared pool)
# REARM_LONG_PATHS=/reports*,/export,/io-slow
REARM_LONG_WORKERS=4
REARM_LONG_WORKERS_MAX=6

# Elastic scale-up (grow when requests wait for a free worker)
REARM_SCALE_UP_WAITERS=2
REARM_SCALE_UP_QUEUE_MS=25
REARM_SCALE_UP_COOLDOWN_MS=250
REARM_SCALE_UP_BATCH=4

# Elastic scale-down (shed surplus workers after idle / post-burst grace)
REARM_SCALE_DOWN_IDLE_MS=5000
REARM_SCALE_DOWN_GRACE_MS=15000
REARM_SCALE_DOWN_BATCH=1

# Recycle / spawn strategy
REARM_MEMORY_LIMIT_MB=512
REARM_MAX_REQUESTS=0
REARM_FORK_SCALE=true

# Responses
REARM_COMPRESS=brotli

# Optional: only if binary is not at bin/rearm-server
# REARM_BIN=/usr/local/bin/rearm-server
```

`REARM_APP_ROOT` / `REARM_WORKER_SCRIPT` are set automatically when you start via Artisan (`base_path()` + package `worker.php`). Set them only when launching `rearm-server` directly (Docker/systemd without Octane).

### Full reference

| Variable | Default (Octane package) | Purpose |
|----------|--------------------------|---------|
| `OCTANE_SERVER` | — | Set to `rearm` |
| `REARM_BIN` | `rearm-server` | Binary path/name; omit if `bin/rearm-server` exists |
| `REARM_HTTP_ADDR` | `0.0.0.0:8000` | HTTP/1.1 bind |
| `REARM_HTTP3` | `false` | Enable QUIC |
| `REARM_HTTP3_ADDR` | `0.0.0.0:4433` | QUIC bind (UDP) |
| `REARM_CERT` / `REARM_KEY` | — | PEM paths for HTTP/3 |
| `REARM_WORKERS` | `8` | Warm (min) workers |
| `REARM_WORKERS_MAX` | `16` | Max elastic workers (`0` on CLI = same as min) |
| `REARM_LONG_PATHS` | _(empty)_ | Comma-separated slow-route allowlist |
| `REARM_LONG_WORKERS` | `4` | Warm workers in long pool |
| `REARM_LONG_WORKERS_MAX` | `6` | Max long-pool workers |
| `REARM_SCALE_UP_WAITERS` | `2` | Min queued waiters before scale-up |
| `REARM_SCALE_UP_QUEUE_MS` | `25` | Queue pressure must last this long |
| `REARM_SCALE_UP_COOLDOWN_MS` | `250` | Min gap between scale-up bursts |
| `REARM_SCALE_UP_BATCH` | `4` | Workers spawned per burst |
| `REARM_SCALE_DOWN_IDLE_MS` | `5000` | Idle time before shedding surplus workers |
| `REARM_SCALE_DOWN_GRACE_MS` | `15000` | Block scale-down after a scale-up |
| `REARM_SCALE_DOWN_BATCH` | `1` | Workers stopped per monitor tick |
| `REARM_MEMORY_LIMIT_MB` | `512` | Recycle worker when RSS exceeds this |
| `REARM_MAX_REQUESTS` | `0` | Recycle after N requests (`0` = disabled) |
| `REARM_FORK_SCALE` | `true` | Warm donor + `pcntl_fork` scale-up |
| `REARM_PHP_BIN` | `php` | PHP CLI used to spawn workers |
| `REARM_WORKER_SCRIPT` | package `worker.php` | Override only for non-Artisan launches |
| `REARM_APP_ROOT` | Artisan → `base_path()` | Override only for non-Artisan launches |
| `REARM_UPLOAD_DIR` | `/var/www/uploads` (Rust default) | Multipart temp dir; set if that path is wrong for your host |
| `REARM_SOCK_DIR` | `/tmp/rearm-socks` | Per-worker UDS directory |
| `REARM_COMPRESS` | `brotli` | `brotli` / `gzip` / `off` |
| `REARM_DEFER_MAX_MS` | `300000` | Cap for `X-Rearm-Defer-Ms` |
| `REARM_DEFER_CHUNK_MS` | `250` | Defer sleep slice |

CLI flags mirror the same names (`rearm-server --help`). Values from Artisan/`config('octane.rearm')` are exported as `REARM_*` when starting via Octane.

---

## 5. Elastic scaling (env knobs)

Rearm keeps a **warm pool** of PHP workers and can grow/shrink it under queue pressure. All of this is controlled with `REARM_*` (or the matching keys under `config('octane.rearm')`).

### Pool size

```env
REARM_WORKERS=8          # minimum warm workers (always running)
REARM_WORKERS_MAX=16     # hard ceiling for elastic scale-up
```

- Traffic is quiet → pool stays near `REARM_WORKERS`.
- Requests wait for a free worker → pool grows toward `REARM_WORKERS_MAX`.
- Set `REARM_WORKERS_MAX` equal to `REARM_WORKERS` (or `0` on the Rust CLI, which means “same as min”) for a **fixed** pool (no elasticity).

### How scale-up decides to spawn

Scale-up only fires when **enough** requests have been waiting **long enough**, and not more often than the cooldown allows:

```env
REARM_SCALE_UP_WAITERS=2        # at least N requests waiting for a free worker
REARM_SCALE_UP_QUEUE_MS=25      # that pressure must persist this many ms (filters one-shot spikes)
REARM_SCALE_UP_COOLDOWN_MS=250  # minimum ms between scale-up bursts
REARM_SCALE_UP_BATCH=4          # max new workers created in one burst
```

| Env | Effect when **raised** | Effect when **lowered** |
|-----|------------------------|-------------------------|
| `REARM_SCALE_UP_WAITERS` | Needs a deeper queue before growing | Grows earlier (more sensitive) |
| `REARM_SCALE_UP_QUEUE_MS` | Ignores short spikes | Reacts faster to brief contention |
| `REARM_SCALE_UP_COOLDOWN_MS` | Fewer spawn storms | Can burst more often |
| `REARM_SCALE_UP_BATCH` | Bigger jumps toward max | Gentler, smaller steps |

### How scale-down sheds surplus workers

Workers **above** `REARM_WORKERS` can be stopped when idle — but not immediately after a scale-up (anti-churn grace):

```env
REARM_SCALE_DOWN_IDLE_MS=5000    # surplus worker must be idle this long before stop
REARM_SCALE_DOWN_GRACE_MS=15000  # after a scale-up burst, block scale-down for this long
REARM_SCALE_DOWN_BATCH=1         # how many idle surplus workers to stop per monitor tick
```

| Env | Effect when **raised** | Effect when **lowered** |
|-----|------------------------|-------------------------|
| `REARM_SCALE_DOWN_IDLE_MS` | Keeps burst capacity longer | Sheds RAM sooner after idle |
| `REARM_SCALE_DOWN_GRACE_MS` | Less flap after traffic spikes | Returns to min pool faster |
| `REARM_SCALE_DOWN_BATCH` | Faster shrink | Smoother, slower shrink |

### Fork scale vs cold spawn

```env
REARM_FORK_SCALE=true    # default: warm PHP donor + pcntl_fork (CoW) for fast scale-up
# REARM_FORK_SCALE=false # always cold-spawn PHP (simpler; no FFI/JIT fork caveats)
```

With `REARM_FORK_SCALE=true`, prefer `ext-ffi` and `opcache.jit=0` on the PHP CLI used for workers (see Docker ini snippets under `docker/rearm/`).

### Worker recycle (memory / request caps)

Independent of elastic size — restarts a **single** worker in place:

```env
REARM_MEMORY_LIMIT_MB=512   # recycle when worker RSS exceeds this (MiB)
REARM_MAX_REQUESTS=0        # recycle after N handled requests (0 = disabled in Octane defaults)
```

### Split pool for slow routes (optional)

Keep fast APIs on the default pool; pin heavy paths to a second pool so they do not starve short requests:

```env
REARM_LONG_PATHS=/reports*,/export,/io-slow
REARM_LONG_WORKERS=4
REARM_LONG_WORKERS_MAX=6
```

Patterns: exact (`/io-slow`), prefix glob (`/report*`), or path prefix (`/export` matches `/export/...`). Empty `REARM_LONG_PATHS` = **one** shared pool (default).

Each pool has its own min/max; scale-up/down knobs above apply to the supervisor’s elastic behaviour for those pools.

### Example profiles

**Fixed small pool (dev / tiny VPS):**
```env
REARM_WORKERS=2
REARM_WORKERS_MAX=2
REARM_FORK_SCALE=false
```

**Bursty API (default-ish production):**
```env
REARM_WORKERS=8
REARM_WORKERS_MAX=24
REARM_SCALE_UP_WAITERS=2
REARM_SCALE_UP_QUEUE_MS=25
REARM_SCALE_UP_COOLDOWN_MS=250
REARM_SCALE_UP_BATCH=4
REARM_SCALE_DOWN_IDLE_MS=5000
REARM_SCALE_DOWN_GRACE_MS=15000
REARM_FORK_SCALE=true
```

**Fast traffic + slow reports:**
```env
REARM_WORKERS=8
REARM_WORKERS_MAX=16
REARM_LONG_PATHS=/reports*,/admin/export*
REARM_LONG_WORKERS=2
REARM_LONG_WORKERS_MAX=6
```

Watch live pool state: `GET /__rearm/metrics`.

---

## 6. Start

```bash
php artisan octane:start --server=rearm
# aliases:
php artisan rearm:start
php artisan octane:rearm --host=0.0.0.0 --port=8000 --workers=8 --workers-max=16
```

Process model: one long-lived `rearm-server` supervising PHP workers (not php-fpm on the same port). Put a reverse proxy (nginx/Caddy) in front if you need TLS termination for HTTP/1.1.

### Health & metrics

- `GET /__rearm/health` — liveness JSON  
- `GET /__rearm/metrics` — pool / worker stats (`?workers=0` for compact JSON)

---

## 7. Tuning cheat sheet

| Goal | Knobs |
|------|--------|
| More baseline concurrency | Raise `REARM_WORKERS` |
| Allow bursts | Raise `REARM_WORKERS_MAX` |
| Isolate slow endpoints | `REARM_LONG_PATHS` + `REARM_LONG_WORKERS` / `_MAX` |
| Faster scale-up under load | Lower `REARM_SCALE_UP_QUEUE_MS` / `REARM_SCALE_UP_COOLDOWN_MS`, raise `REARM_SCALE_UP_BATCH` |
| Scale up only under real queues | Raise `REARM_SCALE_UP_WAITERS` / `REARM_SCALE_UP_QUEUE_MS` |
| Less churn after spikes | Raise `REARM_SCALE_DOWN_GRACE_MS` / `REARM_SCALE_DOWN_IDLE_MS` |
| Shed RAM sooner when idle | Lower `REARM_SCALE_DOWN_IDLE_MS`, raise `REARM_SCALE_DOWN_BATCH` |
| Cap memory leaks | `REARM_MEMORY_LIMIT_MB`, optional `REARM_MAX_REQUESTS` |
| Safer/simple deploys | `REARM_FORK_SCALE=false` |
| Smaller responses | `REARM_COMPRESS=brotli` (or `gzip`) |
| HTTP/3 | `REARM_HTTP3=true` + cert/key (+ UDP port open) |

---

## 8. Example systemd unit

```ini
[Unit]
Description=Rearm (Laravel Octane)
After=network.target

[Service]
Type=simple
User=www-data
WorkingDirectory=/var/www/app
EnvironmentFile=/var/www/app/.env
ExecStart=/usr/bin/php artisan octane:start --server=rearm
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

Ensure `bin/rearm-server` is installed (or `REARM_BIN` is set) and the service user can execute it and write to `REARM_SOCK_DIR` / upload dir.

---

## 9. Deploy checklist

- [ ] Linux/WSL host with PHP 8.5+, `pcntl`, `sockets` (+ `ffi` if fork scale)
- [ ] `composer require k1roshio/octane-driver-rearm`
- [ ] `php artisan rearm:install-binary` (or custom `REARM_BIN`)
- [ ] `config/octane.php` → `'server' => 'rearm'` + `'rearm' => […]`
- [ ] `.env` → `OCTANE_SERVER=rearm` and pool/listen vars
- [ ] No Swoole-only APIs left in application code
- [ ] Proxy / firewall allows HTTP (and UDP for HTTP/3 if enabled)
- [ ] Smoke: `curl -sf http://127.0.0.1:8000/__rearm/health`

---

## Related

- Packagist package: [`k1roshio/octane-driver-rearm`](https://packagist.org/packages/k1roshio/octane-driver-rearm)
- Binary releases: [github.com/k1roshio/rearm-php/releases](https://github.com/k1roshio/rearm-php/releases)
- Package source: [github.com/k1roshio/octane-driver-rearm](https://github.com/k1roshio/octane-driver-rearm)
- Docker runtime: [`docker/rearm/Dockerfile`](../docker/rearm/Dockerfile)
