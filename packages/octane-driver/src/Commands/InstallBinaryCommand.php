<?php

declare(strict_types=1);

namespace Rearm\Octane\Commands;

use Rearm\Octane\BinaryInstaller;
use Illuminate\Console\Command;
use Symfony\Component\Console\Attribute\AsCommand;
use Throwable;

#[AsCommand(name: 'rearm:install-binary')]
class InstallBinaryCommand extends Command
{
    /**
     * @var string
     */
    public $signature = 'rearm:install-binary
                    {--version= : Release tag (default: latest)}
                    {--dir= : Directory to place rearm-server (default: base_path/bin)}
                    {--force : Overwrite an existing binary}
                    {--repo=k1roshio/rearm-php : GitHub repo hosting releases}';

    /**
     * @var string
     */
    public $description = 'Download a prebuilt rearm-server binary from GitHub Releases';

    public function handle(): int
    {
        $dir = $this->option('dir');
        $destination = ($dir !== null && $dir !== '')
            ? rtrim((string) $dir, '/\\').DIRECTORY_SEPARATOR.'rearm-server'
            : BinaryInstaller::defaultDestination(base_path());

        if (is_file($destination) && ! $this->option('force')) {
            $this->components->warn("Binary already exists: {$destination}");
            $this->line('  Re-run with <fg=yellow>--force</> to replace it.');

            return 0;
        }

        $repo = (string) ($this->option('repo') ?: BinaryInstaller::DEFAULT_REPO);
        $token = $_ENV['GITHUB_TOKEN'] ?? $_SERVER['GITHUB_TOKEN'] ?? getenv('GITHUB_TOKEN') ?: null;
        $installer = new BinaryInstaller($repo, is_string($token) && $token !== '' ? $token : null);

        try {
            $target = $installer->detectTarget();
        } catch (Throwable $e) {
            $this->components->error($e->getMessage());

            return 1;
        }

        $version = $this->option('version');
        $version = is_string($version) && $version !== '' ? $version : null;

        $this->components->info('Downloading rearm-server…');
        $this->line("  <fg=gray>repo</>    {$repo}");
        $this->line("  <fg=gray>target</>  {$target}");
        $this->line('  <fg=gray>version</> '.($version ?? 'latest'));

        try {
            $result = $installer->install($destination, $version, $target);
        } catch (Throwable $e) {
            $this->components->error($e->getMessage());

            return 1;
        }

        $this->components->info("Installed {$result['version']} → {$result['path']}");
        $this->newLine();
        $this->line('Add to <fg=yellow>.env</>:');
        $this->line("  REARM_BIN={$result['path']}");
        $this->line('Or rely on auto-detect (project <fg=gray>bin/rearm-server</>).');
        $this->newLine();
        $this->line('Then: <fg=green>php artisan octane:start --server=rearm</>');

        return 0;
    }
}
