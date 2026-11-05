<?php

declare(strict_types=1);

namespace Rearm\Octane;

use RuntimeException;

/**
 * Downloads a prebuilt {@code rearm-server} binary from GitHub Releases.
 */
final class BinaryInstaller
{
    public const DEFAULT_REPO = 'k1roshio/rearm-php';

    public function __construct(
        private readonly string $repo = self::DEFAULT_REPO,
        private readonly ?string $githubToken = null,
    ) {
    }

    /**
     * Detect the Rust target triple for the current host.
     *
     * @throws RuntimeException when the platform is unsupported
     */
    public function detectTarget(): string
    {
        $os = strtolower(PHP_OS_FAMILY);
        if ($os !== 'linux') {
            throw new RuntimeException(
                "Prebuilt rearm-server binaries are published for Linux only (got {$os}). ".
                'Build from source: cargo build --release -p rearm-server'
            );
        }

        $machine = strtolower(php_uname('m'));

        return match ($machine) {
            'x86_64', 'amd64' => 'x86_64-unknown-linux-gnu',
            'aarch64', 'arm64' => 'aarch64-unknown-linux-gnu',
            default => throw new RuntimeException(
                "Unsupported CPU architecture: {$machine}. Expected x86_64 or aarch64."
            ),
        };
    }

    /**
     * Download and extract the binary to {@see $destination} (file path).
     *
     * @param  string|null  $version  Tag like {@code v0.1.2}, or null for latest release
     * @return array{path: string, version: string, target: string, asset: string}
     */
    public function install(string $destination, ?string $version = null, ?string $target = null): array
    {
        $target ??= $this->detectTarget();
        $release = $version === null || $version === '' || $version === 'latest'
            ? $this->fetchJson($this->apiUrl('/releases/latest'))
            : $this->fetchJson($this->apiUrl('/releases/tags/'.rawurlencode($version)));

        $tag = (string) ($release['tag_name'] ?? '');
        if ($tag === '') {
            throw new RuntimeException('GitHub release response missing tag_name.');
        }

        $assetName = "rearm-server-{$target}.tar.gz";
        $assets = $release['assets'] ?? [];
        if (! is_array($assets)) {
            throw new RuntimeException('GitHub release has no assets.');
        }

        $asset = null;
        foreach ($assets as $item) {
            if (is_array($item) && ($item['name'] ?? '') === $assetName) {
                $asset = $item;
                break;
            }
        }

        if ($asset === null) {
            throw new RuntimeException(
                "Release {$tag} has no asset {$assetName}. ".
                'Publish a release with CI or build from source.'
            );
        }

        $url = (string) ($asset['browser_download_url'] ?? '');
        if ($url === '') {
            throw new RuntimeException("Asset {$assetName} has no download URL.");
        }

        $dir = dirname($destination);
        if (! is_dir($dir) && ! mkdir($dir, 0755, true) && ! is_dir($dir)) {
            throw new RuntimeException("Failed to create directory: {$dir}");
        }

        $tmpTar = tempnam(sys_get_temp_dir(), 'rearm-bin-');
        if ($tmpTar === false) {
            throw new RuntimeException('Failed to create temp file.');
        }
        $tmpTarGz = $tmpTar.'.tar.gz';
        @unlink($tmpTar);

        try {
            $this->download($url, $tmpTarGz);
            $this->extractBinary($tmpTarGz, $destination);
        } finally {
            @unlink($tmpTarGz);
        }

        if (! is_file($destination)) {
            throw new RuntimeException("Extracted binary missing at {$destination}");
        }

        chmod($destination, 0755);

        return [
            'path' => $destination,
            'version' => $tag,
            'target' => $target,
            'asset' => $assetName,
        ];
    }

    /**
     * Default install path inside a Laravel (or CWD) project.
     */
    public static function defaultDestination(?string $basePath = null): string
    {
        $base = $basePath
            ?? (function_exists('base_path') ? base_path() : getcwd());

        if ($base === false || $base === '') {
            $base = getcwd() ?: sys_get_temp_dir();
        }

        return rtrim((string) $base, '/\\').DIRECTORY_SEPARATOR.'bin'.DIRECTORY_SEPARATOR.'rearm-server';
    }

    private function apiUrl(string $suffix): string
    {
        return 'https://api.github.com/repos/'.$this->repo.$suffix;
    }

    /**
     * @return array<string, mixed>
     */
    private function fetchJson(string $url): array
    {
        $body = $this->httpGet($url, accept: 'application/vnd.github+json');
        $decoded = json_decode($body, true);
        if (! is_array($decoded)) {
            throw new RuntimeException("Invalid JSON from {$url}");
        }

        if (isset($decoded['message']) && ! isset($decoded['tag_name'])) {
            throw new RuntimeException('GitHub API: '.(string) $decoded['message']);
        }

        return $decoded;
    }

    private function download(string $url, string $destination): void
    {
        $data = $this->httpGet($url, accept: 'application/octet-stream');
        if (file_put_contents($destination, $data) === false) {
            throw new RuntimeException("Failed writing download to {$destination}");
        }
    }

    private function httpGet(string $url, string $accept): string
    {
        if (function_exists('curl_init')) {
            return $this->curlGet($url, $accept);
        }

        $headers = [
            "Accept: {$accept}",
            'User-Agent: k1roshio-octane-driver-rearm',
        ];
        if ($this->githubToken) {
            $headers[] = 'Authorization: Bearer '.$this->githubToken;
        }

        $ctx = stream_context_create([
            'http' => [
                'method' => 'GET',
                'header' => implode("\r\n", $headers),
                'follow_location' => 1,
                'timeout' => 120,
            ],
        ]);

        $body = @file_get_contents($url, false, $ctx);
        if ($body === false) {
            throw new RuntimeException("HTTP GET failed: {$url}");
        }

        return $body;
    }

    private function curlGet(string $url, string $accept): string
    {
        $ch = curl_init($url);
        if ($ch === false) {
            throw new RuntimeException('curl_init failed');
        }

        $headers = [
            "Accept: {$accept}",
            'User-Agent: k1roshio-octane-driver-rearm',
            'X-GitHub-Api-Version: 2022-11-28',
        ];
        if ($this->githubToken) {
            $headers[] = 'Authorization: Bearer '.$this->githubToken;
        }

        curl_setopt_array($ch, [
            CURLOPT_RETURNTRANSFER => true,
            CURLOPT_FOLLOWLOCATION => true,
            CURLOPT_TIMEOUT => 120,
            CURLOPT_HTTPHEADER => $headers,
        ]);

        $body = curl_exec($ch);
        $status = (int) curl_getinfo($ch, CURLINFO_HTTP_CODE);
        $err = curl_error($ch);
        curl_close($ch);

        if ($body === false) {
            throw new RuntimeException("HTTP GET failed: {$url} ({$err})");
        }

        if ($status >= 400) {
            throw new RuntimeException("HTTP {$status} from {$url}: ".$this->truncate((string) $body));
        }

        return (string) $body;
    }

    private function extractBinary(string $tarGz, string $destination): void
    {
        $extractDir = sys_get_temp_dir().'/rearm-extract-'.bin2hex(random_bytes(4));
        if (! mkdir($extractDir, 0755, true) && ! is_dir($extractDir)) {
            throw new RuntimeException("Failed to create extract dir: {$extractDir}");
        }

        try {
            $cmd = sprintf(
                'tar -xzf %s -C %s',
                escapeshellarg($tarGz),
                escapeshellarg($extractDir),
            );
            exec($cmd.' 2>&1', $output, $code);
            if ($code !== 0) {
                throw new RuntimeException('tar extract failed: '.implode("\n", $output));
            }

            $found = null;
            foreach (scandir($extractDir) ?: [] as $entry) {
                if ($entry === '.' || $entry === '..') {
                    continue;
                }
                $path = $extractDir.'/'.$entry;
                if (is_file($path)) {
                    $found = $path;
                    if ($entry === 'rearm-server') {
                        break;
                    }
                }
            }

            if ($found === null) {
                throw new RuntimeException('Archive did not contain rearm-server binary.');
            }

            if (! rename($found, $destination) && ! copy($found, $destination)) {
                throw new RuntimeException("Failed to place binary at {$destination}");
            }
        } finally {
            $this->rrmdir($extractDir);
        }
    }

    private function rrmdir(string $dir): void
    {
        if (! is_dir($dir)) {
            return;
        }
        foreach (scandir($dir) ?: [] as $entry) {
            if ($entry === '.' || $entry === '..') {
                continue;
            }
            $path = $dir.DIRECTORY_SEPARATOR.$entry;
            if (is_dir($path)) {
                $this->rrmdir($path);
            } else {
                @unlink($path);
            }
        }
        @rmdir($dir);
    }

    private function truncate(string $value, int $max = 200): string
    {
        if (strlen($value) <= $max) {
            return $value;
        }

        return substr($value, 0, $max).'…';
    }
}
