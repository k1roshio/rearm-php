<?php

declare(strict_types=1);

namespace Rearm\Octane;

use Rearm\Octane\Commands\RearmStartCommand;
use Rearm\Octane\Commands\StartCommand;
use Rearm\Octane\Commands\StartRearmCommand;
use Illuminate\Support\ServiceProvider;

class RearmServiceProvider extends ServiceProvider
{
    public function register(): void
    {
        $this->mergeConfigFrom(__DIR__.'/../config/rearm.php', 'octane');

        $this->app->singleton(RearmServer::class, static fn () => new RearmServer);
    }

    public function boot(): void
    {
        if ($this->app->runningInConsole()) {
            $this->publishes([
                __DIR__.'/../config/rearm.php' => config_path('octane-rearm.php'),
            ], 'rearm-config');

            $this->commands([
                StartRearmCommand::class,
                RearmStartCommand::class,
            ]);

            // k1roshio/octane-driver-rearm is discovered before laravel/octane alphabetically,
            // so defer octane:start override until all providers have booted.
            $this->app->booted(function () {
                if ($this->app->runningInConsole()) {
                    $this->commands([StartCommand::class]);
                }
            });
        }
    }
}
