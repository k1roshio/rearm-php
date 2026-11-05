<?php

declare(strict_types=1);

namespace Rearm\Octane\Commands;

use Laravel\Octane\Commands\StartCommand as OctaneStartCommand;
use Symfony\Component\Console\Attribute\AsCommand;

/**
 * Extends Octane's octane:start with a `rearm` server branch.
 *
 * Registered after Laravel\Octane so this command wins the `octane:start` name.
 */
#[AsCommand(name: 'octane:start')]
class StartCommand extends OctaneStartCommand
{
    /**
     * Handle the command.
     *
     * @return int
     */
    public function handle()
    {
        $server = $this->option('server') ?: config('octane.server');

        if ($server === 'rearm') {
            // Do not invent host/port defaults here — octane:rearm falls back to
            // config('octane.rearm.http_addr') when flags are omitted.
            $params = [];
            if ($this->option('host')) {
                $params['--host'] = $this->option('host');
            }
            if ($this->option('port')) {
                $params['--port'] = $this->option('port');
            }
            if ($this->option('workers')) {
                $params['--workers'] = $this->option('workers');
            }
            if ($this->option('max-requests')) {
                $params['--max-requests'] = $this->option('max-requests');
            }

            return $this->call('octane:rearm', $params);
        }

        return parent::handle();
    }
}
