<?php

declare(strict_types=1);

namespace Rearm\Octane\Commands;

use Rearm\Octane\RearmServer;
use Illuminate\Console\Command;
use Symfony\Component\Console\Attribute\AsCommand;

#[AsCommand(name: 'octane:rearm')]
class StartRearmCommand extends Command
{
    /**
     * @var string
     */
    public $signature = 'octane:rearm
                    {--host= : The IP address the server should bind to}
                    {--port= : The port the server should be available on}
                    {--workers= : Warm (min) PHP workers}
                    {--workers-max= : Max PHP workers for elastic scale-up}
                    {--max-requests= : Restart a worker after this many requests (0 = never)}
                    {--bin= : Path to the rearm-server binary}';

    /**
     * @var string
     */
    public $description = 'Start the Rearm application server for Octane';

    public function handle(RearmServer $server): int
    {
        /** @var array<string, mixed> $config */
        $config = (array) config('octane.rearm', []);

        $binOpt = $this->option('bin');
        $bin = RearmServer::resolveBinary([
            'bin' => ($binOpt !== null && $binOpt !== '')
                ? (string) $binOpt
                : (($config['bin'] ?? null) ?: null),
        ]);
        if (! self::binaryLooksAvailable((string) $bin)) {
            $this->components->error(
                "Rearm binary not found: {$bin}. Run: php artisan rearm:install-binary"
            );

            return 1;
        }

        $httpAddr = $this->resolveHttpAddr($config);
        $workers = $this->option('workers');
        $workersMax = $this->option('workers-max');
        $maxRequests = $this->option('max-requests');

        $this->components->info('Starting Rearm server…');
        $this->line("  <fg=gray>listen</>  {$httpAddr}");
        $this->line('  <fg=gray>bin</>     '.$bin);

        return $server->start([
            'bin' => (string) $bin,
            'config' => $config,
            'http_addr' => $httpAddr,
            'workers' => $workers !== null && $workers !== '' ? $workers : null,
            'workers_max' => $workersMax !== null && $workersMax !== '' ? $workersMax : null,
            'max_requests' => $maxRequests !== null && $maxRequests !== '' ? $maxRequests : null,
            'app_root' => $config['app_root'] ?? base_path(),
            'worker_script' => $config['worker_script'] ?? RearmServer::workerScript(),
        ]);
    }

    /**
     * @param  array<string, mixed>  $config
     */
    private function resolveHttpAddr(array $config): string
    {
        $host = $this->option('host');
        $port = $this->option('port');

        if (($host === null || $host === '') && ($port === null || $port === '')) {
            return (string) ($config['http_addr'] ?? '0.0.0.0:8000');
        }

        $existing = (string) ($config['http_addr'] ?? '0.0.0.0:8000');
        $parts = explode(':', $existing);
        $defaultHost = $parts[0] !== '' ? $parts[0] : '0.0.0.0';
        $defaultPort = $parts[1] ?? '8000';

        $host = ($host !== null && $host !== '') ? $host : $defaultHost;
        $port = ($port !== null && $port !== '') ? $port : $defaultPort;

        return "{$host}:{$port}";
    }

    private static function binaryLooksAvailable(string $bin): bool
    {
        if ($bin === 'rearm-server') {
            // May be on PATH inside the container / host; let exec fail later if not.
            return true;
        }

        return is_file($bin) && is_executable($bin);
    }
}
