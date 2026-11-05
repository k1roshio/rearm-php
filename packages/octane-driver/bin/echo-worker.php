#!/usr/bin/env php
<?php

declare(strict_types=1);

/**
 * Minimal MessagePack Unix-socket worker that does NOT boot Laravel.
 * Returns a JSON pong for Rust integration testing.
 *
 * Env:
 *   REARM_SOCK  — Unix socket path (PHP listens; Rust connects)
 */

use Rearm\Octane\Protocol;
use Rearm\Octane\Worker;

$autoloadCandidates = [
    dirname(__DIR__).'/vendor/autoload.php',
    dirname(__DIR__, 3).'/vendor/autoload.php',
    dirname(__DIR__, 4).'/vendor/autoload.php',
];

$autoload = null;
foreach ($autoloadCandidates as $candidate) {
    if (is_file($candidate)) {
        $autoload = $candidate;
        break;
    }
}

if ($autoload === null) {
    fwrite(STDERR, "rearm echo-worker: Composer autoload not found. Run composer install in the package or app.\n");
    exit(1);
}

require $autoload;

$sock = getenv('REARM_SOCK') ?: ($_SERVER['REARM_SOCK'] ?? '');
if ($sock === '') {
    fwrite(STDERR, "rearm echo-worker: REARM_SOCK is required.\n");
    exit(1);
}

// appRoot is unused for echo handling but required by Worker constructor.
$worker = new Worker(sys_get_temp_dir(), $sock, new Protocol);

exit($worker->runWithHandler(static function (array $request): array {
    $body = json_encode([
        'pong' => true,
        'method' => $request['method'],
        'uri' => $request['uri'],
        'remote_addr' => $request['remote_addr'],
        'body_len' => strlen($request['body']),
        'form' => $request['form'],
        'files' => $request['files'] === null ? null : count($request['files']),
        'control' => $request['control'],
    ], JSON_THROW_ON_ERROR);

    return [
        'status' => 200,
        'headers' => [
            'content-type' => 'application/json; charset=UTF-8',
        ],
        'body' => $body,
    ];
}));
