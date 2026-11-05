<?php

declare(strict_types=1);

namespace Rearm\Octane;

use Illuminate\Foundation\Application;
use Illuminate\Http\Request;
use Illuminate\Http\UploadedFile;
use Laravel\Octane\ApplicationFactory;
use Laravel\Octane\ApplicationGateway;
use Laravel\Octane\CurrentApplication;
use Laravel\Octane\Events\WorkerStarting;
use RuntimeException;
use Symfony\Component\HttpFoundation\Response as SymfonyResponse;
use Throwable;

/**
 * Long-lived PHP worker: listens on a Unix socket, boots Laravel once, serves
 * length-prefixed MessagePack frames from the Rearm Rust server.
 *
 * Socket contract: PHP listens on REARM_SOCK; Rust connects and keeps the
 * connection (keep-alive). Frames: [u32 BE length][msgpack].
 */
final class Worker
{
    private Protocol $protocol;

    private ?Application $app = null;

    private bool $shouldStop = false;

    /**
     * Connection bags kept alive after fork so PDO/Redis __destruct does not
     * attempt I/O on inherited sockets (that hangs the child).
     *
     * @var list<mixed>
     */
    private static array $orphanedAfterFork = [];

    public function __construct(
        private readonly string $appRoot,
        /** Mutable: forked children rebind to a new UDS path. */
        private string $socketPath,
        ?Protocol $protocol = null,
    ) {
        $this->protocol = $protocol ?? new Protocol;
    }

    /**
     * Boot Laravel (once) and serve until shutdown control or socket close.
     */
    public function run(): int
    {
        ignore_user_abort(true);
        set_time_limit(0);

        $this->bootApplication();
        $this->serve();

        return 0;
    }

    /**
     * Same accept/read/write loop without assuming Laravel is already booted.
     * Used by echo-worker after injecting a custom request handler via subclassing
     * is not needed — prefer {@see runWithHandler()}.
     *
     * @param  callable(array): array{status: int, headers: array<string, string>, body: string}  $handler
     */
    public function runWithHandler(callable $handler): int
    {
        $server = $this->listen();

        try {
            while (! $this->shouldStop) {
                $connection = @stream_socket_accept($server, -1);
                if ($connection === false) {
                    continue;
                }

                stream_set_blocking($connection, true);
                stream_set_timeout($connection, 60);
                stream_set_write_buffer($connection, 0);

                try {
                    $this->serveConnection($connection, $server, $handler);
                } finally {
                    if (is_resource($connection)) {
                        fclose($connection);
                    }
                }
            }
        } finally {
            if (is_resource($server)) {
                fclose($server);
            }
            $this->unlinkSocket();
        }

        return 0;
    }

    private function bootApplication(): void
    {
        $basePath = realpath($this->appRoot) ?: $this->appRoot;

        if (! is_dir($basePath)) {
            throw new RuntimeException("REARM_APP_ROOT is not a directory: {$this->appRoot}");
        }

        $_ENV['LARAVEL_OCTANE'] = '1';
        $_SERVER['LARAVEL_OCTANE'] = '1';
        putenv('LARAVEL_OCTANE=1');

        if (class_exists(ApplicationFactory::class)) {
            $this->app = (new ApplicationFactory($basePath))->createApplication();
        } else {
            $bootstrap = $basePath.DIRECTORY_SEPARATOR.'bootstrap'.DIRECTORY_SEPARATOR.'app.php';
            if (! is_file($bootstrap)) {
                throw new RuntimeException("Laravel bootstrap not found at {$bootstrap}");
            }

            $app = require $bootstrap;
            if (! $app instanceof Application) {
                throw new RuntimeException('bootstrap/app.php must return an Application instance.');
            }

            if (method_exists($app, 'make')) {
                $kernel = $app->make(\Illuminate\Contracts\Console\Kernel::class);
                $kernel->bootstrap();
            }

            $this->app = $app;
        }

        $this->afterBoot($this->app);
    }

    /**
     * Match Octane worker boot: fire WorkerStarting and warm the default PDO.
     * Donor skips DB warm so forked children inherit no live sockets.
     */
    private function afterBoot(Application $app): void
    {
        if (class_exists(WorkerStarting::class) && $app->bound('events')) {
            $app['events']->dispatch(new WorkerStarting($app));
        }

        if ($this->isDonorProcess()) {
            return;
        }

        // Keep one live DB connection per worker process (pairs with disabling
        // DisconnectFromDatabases in config/octane.php).
        try {
            if ($app->bound('db')) {
                $app->make('db')->connection()->getPdo();
            }
        } catch (Throwable $e) {
            fwrite(STDERR, '[rearm-worker] db warm failed: '.$e->getMessage()."\n");
        }
    }

    private function isDonorProcess(): bool
    {
        $role = getenv('REARM_WORKER_ROLE') ?: ($_SERVER['REARM_WORKER_ROLE'] ?? '');
        if (is_string($role) && strcasecmp($role, 'donor') === 0) {
            return true;
        }

        return str_contains($this->socketPath, 'donor-');
    }

    private function serve(): void
    {
        $this->runWithHandler(fn (array $request): array => $this->handleLaravelRequest($request));
    }

    /**
     * @param  resource  $server
     * @return resource
     */
    private function listen()
    {
        $this->unlinkSocket();

        $dir = dirname($this->socketPath);
        if (! is_dir($dir) && ! mkdir($dir, 0775, true) && ! is_dir($dir)) {
            throw new RuntimeException("Unable to create socket directory: {$dir}");
        }

        $server = @stream_socket_server(
            'unix://'.$this->socketPath,
            $errno,
            $errstr,
        );

        if ($server === false) {
            throw new RuntimeException("Failed to listen on {$this->socketPath}: [{$errno}] {$errstr}");
        }

        chmod($this->socketPath, 0666);

        return $server;
    }

    /**
     * @param  resource  $connection
     * @param  resource  $server
     * @param  callable(array): array{status: int, headers: array<string, string>, body: string}  $handler
     */
    private function serveConnection($connection, $server, callable $handler): void
    {
        while (! $this->shouldStop) {
            try {
                $payload = $this->protocol->readFrame($connection);
            } catch (Throwable) {
                break;
            }

            $request = $this->protocol->decodeRequest($payload);
            $control = $request['control'] ?? null;
            $shutdown = $control === 'shutdown';

            if ($shutdown && $request['method'] === '' && $request['uri'] === '') {
                $this->shouldStop = true;
                break;
            }

            if ($control === 'fork') {
                $forked = $this->handleForkControl($request, $server, $connection);
                if ($forked === null) {
                    // Child process: parent connection/server already closed; never return.
                    exit(0);
                }
                $response = $forked;
            } else {
                try {
                    $response = $handler($request);
                } catch (Throwable $e) {
                    $response = [
                        'status' => 500,
                        'headers' => ['content-type' => 'text/plain; charset=UTF-8'],
                        'body' => 'Internal Server Error',
                    ];
                    fwrite(STDERR, '[rearm-worker] '.$e->getMessage()."\n".$e->getTraceAsString()."\n");
                }
            }

            try {
                $this->protocol->writeFrame($connection, $this->protocol->encodeResponse($response));
            } catch (Throwable $e) {
                fwrite(STDERR, '[rearm-worker] write failed: '.$e->getMessage()."\n");
                break;
            }

            if ($shutdown) {
                $this->shouldStop = true;
                break;
            }
        }
    }

    /**
     * Donor/control path: fork a warm child listening on a new UDS.
     *
     * @param  array{headers?: array<string, string>}  $request
     * @param  resource  $server
     * @param  resource  $connection
     * @return array{status: int, headers: array<string, string>, body: string}|null
     *         Parent returns a response frame; child returns null after taking over.
     */
    private function handleForkControl(array $request, $server, $connection): ?array
    {
        $headers = $request['headers'] ?? [];
        $sock = $this->headerValue($headers, 'rearm-fork-sock');
        $workerId = $this->headerValue($headers, 'rearm-fork-id');

        if ($sock === '' || ! function_exists('pcntl_fork')) {
            return [
                'status' => 500,
                'headers' => ['content-type' => 'application/json'],
                'body' => json_encode([
                    'ok' => false,
                    'error' => $sock === '' ? 'missing rearm-fork-sock' : 'pcntl_fork unavailable',
                ], JSON_THROW_ON_ERROR),
            ];
        }

        // Snapshot donor UDS FDs before fork — child must drop them (listen race).
        $donorSockPath = $this->socketPath;
        $inheritedFds = $this->unixSocketFdsForPath($donorSockPath);

        $pid = pcntl_fork();
        if ($pid === -1) {
            return [
                'status' => 500,
                'headers' => ['content-type' => 'application/json'],
                'body' => json_encode(['ok' => false, 'error' => 'pcntl_fork failed'], JSON_THROW_ON_ERROR),
            ];
        }

        if ($pid === 0) {
            // Child: hard-drop donor FDs, rebind identity, listen on $sock.
            try {
                fwrite(STDERR, '[rearm-worker] child enter pid='.getmypid()." sock={$sock}\n");
                fflush(STDERR);

                // 1) Abortive close of inherited donor streams (no PHP fclose flush).
                $this->detachInheritedStream($connection);
                $this->detachInheritedStream($server);
                // Keep refs so destructors do not re-enter closed wrappers.
                self::$orphanedAfterFork[] = $connection;
                self::$orphanedAfterFork[] = $server;

                // 2) Drop any leftover FDs still bound to the donor UDS path.
                $this->hardCloseFds(array_merge(
                    $inheritedFds,
                    $this->unixSocketFdsForPath($donorSockPath),
                ));

                // 3) Worker identity (fork otherwise keeps donor environ).
                $this->socketPath = $sock;
                putenv('REARM_SOCK='.$sock);
                $_ENV['REARM_SOCK'] = $sock;
                $_SERVER['REARM_SOCK'] = $sock;
                putenv('REARM_WORKER_ROLE=worker');
                $_ENV['REARM_WORKER_ROLE'] = 'worker';
                $_SERVER['REARM_WORKER_ROLE'] = 'worker';
                if ($workerId !== '') {
                    putenv('REARM_WORKER_ID='.$workerId);
                    $_ENV['REARM_WORKER_ID'] = $workerId;
                    $_SERVER['REARM_WORKER_ID'] = $workerId;
                }

                $this->resetConnectionsAfterFork();
                fwrite(STDERR, '[rearm-worker] forked child ready sock='.$sock." id={$workerId}\n");
                fflush(STDERR);

                $this->serveAsForkedChild();
            } catch (Throwable $e) {
                fwrite(STDERR, '[rearm-worker] forked child failed: '.$e->getMessage()."\n".$e->getTraceAsString()."\n");
                fflush(STDERR);
                exit(1);
            }

            return null;
        }

        fwrite(STDERR, "[rearm-worker] donor forked child pid={$pid} sock={$sock}\n");
        fflush(STDERR);

        return [
            'status' => 200,
            'headers' => ['content-type' => 'application/json'],
            'body' => json_encode(['ok' => true, 'pid' => $pid], JSON_THROW_ON_ERROR),
        ];
    }

    /**
     * @param  array<string, string>  $headers
     */
    private function headerValue(array $headers, string $name): string
    {
        foreach ($headers as $key => $value) {
            if (strcasecmp((string) $key, $name) === 0) {
                return trim((string) $value);
            }
        }

        return '';
    }

    /**
     * Abortive close of an inherited stream. Plain fclose() flushes and hangs
     * after pcntl_fork; socket_close + SO_LINGER=0 drops the FD immediately.
     *
     * @param  resource|null  $stream
     */
    private function detachInheritedStream(mixed $stream): void
    {
        if (! is_resource($stream)) {
            return;
        }

        if (! function_exists('socket_import_stream') || ! function_exists('socket_close')) {
            return;
        }

        $socket = @socket_import_stream($stream);
        if ($socket === false) {
            return;
        }

        if (defined('SO_LINGER') && defined('SOL_SOCKET')) {
            @socket_set_option($socket, SOL_SOCKET, SO_LINGER, [
                'l_onoff' => 1,
                'l_linger' => 0,
            ]);
        }
        @socket_close($socket);
    }

    /**
     * FDs in this process whose Unix path is $path (listen and accepted).
     *
     * @return list<int>
     */
    private function unixSocketFdsForPath(string $path): array
    {
        if ($path === '' || ! is_dir('/proc/self/fd')) {
            return [];
        }

        $inodes = [];
        $unix = @file('/proc/net/unix');
        if (is_array($unix)) {
            foreach ($unix as $i => $line) {
                if ($i === 0) {
                    continue;
                }
                $parts = preg_split('/\s+/', trim($line));
                if ($parts === false || count($parts) < 8) {
                    continue;
                }
                if (($parts[7] ?? '') === $path) {
                    $inodes[$parts[6]] = true;
                }
            }
        }

        if ($inodes === []) {
            return [];
        }

        $fds = [];
        foreach (scandir('/proc/self/fd') ?: [] as $name) {
            if (! ctype_digit($name)) {
                continue;
            }
            $fd = (int) $name;
            if ($fd < 3) {
                continue;
            }
            $target = @readlink('/proc/self/fd/'.$name);
            if (! is_string($target) || ! str_starts_with($target, 'socket:[')) {
                continue;
            }
            if (preg_match('/socket:\[(\d+)\]/', $target, $m) !== 1) {
                continue;
            }
            if (isset($inodes[$m[1]])) {
                $fds[] = $fd;
            }
        }

        return array_values(array_unique($fds));
    }

    /**
     * libc close() — safe after fork. Never use PHP stream fclose here.
     *
     * @param  list<int>  $fds
     */
    private function hardCloseFds(array $fds): void
    {
        $fds = array_values(array_unique(array_filter(
            $fds,
            static fn (int $fd): bool => $fd >= 3,
        )));
        if ($fds === []) {
            return;
        }

        if (! extension_loaded('ffi') || ! class_exists(\FFI::class)) {
            fwrite(STDERR, '[rearm-worker] ffi unavailable; inherited FDs may leak: '.implode(',', $fds)."\n");
            fflush(STDERR);

            return;
        }

        try {
            /** @var \FFI&object{close: callable(int): int} $libc */
            $libc = \FFI::cdef('int close(int fd);', 'libc.so.6');
            foreach ($fds as $fd) {
                $libc->close($fd);
            }
        } catch (Throwable $e) {
            fwrite(STDERR, '[rearm-worker] hardCloseFds: '.$e->getMessage()."\n");
            fflush(STDERR);
        }
    }

    /**
     * Drop cached DB/Redis handles without I/O. Calling disconnect() or
     * allowing PDO/Redis __destruct after fork hangs on inherited sockets.
     * Orphan the old bags so destructors never run in the child.
     */
    private function resetConnectionsAfterFork(): void
    {
        if ($this->app === null) {
            return;
        }

        try {
            if ($this->app->bound('db')) {
                $manager = $this->app->make('db');
                $prop = new \ReflectionProperty($manager, 'connections');
                self::$orphanedAfterFork[] = $prop->getValue($manager);
                $prop->setValue($manager, []);
            }
        } catch (Throwable $e) {
            fwrite(STDERR, '[rearm-worker] post-fork db reset: '.$e->getMessage()."\n");
        }

        try {
            if ($this->app->bound('redis')) {
                $factory = $this->app->make('redis');
                $prop = new \ReflectionProperty($factory, 'connections');
                self::$orphanedAfterFork[] = $prop->getValue($factory);
                $prop->setValue($factory, []);
            }
        } catch (Throwable $e) {
            fwrite(STDERR, '[rearm-worker] post-fork redis reset: '.$e->getMessage()."\n");
        }
    }

    /**
     * Child-only accept loop on the new socket path.
     */
    private function serveAsForkedChild(): void
    {
        $this->shouldStop = false;
        $server = $this->listen();

        try {
            while (! $this->shouldStop) {
                $connection = @stream_socket_accept($server, -1);
                if ($connection === false) {
                    continue;
                }

                stream_set_blocking($connection, true);
                stream_set_timeout($connection, 60);
                stream_set_write_buffer($connection, 0);

                try {
                    $this->serveConnection(
                        $connection,
                        $server,
                        fn (array $request): array => $this->handleLaravelRequest($request),
                    );
                } finally {
                    if (is_resource($connection)) {
                        fclose($connection);
                    }
                }
            }
        } finally {
            if (is_resource($server)) {
                fclose($server);
            }
            $this->unlinkSocket();
        }
    }

    /**
     * @param  array{
     *     method: string,
     *     uri: string,
     *     headers: array<string, string>,
     *     remote_addr: string,
     *     body: string,
     *     form: array<string, string>|null,
     *     files: list<array{field: string, path: string, original_name: string, mime: string, size: int}>|null,
     *     control: string|null
     * }  $frame
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    private function handleLaravelRequest(array $frame): array
    {
        if ($this->app === null) {
            throw new RuntimeException('Application has not been booted.');
        }

        $request = $this->createRequest($frame);

        if (class_exists(ApplicationGateway::class) && class_exists(CurrentApplication::class)) {
            return $this->handleViaOctane($request);
        }

        return $this->handleViaApplication($request);
    }

    /**
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    private function handleViaOctane(Request $request): array
    {
        /** @var Application $app */
        $app = $this->app;

        // Same sandbox model as Laravel\Octane\Worker — clone is required for
        // request isolation; we keep the root app (and its PDO) intact.
        CurrentApplication::set($sandbox = clone $app);
        $gateway = new ApplicationGateway($app, $sandbox);

        try {
            // JSON/API bench paths do not echo; skip ob_* unless something leaks.
            $response = $gateway->handle($request);
            $gateway->terminate($request, $response);

            return $this->packSymfonyResponse($response);
        } finally {
            if (method_exists($sandbox, 'flush')) {
                $sandbox->flush();
            }

            // Matches Laravel\Octane\Worker: engines live on the root container.
            if ($app->bound('view.engine.resolver')) {
                $resolver = $app->make('view.engine.resolver');
                if (method_exists($resolver, 'forget')) {
                    $resolver->forget('blade');
                    $resolver->forget('php');
                }
            }

            CurrentApplication::set($app);
            unset($gateway, $sandbox, $request, $response);
        }
    }

    /**
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    private function handleViaApplication(Request $request): array
    {
        /** @var Application $app */
        $app = $this->app;

        try {
            if (method_exists($app, 'handleRequest')) {
                $response = $app->handleRequest($request);
            } else {
                $kernel = $app->make(\Illuminate\Contracts\Http\Kernel::class);
                $response = $kernel->handle($request);
                $kernel->terminate($request, $response);
            }

            return $this->packSymfonyResponse($response);
        } finally {
            if (method_exists($app, 'forgetInstance')) {
                $app->forgetInstance('request');
            }
        }
    }

    /**
     * @param  array{
     *     method: string,
     *     uri: string,
     *     headers: array<string, string>,
     *     remote_addr: string,
     *     body: string,
     *     form: array<string, string>|null,
     *     files: list<array{field: string, path: string, original_name: string, mime: string, size: int}>|null,
     *     control: string|null
     * }  $frame
     */
    private function createRequest(array $frame): Request
    {
        $uri = $frame['uri'] !== '' ? $frame['uri'] : '/';
        $method = strtoupper($frame['method'] !== '' ? $frame['method'] : 'GET');
        $headers = $frame['headers'];
        $parameters = $frame['form'] ?? [];
        $cookies = $this->parseCookies($headers['cookie'] ?? $headers['Cookie'] ?? null);
        $files = $this->createUploadedFiles($frame['files'] ?? []);
        $server = $this->buildServerBag($method, $uri, $headers, $frame['remote_addr']);
        $content = $frame['body'];

        $request = Request::create($uri, $method, $parameters, $cookies, $files, $server, $content);

        // Ensure uploaded files are visible as native Laravel uploads (test mode).
        if ($files !== []) {
            $request->files->replace($files);
        }

        return $request;
    }

    /**
     * @param  list<array{field: string, path: string, original_name: string, mime: string, size: int}>|null  $entries
     * @return array<string, UploadedFile|array<int, UploadedFile>>
     */
    private function createUploadedFiles(?array $entries): array
    {
        if ($entries === null || $entries === []) {
            return [];
        }

        $files = [];

        foreach ($entries as $entry) {
            $field = $entry['field'];
            $upload = new UploadedFile(
                $entry['path'],
                $entry['original_name'] !== '' ? $entry['original_name'] : basename($entry['path']),
                $entry['mime'] !== '' ? $entry['mime'] : null,
                UPLOAD_ERR_OK,
                true,
            );

            if (! array_key_exists($field, $files)) {
                $files[$field] = $upload;
                continue;
            }

            if (! is_array($files[$field])) {
                $files[$field] = [$files[$field]];
            }

            $files[$field][] = $upload;
        }

        return $files;
    }

    /**
     * @param  array<string, string>  $headers
     * @return array<string, mixed>
     */
    private function buildServerBag(string $method, string $uri, array $headers, string $remoteAddr): array
    {
        $parts = parse_url($uri) ?: [];
        $query = $parts['query'] ?? '';
        $path = $parts['path'] ?? '/';

        [$remoteIp, $remotePort] = $this->splitRemoteAddr($remoteAddr);

        $server = [
            'REQUEST_METHOD' => $method,
            'REQUEST_URI' => $uri,
            'QUERY_STRING' => $query,
            'SERVER_PROTOCOL' => 'HTTP/1.1',
            'SERVER_SOFTWARE' => 'Rearm',
            'SCRIPT_NAME' => '/index.php',
            'SCRIPT_FILENAME' => $this->appRoot.DIRECTORY_SEPARATOR.'public'.DIRECTORY_SEPARATOR.'index.php',
            'DOCUMENT_ROOT' => $this->appRoot.DIRECTORY_SEPARATOR.'public',
            'REMOTE_ADDR' => $remoteIp,
            'REMOTE_PORT' => $remotePort,
            'SERVER_ADDR' => '127.0.0.1',
            'SERVER_PORT' => '80',
            'HTTPS' => 'off',
            'PATH_INFO' => '',
            'PHP_SELF' => '/index.php'.$path,
        ];

        foreach ($headers as $name => $value) {
            $upper = strtoupper(str_replace('-', '_', $name));
            $server['HTTP_'.$upper] = $value;

            if ($upper === 'CONTENT_TYPE') {
                $server['CONTENT_TYPE'] = $value;
            }
            if ($upper === 'CONTENT_LENGTH') {
                $server['CONTENT_LENGTH'] = $value;
            }
            if ($upper === 'HOST') {
                $host = $value;
                if (str_contains($host, ':')) {
                    [$h, $p] = explode(':', $host, 2);
                    $server['SERVER_NAME'] = $h;
                    $server['SERVER_PORT'] = $p;
                } else {
                    $server['SERVER_NAME'] = $host;
                }
            }
            if ($upper === 'X_FORWARDED_PROTO' && strtolower($value) === 'https') {
                $server['HTTPS'] = 'on';
            }
        }

        return $server;
    }

    /**
     * @return array{0: string, 1: string}
     */
    private function splitRemoteAddr(string $remoteAddr): array
    {
        if ($remoteAddr === '') {
            return ['127.0.0.1', '0'];
        }

        if (str_starts_with($remoteAddr, '[') && str_contains($remoteAddr, ']:')) {
            $pos = strrpos($remoteAddr, ']:');
            return [substr($remoteAddr, 1, $pos - 1), substr($remoteAddr, $pos + 2)];
        }

        if (substr_count($remoteAddr, ':') === 1) {
            return explode(':', $remoteAddr, 2);
        }

        return [$remoteAddr, '0'];
    }

    /**
     * @return array<string, string>
     */
    private function parseCookies(?string $header): array
    {
        if ($header === null || $header === '') {
            return [];
        }

        $cookies = [];
        foreach (explode(';', $header) as $part) {
            $part = trim($part);
            if ($part === '' || ! str_contains($part, '=')) {
                continue;
            }
            [$name, $value] = explode('=', $part, 2);
            $cookies[trim($name)] = urldecode(trim($value));
        }

        return $cookies;
    }

    /**
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    private function packSymfonyResponse(SymfonyResponse $response, string $bufferedOutput = ''): array
    {
        $headers = [];

        foreach ($response->headers->allPreserveCaseWithoutCookies() as $name => $values) {
            $headers[$name] = implode(', ', $values);
        }

        foreach ($response->headers->getCookies() as $cookie) {
            $cookieLine = (string) $cookie;
            if (isset($headers['Set-Cookie'])) {
                $headers['Set-Cookie'] .= ', '.$cookieLine;
            } else {
                $headers['Set-Cookie'] = $cookieLine;
            }
        }

        $body = $response->getContent();
        if ($body === false) {
            ob_start();
            $response->sendContent();
            $body = ob_get_clean() ?: '';
        }

        if ($bufferedOutput !== '') {
            $body = $bufferedOutput.$body;
        }

        return [
            'status' => $response->getStatusCode(),
            'headers' => $headers,
            'body' => (string) $body,
        ];
    }

    private function unlinkSocket(): void
    {
        if (is_file($this->socketPath) || is_link($this->socketPath)) {
            @unlink($this->socketPath);
        }
    }
}
