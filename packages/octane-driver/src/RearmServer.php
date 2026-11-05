<?php

declare(strict_types=1);

namespace Rearm\Octane;

use InvalidArgumentException;
use RuntimeException;

/**
 * Octane-facing entry for Rearm.
 *
 * The Rust `rearm-server` binary owns process supervision (spawn, RSS limits,
 * max_requests, graceful drain). This class resolves the worker script path and
 * can exec/hand off to that binary when starting via Artisan/Octane.
 */
final class RearmServer
{
    public const ENV_SOCK = 'REARM_SOCK';

    public const ENV_APP_ROOT = 'REARM_APP_ROOT';

    public const ENV_BIN = 'REARM_BIN';

    public const ENV_WORKER_SCRIPT = 'REARM_WORKER_SCRIPT';

    public const ENV_WORKERS = 'REARM_WORKERS';

    /**
     * config('octane.rearm') key → REARM_* env var for the Rust binary.
     *
     * @var array<string, string>
     */
    private const CONFIG_ENV_MAP = [
        'http_addr' => 'REARM_HTTP_ADDR',
        'http3' => 'REARM_HTTP3',
        'http3_addr' => 'REARM_HTTP3_ADDR',
        'cert' => 'REARM_CERT',
        'key' => 'REARM_KEY',
        'workers' => 'REARM_WORKERS',
        'workers_max' => 'REARM_WORKERS_MAX',
        'scale_up_waiters' => 'REARM_SCALE_UP_WAITERS',
        'scale_up_queue_ms' => 'REARM_SCALE_UP_QUEUE_MS',
        'scale_up_cooldown_ms' => 'REARM_SCALE_UP_COOLDOWN_MS',
        'scale_up_batch' => 'REARM_SCALE_UP_BATCH',
        'scale_down_idle_ms' => 'REARM_SCALE_DOWN_IDLE_MS',
        'scale_down_grace_ms' => 'REARM_SCALE_DOWN_GRACE_MS',
        'scale_down_batch' => 'REARM_SCALE_DOWN_BATCH',
        'memory_limit_mb' => 'REARM_MEMORY_LIMIT_MB',
        'max_requests' => 'REARM_MAX_REQUESTS',
        'php_bin' => 'REARM_PHP_BIN',
        'worker_script' => 'REARM_WORKER_SCRIPT',
        'app_root' => 'REARM_APP_ROOT',
        'upload_dir' => 'REARM_UPLOAD_DIR',
        'compress' => 'REARM_COMPRESS',
        'sock_dir' => 'REARM_SOCK_DIR',
        'long_paths' => 'REARM_LONG_PATHS',
        'long_workers' => 'REARM_LONG_WORKERS',
        'long_workers_max' => 'REARM_LONG_WORKERS_MAX',
        'defer_max_ms' => 'REARM_DEFER_MAX_MS',
        'defer_chunk_ms' => 'REARM_DEFER_CHUNK_MS',
        'fork_scale' => 'REARM_FORK_SCALE',
    ];

    /**
     * Absolute path to the Laravel worker CLI entrypoint.
     */
    public static function workerScript(): string
    {
        return dirname(__DIR__).DIRECTORY_SEPARATOR.'bin'.DIRECTORY_SEPARATOR.'worker.php';
    }

    /**
     * Absolute path to the Laravel-free echo worker (Rust integration tests).
     */
    public static function echoWorkerScript(): string
    {
        return dirname(__DIR__).DIRECTORY_SEPARATOR.'bin'.DIRECTORY_SEPARATOR.'echo-worker.php';
    }

    /**
     * Build a Worker configured from environment / options.
     *
     * @param  array{app_root?: string, sock?: string}  $options
     */
    public static function makeWorker(array $options = []): Worker
    {
        $appRoot = $options['app_root']
            ?? self::env(self::ENV_APP_ROOT)
            ?? getcwd();

        $sock = $options['sock']
            ?? self::env(self::ENV_SOCK);

        if ($appRoot === false || $appRoot === '') {
            throw new InvalidArgumentException('REARM_APP_ROOT (or app_root) is required.');
        }

        if ($sock === null || $sock === '') {
            throw new InvalidArgumentException('REARM_SOCK (or sock) is required.');
        }

        return new Worker((string) $appRoot, (string) $sock);
    }

    /**
     * Resolve the rearm-server binary path.
     *
     * @param  array{bin?: string}  $options
     */
    public static function resolveBinary(array $options = []): string
    {
        $bin = $options['bin'] ?? self::env(self::ENV_BIN) ?? 'rearm-server';

        if ($bin !== 'rearm-server' && is_file($bin) && is_executable($bin)) {
            return $bin;
        }

        return $bin;
    }

    /**
     * Map config('octane.rearm') (+ overrides) to REARM_* env strings for Rust.
     *
     * @param  array<string, mixed>  $rearm
     * @param  array<string, mixed>  $overrides  same keys as config (workers, http_addr, …)
     * @return array<string, string>
     */
    public static function envFromConfig(array $rearm = [], array $overrides = []): array
    {
        $merged = array_replace($rearm, array_filter(
            $overrides,
            static fn ($value) => $value !== null && $value !== '',
        ));

        $env = [];

        foreach (self::CONFIG_ENV_MAP as $key => $envKey) {
            if (! array_key_exists($key, $merged)) {
                continue;
            }

            $value = $merged[$key];
            if ($value === null || $value === '') {
                continue;
            }

            $env[$envKey] = self::stringifyEnvValue($value);
        }

        return $env;
    }

    /**
     * Hand off to the Rust supervisor. Does not return on success when using exec.
     *
     * Expected Rust-side contract: spawn N PHP workers with unique REARM_SOCK
     * paths and REARM_WORKER_SCRIPT pointing at {@see workerScript()}.
     *
     * @param  array{
     *     bin?: string,
     *     app_root?: string,
     *     workers?: int|string,
     *     workers_max?: int|string,
     *     worker_script?: string,
     *     http_addr?: string,
     *     config?: array<string, mixed>,
     *     args?: list<string>,
     *     env?: array<string, string>
     * }  $options
     */
    public function start(array $options = []): int
    {
        $config = $options['config'] ?? [];
        if ($config === [] && function_exists('config')) {
            /** @var array<string, mixed> $config */
            $config = (array) config('octane.rearm', []);
        }

        $overrides = $options;
        unset($overrides['config'], $overrides['args'], $overrides['env'], $overrides['bin']);

        $fromConfig = self::envFromConfig($config, $overrides);

        $bin = self::resolveBinary([
            'bin' => $options['bin'] ?? $config['bin'] ?? null,
        ]);

        $appRoot = (string) (
            $fromConfig[self::ENV_APP_ROOT]
            ?? $options['app_root']
            ?? self::env(self::ENV_APP_ROOT)
            ?: (function_exists('base_path') ? base_path() : getcwd())
        );

        $workerScript = (string) (
            $fromConfig[self::ENV_WORKER_SCRIPT]
            ?? $options['worker_script']
            ?? self::workerScript()
        );

        if (! is_file($workerScript)) {
            throw new RuntimeException("Worker script not found: {$workerScript}");
        }

        $args = $options['args'] ?? [];
        $command = array_merge([$bin], $args);

        $env = array_merge(
            self::processEnvironment(),
            $fromConfig,
            [
                self::ENV_APP_ROOT => $appRoot,
                self::ENV_WORKER_SCRIPT => $workerScript,
                'LARAVEL_OCTANE' => '1',
            ],
            $options['env'] ?? [],
        );

        if (function_exists('pcntl_exec')) {
            pcntl_exec($command[0], array_values(array_slice($command, 1)), $env);
            throw new RuntimeException("pcntl_exec failed for {$bin}");
        }

        $descriptors = [
            0 => STDIN,
            1 => STDOUT,
            2 => STDERR,
        ];

        $process = proc_open(
            $command,
            $descriptors,
            $pipes,
            $appRoot,
            $env,
        );

        if (! is_resource($process)) {
            throw new RuntimeException("Failed to start Rearm binary: {$bin}");
        }

        return proc_close($process);
    }

    /**
     * @return array<string, string>
     */
    private static function processEnvironment(): array
    {
        $env = [];

        foreach ($_ENV as $key => $value) {
            if (is_string($key) && (is_string($value) || is_numeric($value))) {
                $env[$key] = (string) $value;
            }
        }

        foreach ($_SERVER as $key => $value) {
            if (is_string($key) && (is_string($value) || is_numeric($value)) && ! array_key_exists($key, $env)) {
                $env[$key] = (string) $value;
            }
        }

        return $env;
    }

    private static function stringifyEnvValue(mixed $value): string
    {
        if (is_bool($value)) {
            return $value ? 'true' : 'false';
        }

        if (is_array($value)) {
            return implode(',', array_map(
                static fn ($item) => self::stringifyEnvValue($item),
                $value,
            ));
        }

        return (string) $value;
    }

    private static function env(string $key): ?string
    {
        $value = $_ENV[$key] ?? $_SERVER[$key] ?? getenv($key);

        if ($value === false || $value === null || $value === '') {
            return null;
        }

        return (string) $value;
    }
}
