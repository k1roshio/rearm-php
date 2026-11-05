<?php

declare(strict_types=1);

namespace Rearm\Octane\Tests;

use PHPUnit\Framework\TestCase;
use Rearm\Octane\BinaryInstaller;
use RuntimeException;

final class BinaryInstallerTest extends TestCase
{
    public function test_detect_target_on_linux(): void
    {
        if (PHP_OS_FAMILY !== 'Linux') {
            $this->markTestSkipped('Linux only');
        }

        $target = (new BinaryInstaller)->detectTarget();
        $this->assertContains($target, [
            'x86_64-unknown-linux-gnu',
            'aarch64-unknown-linux-gnu',
        ]);
    }

    public function test_default_destination_ends_with_bin_rearm_server(): void
    {
        $path = BinaryInstaller::defaultDestination('/tmp/app');
        $this->assertSame('/tmp/app'.DIRECTORY_SEPARATOR.'bin'.DIRECTORY_SEPARATOR.'rearm-server', $path);
    }

    public function test_detect_target_rejects_non_linux(): void
    {
        if (PHP_OS_FAMILY === 'Linux') {
            $this->markTestSkipped('Need non-Linux host to assert rejection');
        }

        $this->expectException(RuntimeException::class);
        (new BinaryInstaller)->detectTarget();
    }
}
