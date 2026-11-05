<?php

/**
 * Defaults merged into config('octane.rearm').
 * Publish and/or copy the 'rearm' key into your app's config/octane.php.
 *
 * @return array{rearm: array<string, mixed>}
 */
return [

    'rearm' => [

        /*
        | Absolute path to the rearm-server binary (or name on PATH).
        */
        'bin' => env('REARM_BIN', 'rearm-server'),

        /*
        | HTTP/1.1 listen address (host:port).
        */
        'http_addr' => env('REARM_HTTP_ADDR', '0.0.0.0:8000'),

        /*
        | Opt-in HTTP/3 (QUIC). Requires cert + key when true.
        */
        'http3' => env('REARM_HTTP3', false),
        'http3_addr' => env('REARM_HTTP3_ADDR', '0.0.0.0:4433'),
        'cert' => env('REARM_CERT'),
        'key' => env('REARM_KEY'),

        /*
        | Elastic PHP worker pool (short / default).
        */
        'workers' => env('REARM_WORKERS', 8),
        'workers_max' => env('REARM_WORKERS_MAX', 16),

        /*
        | Long pool (empty long_paths = single shared pool).
        */
        'long_paths' => env('REARM_LONG_PATHS', ''),
        'long_workers' => env('REARM_LONG_WORKERS', 4),
        'long_workers_max' => env('REARM_LONG_WORKERS_MAX', 6),

        /*
        | Scale / recycle.
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
        | Paths / runtime.
        */
        'php_bin' => env('REARM_PHP_BIN', 'php'),
        'worker_script' => env('REARM_WORKER_SCRIPT'),
        'app_root' => env('REARM_APP_ROOT'),
        'upload_dir' => env('REARM_UPLOAD_DIR'),
        'sock_dir' => env('REARM_SOCK_DIR', '/tmp/rearm-socks'),
        'compress' => env('REARM_COMPRESS', 'brotli'),

        /*
        | Deferred client wait (X-Rearm-Defer-Ms).
        */
        'defer_max_ms' => env('REARM_DEFER_MAX_MS', 300_000),
        'defer_chunk_ms' => env('REARM_DEFER_CHUNK_MS', 250),

    ],

];
