# k1roshio/octane-driver-rearm

Laravel 12 / PHP 8.5 Octane worker that speaks Rearm’s length-prefixed MessagePack protocol over a Unix domain socket.

The Rust `rearm-server` binary is the real process supervisor. This package:

1. Listens on `REARM_SOCK` (PHP side)
2. Boots the Laravel app once from `REARM_APP_ROOT`
3. Accepts keep-alive connections from Rust, handles framed requests, returns framed responses
4. Resets Octane sandbox state between requests when Octane APIs are available

## Install

Path repository from the monorepo (see `apps/bench`):

```json
{
  "repositories": [
    {
      "type": "path",
      "url": "../../packages/octane-driver",
      "options": { "symlink": true }
    }
  ],
  "require": {
    "k1roshio/octane-driver-rearm": "*"
  }
}
```

MessagePack is pure PHP via [`rybakit/msgpack`](https://github.com/rybakit/msgpack.php). The C `ext-msgpack` extension is optional and can be added in Docker later without changing this package.

## Environment

| Variable | Required | Description |
|----------|----------|-------------|
| `REARM_SOCK` | yes | Unix socket path. **PHP listens**; Rust connects. |
| `REARM_APP_ROOT` | yes (Laravel worker) | Absolute path to the Laravel application root. |
| `REARM_BIN` | no | Path to `rearm-server` when starting via `RearmServer::start()`. |
| `REARM_WORKER_SCRIPT` | no | Worker script path passed through to Rust. |
| `REARM_WORKERS` | no | Worker count hint for the Rust supervisor. |

## Entrypoints

```bash
# Laravel worker (production path)
REARM_SOCK=/tmp/rearm-worker-0.sock \
REARM_APP_ROOT=/app/apps/bench \
php vendor/bin/worker.php

# Echo worker — no Laravel, JSON pong (Rust smoke tests)
REARM_SOCK=/tmp/rearm-echo.sock \
php vendor/k1roshio/octane-driver-rearm/bin/echo-worker.php
```

## Socket contract (for Rust)

1. **Listener:** PHP creates `unix://$REARM_SOCK` with `stream_socket_server` and accepts connections.
2. **Connector:** Rust connects to that path and **keeps the connection open** (keep-alive pool; one sock per worker process).
3. **Frame:** `[u32 big-endian length][MessagePack body]`, max payload 64 MiB.
4. **Request map** (named keys, `rmp_serde::to_vec_named` compatible):

   | Key | Type | Notes |
   |-----|------|--------|
   | `method` | str | HTTP method |
   | `uri` | str | Path + query |
   | `headers` | map str→str | Lower or mixed case OK |
   | `remote_addr` | str | e.g. `127.0.0.1:54321` |
   | `body` | **bin** | Non-multipart raw body |
   | `form` | map str→str? | Multipart fields (omit if none) |
   | `files` | array of maps? | `{field,path,original_name,mime,size}` — omit if none |
   | `control` | str? | e.g. `"shutdown"` |

5. **Response map:**

   | Key | Type |
   |-----|------|
   | `status` | int / u16 |
   | `headers` | map str→str |
   | `body` | **bin** (uncompressed; Rust may Brotli/Gzip) |

6. **Shutdown:** send a request with `control = "shutdown"`. The worker finishes the current request (if any), then exits `0`. A control-only frame (`method`/`uri` empty) exits without invoking Laravel.

7. **Uploads:** Rust parses multipart, streams file parts to disk, and sends `files[]` metadata. PHP builds `Illuminate\Http\UploadedFile` with `test: true` from `path`.

8. **Lifecycle per worker:** unlink stale sock → listen → accept → read/write frames until EOF or shutdown → unlink sock.

## Laravel / Octane

Auto-discovers `RearmServiceProvider`. Defaults merge into `config('octane.rearm')`.

```bash
php artisan vendor:publish --tag=rearm-config   # optional stub → config/octane-rearm.php
# Prefer a 'rearm' => […] section inside your app's config/octane.php.

php artisan octane:start --server=rearm
php artisan rearm:start --workers=8
php artisan octane:rearm --host=0.0.0.0 --port=8080
```

CLI flags / `config/octane.php` `rearm` section map to `REARM_*` for the Rust binary.

## PHP API

```php
use Rearm\Octane\RearmServer;
use Rearm\Octane\Protocol;

// Supervised Laravel worker
RearmServer::makeWorker()->run();

// Hand off to Rust binary (pcntl_exec when available)
(new RearmServer)->start([
    'app_root' => base_path(),
    'workers' => 4,
    'config' => config('octane.rearm'),
]);

// Codec helpers
$protocol = new Protocol;
$framed = $protocol->encodeResponse(['status' => 200, 'headers' => [], 'body' => 'ok']);
```

## Package layout

```
packages/octane-driver/
  composer.json
  README.md
  config/rearm.php
  bin/worker.php
  bin/echo-worker.php
  src/RearmServiceProvider.php
  src/RearmServer.php
  src/Worker.php
  src/Protocol.php
  src/Commands/
```

PSR-4: `Rearm\Octane\` → `src/`.
