use clap::{Parser, ValueEnum};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

/// Response compression preference advertised by the CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompressMode {
    Brotli,
    Gzip,
    Off,
}

/// Rearm server command-line configuration.
#[derive(Debug, Clone, Parser)]
#[command(name = "rearm-server", about = "Rust application server for Laravel")]
pub struct Config {
    /// HTTP/1.1 listen address
    #[arg(long, env = "REARM_HTTP_ADDR", default_value = "0.0.0.0:8080")]
    pub listen: SocketAddr,

    /// Enable HTTP/3 (QUIC). Off by default; needs --cert/--key (or REARM_CERT/KEY).
    #[arg(long, env = "REARM_HTTP3", default_value_t = false)]
    pub http3: bool,

    /// HTTP/3 (QUIC) listen address (used only when --http3 / REARM_HTTP3=true)
    #[arg(long, env = "REARM_HTTP3_ADDR", default_value = "0.0.0.0:4433")]
    pub http3_listen: SocketAddr,

    /// TLS certificate (PEM) for HTTP/3
    #[arg(long, env = "REARM_CERT")]
    pub cert: Option<PathBuf>,

    /// TLS private key (PEM) for HTTP/3
    #[arg(long, env = "REARM_KEY")]
    pub key: Option<PathBuf>,

    /// Minimum (warm) persistent PHP workers
    #[arg(long, env = "REARM_WORKERS", default_value_t = 4)]
    pub workers: usize,

    /// Maximum PHP workers for elastic scale-up (0 = same as --workers)
    #[arg(long, env = "REARM_WORKERS_MAX", default_value_t = 0)]
    pub workers_max: usize,

    /// Scale up only when at least this many requests wait for a free worker
    #[arg(long, env = "REARM_SCALE_UP_WAITERS", default_value_t = 2)]
    pub scale_up_waiters: usize,

    /// Queue pressure must persist this many ms before a scale-up burst
    /// (avoids spawning PHP on one-shot fast spikes).
    #[arg(long, env = "REARM_SCALE_UP_QUEUE_MS", default_value_t = 25)]
    pub scale_up_queue_ms: u64,

    /// Minimum milliseconds between scale-up bursts
    #[arg(long, env = "REARM_SCALE_UP_COOLDOWN_MS", default_value_t = 250)]
    pub scale_up_cooldown_ms: u64,

    /// Max workers to spawn in one parallel scale-up burst
    #[arg(long, env = "REARM_SCALE_UP_BATCH", default_value_t = 4)]
    pub scale_up_batch: usize,

    /// Idle milliseconds before a worker above min is shut down
    #[arg(long, env = "REARM_SCALE_DOWN_IDLE_MS", default_value_t = 5_000)]
    pub scale_down_idle_ms: u64,

    /// After a scale-up burst, block scale-down for this many ms (anti-churn).
    #[arg(long, env = "REARM_SCALE_DOWN_GRACE_MS", default_value_t = 15_000)]
    pub scale_down_grace_ms: u64,

    /// How many idle workers to stop per monitor tick
    #[arg(long, env = "REARM_SCALE_DOWN_BATCH", default_value_t = 1)]
    pub scale_down_batch: usize,

    /// Restart a worker when its RSS exceeds this many MiB
    #[arg(long, env = "REARM_MEMORY_LIMIT_MB", default_value_t = 128)]
    pub memory_limit_mb: u64,

    /// Restart a worker after this many handled requests
    #[arg(long, env = "REARM_MAX_REQUESTS", default_value_t = 1000)]
    pub max_requests: u64,

    /// PHP binary path
    #[arg(long, env = "REARM_PHP_BIN", default_value = "php")]
    pub php_bin: PathBuf,

    /// Absolute path to the Rearm PHP worker script
    #[arg(
        long,
        env = "REARM_WORKER_SCRIPT",
        default_value = "/var/www/packages/octane-driver/bin/worker.php"
    )]
    pub worker_script: PathBuf,

    /// Laravel application root
    #[arg(long, env = "REARM_APP_ROOT", default_value = "/var/www/app")]
    pub app_root: PathBuf,

    /// Directory for streamed multipart uploads
    #[arg(long, env = "REARM_UPLOAD_DIR", default_value = "/var/www/uploads")]
    pub upload_dir: PathBuf,

    /// Preferred response compression
    #[arg(long, env = "REARM_COMPRESS", value_enum, default_value_t = CompressMode::Brotli)]
    pub compress: CompressMode,

    /// Directory for per-worker Unix domain sockets
    #[arg(long, env = "REARM_SOCK_DIR", default_value = "/tmp/rearm-socks")]
    pub sock_dir: PathBuf,

    /// Comma-separated path allowlist for the long worker pool.
    /// Empty = single shared pool (legacy behaviour).
    /// Patterns: exact `/io-slow` or prefix `/report*` / `/export/*`.
    #[arg(long, env = "REARM_LONG_PATHS", default_value = "")]
    pub long_paths: String,

    /// Warm workers in the long pool (only used when LONG_PATHS is non-empty)
    #[arg(long, env = "REARM_LONG_WORKERS", default_value_t = 2)]
    pub long_workers: usize,

    /// Max long-pool workers (0 = same as LONG_WORKERS)
    #[arg(long, env = "REARM_LONG_WORKERS_MAX", default_value_t = 0)]
    pub long_workers_max: usize,

    /// Max deferred client wait honoured from `X-Rearm-Defer-Ms` (ms).
    #[arg(long, env = "REARM_DEFER_MAX_MS", default_value_t = 300_000)]
    pub defer_max_ms: u64,

    /// Defer is applied in chunks so a long wait is "sleep again" without
    /// tearing down the HTTP connection (default 250ms per slice).
    #[arg(long, env = "REARM_DEFER_CHUNK_MS", default_value_t = 250)]
    pub defer_chunk_ms: u64,

    /// Scale-up via pcntl_fork from a shared warm PHP donor (CoW). Falls back
    /// to cold spawn when disabled or fork fails.
    #[arg(long, env = "REARM_FORK_SCALE", default_value_t = true)]
    pub fork_scale: bool,
}

impl Config {
    pub fn public_dir(&self) -> PathBuf {
        self.app_root.join("public")
    }

    /// Effective minimum warm workers (at least 1).
    pub fn workers_min(&self) -> usize {
        self.workers.max(1)
    }

    /// Effective maximum workers (at least min).
    pub fn workers_max_effective(&self) -> usize {
        let min = self.workers_min();
        if self.workers_max == 0 {
            min
        } else {
            self.workers_max.max(min)
        }
    }

    pub fn scale_up_cooldown(&self) -> Duration {
        Duration::from_millis(self.scale_up_cooldown_ms)
    }

    pub fn scale_up_queue(&self) -> Duration {
        Duration::from_millis(self.scale_up_queue_ms)
    }

    pub fn scale_down_idle(&self) -> Duration {
        Duration::from_millis(self.scale_down_idle_ms)
    }

    pub fn scale_down_grace(&self) -> Duration {
        Duration::from_millis(self.scale_down_grace_ms)
    }

    /// Parsed long-path allowlist; empty disables the split pool.
    pub fn long_paths_list(&self) -> Vec<String> {
        self.long_paths
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    pub fn long_pool_enabled(&self) -> bool {
        !self.long_paths_list().is_empty()
    }

    pub fn long_workers_min(&self) -> usize {
        self.long_workers.max(1)
    }

    pub fn long_workers_max_effective(&self) -> usize {
        let min = self.long_workers_min();
        if self.long_workers_max == 0 {
            min
        } else {
            self.long_workers_max.max(min)
        }
    }

    pub fn defer_chunk_ms_effective(&self) -> u64 {
        self.defer_chunk_ms.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_list_parses_and_trims() {
        let cfg = Config::try_parse_from([
            "rearm-server",
            "--long-paths",
            " /a , /b* , ",
            "--worker-script",
            "/tmp/worker.php",
        ])
        .unwrap();
        assert_eq!(cfg.long_paths_list(), vec!["/a".to_string(), "/b*".to_string()]);
        assert!(cfg.long_pool_enabled());
    }

    #[test]
    fn workers_max_defaults_to_min() {
        let cfg = Config::try_parse_from([
            "rearm-server",
            "--workers",
            "4",
            "--workers-max",
            "0",
            "--worker-script",
            "/tmp/worker.php",
        ])
        .unwrap();
        assert_eq!(cfg.workers_min(), 4);
        assert_eq!(cfg.workers_max_effective(), 4);
    }
}
