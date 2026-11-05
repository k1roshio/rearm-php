#!/usr/bin/env php
<?php

declare(strict_types=1);

use Rearm\Octane\RearmServer;

$autoloadCandidates = [];

$appRoot = getenv('REARM_APP_ROOT') ?: ($_SERVER['REARM_APP_ROOT'] ?? null);
if (is_string($appRoot) && $appRoot !== '') {
    $autoloadCandidates[] = rtrim($appRoot, '/\\').'/vendor/autoload.php';
}

$autoloadCandidates = array_merge($autoloadCandidates, [
    dirname(__DIR__).'/vendor/autoload.php',
    dirname(__DIR__, 3).'/vendor/autoload.php',
    dirname(__DIR__, 4).'/vendor/autoload.php',
]);

$autoload = null;
foreach ($autoloadCandidates as $candidate) {
    if (is_file($candidate)) {
        $autoload = $candidate;
        break;
    }
}

if ($autoload === null) {
    fwrite(STDERR, "rearm worker: Composer autoload not found.\n");
    exit(1);
}

require $autoload;

try {
    $worker = RearmServer::makeWorker();
    exit($worker->run());
} catch (Throwable $e) {
    fwrite(STDERR, '[rearm-worker] '.$e->getMessage()."\n".$e->getTraceAsString()."\n");
    exit(1);
}
