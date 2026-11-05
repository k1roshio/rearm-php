<?php

declare(strict_types=1);

namespace Rearm\Octane;

use MessagePack\Packer;
use MessagePack\Type\Bin;
use MessagePack\Type\Map;
use MessagePack\BufferUnpacker;
use RuntimeException;

/**
 * Length-prefixed MessagePack codec matching crates/rearm-protocol.
 *
 * Wire layout: [u32 BE length][msgpack body]
 *
 * Prefers the C `msgpack` extension when loaded; falls back to rybakit/msgpack.
 */
final class Protocol
{
    public const MAX_FRAME_SIZE = 64 * 1024 * 1024;

    private ?Packer $packer = null;

    private readonly bool $useExt;

    public function __construct(?Packer $packer = null)
    {
        $this->useExt = $packer === null && extension_loaded('msgpack') && function_exists('msgpack_pack');
        if (! $this->useExt) {
            $this->packer = $packer ?? new Packer;
        }
    }

    /**
     * @param  array{
     *     method?: string,
     *     uri?: string,
     *     headers?: array<string, string>,
     *     remote_addr?: string,
     *     body?: string,
     *     form?: array<string, string>|null,
     *     files?: list<array{field: string, path: string, original_name: string, mime: string, size: int|string}>|null,
     *     control?: string|null
     * }  $request
     */
    public function encodeRequest(array $request): string
    {
        $map = [
            'method' => (string) ($request['method'] ?? ''),
            'uri' => (string) ($request['uri'] ?? ''),
            'headers' => $this->stringMap($request['headers'] ?? []),
            'remote_addr' => (string) ($request['remote_addr'] ?? ''),
            'body' => (string) ($request['body'] ?? ''),
        ];

        if (array_key_exists('form', $request) && $request['form'] !== null) {
            $map['form'] = $this->stringMap($request['form']);
        }

        if (array_key_exists('files', $request) && $request['files'] !== null) {
            $files = [];
            foreach ($request['files'] as $file) {
                $files[] = [
                    'field' => (string) ($file['field'] ?? ''),
                    'path' => (string) ($file['path'] ?? ''),
                    'original_name' => (string) ($file['original_name'] ?? ''),
                    'mime' => (string) ($file['mime'] ?? ''),
                    'size' => (int) ($file['size'] ?? 0),
                ];
            }
            $map['files'] = $files;
        }

        if (array_key_exists('control', $request) && $request['control'] !== null) {
            $map['control'] = (string) $request['control'];
        }

        return $this->frame($map);
    }

    /**
     * @param  array{status: int, headers?: array<string, string>, body?: string}  $response
     */
    public function encodeResponse(array $response): string
    {
        return $this->frame([
            'status' => (int) ($response['status'] ?? 200),
            'headers' => $this->stringMap($response['headers'] ?? []),
            'body' => (string) ($response['body'] ?? ''),
        ]);
    }

    /**
     * Decode a MessagePack payload (without the 4-byte length prefix).
     *
     * @return array<string, mixed>
     */
    public function decode(string $payload): array
    {
        if ($this->useExt) {
            $value = msgpack_unpack($payload);
        } else {
            $unpacker = new BufferUnpacker;
            $unpacker->reset($payload);
            $value = $unpacker->unpack();
        }

        if (! is_array($value)) {
            throw new RuntimeException('Protocol frame must decode to a map.');
        }

        return $value;
    }

    /**
     * @return array{
     *     method: string,
     *     uri: string,
     *     headers: array<string, string>,
     *     remote_addr: string,
     *     body: string,
     *     form: array<string, string>|null,
     *     files: list<array{field: string, path: string, original_name: string, mime: string, size: int}>|null,
     *     control: string|null
     * }
     */
    public function decodeRequest(string $payload): array
    {
        $raw = $this->decode($payload);

        $files = null;
        if (isset($raw['files']) && is_array($raw['files'])) {
            $files = [];
            foreach ($raw['files'] as $file) {
                if (! is_array($file)) {
                    continue;
                }
                $files[] = [
                    'field' => (string) ($file['field'] ?? ''),
                    'path' => (string) ($file['path'] ?? ''),
                    'original_name' => (string) ($file['original_name'] ?? ''),
                    'mime' => (string) ($file['mime'] ?? ''),
                    'size' => (int) ($file['size'] ?? 0),
                ];
            }
        }

        $form = null;
        if (isset($raw['form']) && is_array($raw['form'])) {
            $form = $this->stringMap($raw['form']);
        }

        return [
            'method' => (string) ($raw['method'] ?? ''),
            'uri' => (string) ($raw['uri'] ?? ''),
            'headers' => $this->stringMap(is_array($raw['headers'] ?? null) ? $raw['headers'] : []),
            'remote_addr' => (string) ($raw['remote_addr'] ?? ''),
            'body' => $this->asBytes($raw['body'] ?? ''),
            'form' => $form,
            'files' => $files,
            'control' => isset($raw['control']) ? (string) $raw['control'] : null,
        ];
    }

    /**
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    public function decodeResponse(string $payload): array
    {
        $raw = $this->decode($payload);

        return [
            'status' => (int) ($raw['status'] ?? 500),
            'headers' => $this->stringMap(is_array($raw['headers'] ?? null) ? $raw['headers'] : []),
            'body' => $this->asBytes($raw['body'] ?? ''),
        ];
    }

    /**
     * Read one length-prefixed frame payload from a stream.
     *
     * @param  resource  $stream
     */
    public function readFrame($stream): string
    {
        $header = $this->readExact($stream, 4);
        $length = unpack('N', $header)[1];

        if ($length > self::MAX_FRAME_SIZE) {
            throw new RuntimeException("Frame length {$length} exceeds maximum ".self::MAX_FRAME_SIZE);
        }

        if ($length === 0) {
            return '';
        }

        return $this->readExact($stream, $length);
    }

    /**
     * Write a pre-framed buffer (already includes length prefix) to a stream.
     *
     * @param  resource  $stream
     */
    public function writeFrame($stream, string $framed): void
    {
        $written = 0;
        $length = strlen($framed);

        while ($written < $length) {
            $chunk = $written === 0 ? $framed : substr($framed, $written);
            $n = @fwrite($stream, $chunk);

            if ($n === false || $n === 0) {
                $meta = stream_get_meta_data($stream);
                if (! empty($meta['timed_out'])) {
                    throw new RuntimeException('Timed out writing protocol frame to socket.');
                }
                if (! empty($meta['eof'])) {
                    throw new RuntimeException('Socket closed while writing protocol frame.');
                }

                $read = null;
                $write = [$stream];
                $except = null;
                $selected = @stream_select($read, $write, $except, 5);
                if ($selected === false || $selected === 0 || $write === []) {
                    throw new RuntimeException('Failed writing protocol frame to socket (peer not reading).');
                }

                continue;
            }

            $written += $n;
        }
    }

    /**
     * @param  array<string, mixed>  $map
     */
    private function frame(array $map): string
    {
        if ($this->useExt) {
            $payload = msgpack_pack($map);
        } else {
            $packed = [];
            foreach ($map as $key => $value) {
                if (is_array($value) && $this->isStringMap($value)) {
                    $packed[$key] = new Map($value);
                } elseif (is_string($value) && ($key === 'body')) {
                    $packed[$key] = new Bin($value);
                } else {
                    $packed[$key] = $value;
                }
            }
            if (isset($map['files']) && is_array($map['files'])) {
                $files = [];
                foreach ($map['files'] as $file) {
                    $files[] = is_array($file) ? new Map($file) : $file;
                }
                $packed['files'] = $files;
            }
            $payload = $this->packer->pack(new Map($packed));
        }

        $len = strlen($payload);

        if ($len > self::MAX_FRAME_SIZE) {
            throw new RuntimeException("Encoded frame length {$len} exceeds maximum ".self::MAX_FRAME_SIZE);
        }

        return pack('N', $len).$payload;
    }

    /**
     * @param  array<mixed, mixed>  $map
     */
    private function isStringMap(array $map): bool
    {
        foreach ($map as $key => $value) {
            if (! is_string($key) || is_array($value)) {
                return false;
            }
        }

        return true;
    }

    /**
     * @param  array<mixed, mixed>  $map
     * @return array<string, string>
     */
    private function stringMap(array $map): array
    {
        $out = [];
        foreach ($map as $key => $value) {
            if (is_array($value)) {
                $value = implode(', ', array_map(static fn ($v) => (string) $v, $value));
            }
            $out[(string) $key] = (string) $value;
        }

        return $out;
    }

    private function asBytes(mixed $value): string
    {
        if (is_string($value)) {
            return $value;
        }

        if (is_array($value)) {
            return pack('C*', ...array_map(static fn ($b) => (int) $b, $value));
        }

        return (string) $value;
    }

    /**
     * @param  resource  $stream
     */
    private function readExact($stream, int $n): string
    {
        $buf = '';

        while (strlen($buf) < $n) {
            $chunk = fread($stream, $n - strlen($buf));
            if ($chunk === false || $chunk === '') {
                throw new RuntimeException('Socket closed while reading protocol frame.');
            }
            $buf .= $chunk;
        }

        return $buf;
    }
}
