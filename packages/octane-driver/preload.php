<?php

declare(strict_types=1);

/**
 * Opcache preload for Rearm PHP workers.
 * Loads Composer autoload (+ Octane/Laravel classes when present) once per
 * PHP process image so scale-up / warm boots pay less compile cost.
 */
$candidates = [
    dirname(__DIR__).'/../apps/bench/vendor/autoload.php',
    '/var/www/apps/bench/vendor/autoload.php',
    dirname(__DIR__).'/vendor/autoload.php',
];

foreach ($candidates as $autoload) {
    if (is_file($autoload)) {
        require $autoload;
        break;
    }
}
