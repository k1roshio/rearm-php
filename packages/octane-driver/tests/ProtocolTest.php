<?php

declare(strict_types=1);

namespace Rearm\Octane\Tests;

use PHPUnit\Framework\TestCase;
use Rearm\Octane\Protocol;
use RuntimeException;

final class ProtocolTest extends TestCase
{
    private Protocol $protocol;

    protected function setUp(): void
    {
        $this->protocol = new Protocol;
    }

    public function test_request_roundtrip(): void
    {
        $framed = $this->protocol->encodeRequest([
            'method' => 'POST',
            'uri' => '/api/upload?x=1',
            'headers' => [
                'content-type' => 'application/json',
                'host' => 'example.com',
            ],
            'remote_addr' => '127.0.0.1:54321',
            'body' => '{"ok":true}',
            'form' => ['name' => 'rearm'],
            'files' => [[
                'field' => 'avatar',
                'path' => '/tmp/rearm-uploads/abc',
                'original_name' => 'photo.png',
                'mime' => 'image/png',
                'size' => 1234,
            ]],
        ]);

        [$len, $payload] = $this->split($framed);
        $this->assertSame(strlen($payload), $len);

        $decoded = $this->protocol->decodeRequest($payload);
        $this->assertSame('POST', $decoded['method']);
        $this->assertSame('/api/upload?x=1', $decoded['uri']);
        $this->assertSame('127.0.0.1:54321', $decoded['remote_addr']);
        $this->assertSame('{"ok":true}', $decoded['body']);
        $this->assertSame('rearm', $decoded['form']['name']);
        $this->assertSame('avatar', $decoded['files'][0]['field']);
        $this->assertSame(1234, $decoded['files'][0]['size']);
        $this->assertNull($decoded['control']);
    }

    public function test_response_roundtrip(): void
    {
        $framed = $this->protocol->encodeResponse([
            'status' => 200,
            'headers' => ['content-type' => 'text/plain'],
            'body' => 'hello',
        ]);

        [, $payload] = $this->split($framed);
        $decoded = $this->protocol->decodeResponse($payload);

        $this->assertSame(200, $decoded['status']);
        $this->assertSame('hello', $decoded['body']);
        $this->assertSame('text/plain', $decoded['headers']['content-type']);
    }

    public function test_binary_body_roundtrip(): void
    {
        $body = '';
        for ($i = 0; $i < 256; $i++) {
            $body .= chr($i);
        }

        $framed = $this->protocol->encodeRequest([
            'method' => 'PUT',
            'uri' => '/bin',
            'headers' => [],
            'remote_addr' => '127.0.0.1:1',
            'body' => $body,
        ]);

        [, $payload] = $this->split($framed);
        $decoded = $this->protocol->decodeRequest($payload);
        $this->assertSame($body, $decoded['body']);
    }

    public function test_read_frame_rejects_oversized_length(): void
    {
        $stream = fopen('php://memory', 'r+');
        $this->assertNotFalse($stream);
        fwrite($stream, pack('N', Protocol::MAX_FRAME_SIZE + 1));
        rewind($stream);

        $this->expectException(RuntimeException::class);
        $this->expectExceptionMessageMatches('/exceeds maximum/');
        $this->protocol->readFrame($stream);
    }

    public function test_decodes_rust_request_fixture(): void
    {
        $framed = $this->fixture('request.bin');
        [, $payload] = $this->split($framed);
        $decoded = $this->protocol->decodeRequest($payload);

        $this->assertSame('POST', $decoded['method']);
        $this->assertSame('/api/upload?x=1', $decoded['uri']);
        $this->assertSame('127.0.0.1:54321', $decoded['remote_addr']);
        $this->assertSame('{"ok":true}', $decoded['body']);
        $this->assertSame('rearm', $decoded['form']['name']);
        $this->assertSame('application/json', $decoded['headers']['content-type']);
        $this->assertSame('1', $decoded['headers']['x-rearm-test']);
        $this->assertSame('avatar', $decoded['files'][0]['field']);
        $this->assertSame(1234, $decoded['files'][0]['size']);
    }

    public function test_decodes_rust_response_fixture(): void
    {
        $framed = $this->fixture('response.bin');
        [, $payload] = $this->split($framed);
        $decoded = $this->protocol->decodeResponse($payload);

        $this->assertSame(200, $decoded['status']);
        $this->assertSame('hello', $decoded['body']);
        $this->assertSame('text/plain', $decoded['headers']['content-type']);
    }

    public function test_decodes_rust_shutdown_fixture(): void
    {
        $framed = $this->fixture('shutdown.bin');
        [, $payload] = $this->split($framed);
        $decoded = $this->protocol->decodeRequest($payload);

        $this->assertSame('shutdown', $decoded['control']);
    }

    /**
     * @return array{0: int, 1: string}
     */
    private function split(string $framed): array
    {
        $this->assertGreaterThanOrEqual(4, strlen($framed));
        $len = unpack('N', substr($framed, 0, 4))[1];
        $payload = substr($framed, 4);

        return [$len, $payload];
    }

    private function fixture(string $name): string
    {
        $path = __DIR__.'/fixtures/'.$name;
        $this->assertFileExists($path);

        return file_get_contents($path);
    }
}
