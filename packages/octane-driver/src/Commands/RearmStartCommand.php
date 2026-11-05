<?php

declare(strict_types=1);

namespace Rearm\Octane\Commands;

use Symfony\Component\Console\Attribute\AsCommand;

/**
 * Friendly alias: php artisan rearm:start → octane:rearm.
 */
#[AsCommand(name: 'rearm:start')]
class RearmStartCommand extends StartRearmCommand
{
    /**
     * @var string
     */
    public $signature = 'rearm:start
                    {--host= : The IP address the server should bind to}
                    {--port= : The port the server should be available on}
                    {--workers= : Warm (min) PHP workers}
                    {--workers-max= : Max PHP workers for elastic scale-up}
                    {--max-requests= : Restart a worker after this many requests (0 = never)}
                    {--bin= : Path to the rearm-server binary}';

    /**
     * @var string
     */
    public $description = 'Start the Rearm application server (alias of octane:rearm)';
}
