use crate::protocol_io::{read_response, write_frame, write_request};
use anyhow::{anyhow, Result};
use rearm_protocol::RequestFrame;
use std::path::{Path, PathBuf};
use std::time::Duration;
use sysinfo::{Pid, ProcessesToUpdate, System};
use tokio::process::Child;
use tokio::time::sleep;
use tracing::{info, warn};

#[cfg(unix)]
use anyhow::Context;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(unix)]
use tokio::process::Command;

/// Transport used to talk to a PHP worker.
///
/// Production target is Linux/WSL Unix domain sockets. On non-Unix hosts the
/// type still compiles so `cargo check` works, but workers will not start.
#[cfg(unix)]
type WorkerStream = UnixStream;

#[cfg(not(unix))]
type WorkerStream = tokio::net::TcpStream;

/// A single long-lived PHP worker process and its socket connection.
pub struct Worker {
    pub id: usize,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub sock_path: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub php_bin: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub worker_script: PathBuf,
    #[cfg_attr(not(unix), allow(dead_code))]
    pub app_root: PathBuf,
    /// OS child when spawned via `Command` (cold start / donor itself).
    pub child: Option<Child>,
    /// PID when created by PHP `pcntl_fork` (no Rust Child handle).
    pub forked_pid: Option<u32>,
    pub stream: Option<WorkerStream>,
    pub requests_handled: u64,
}

impl Worker {
    pub fn new(
        id: usize,
        sock_dir: &Path,
        php_bin: PathBuf,
        worker_script: PathBuf,
        app_root: PathBuf,
        sock_prefix: &str,
    ) -> Self {
        let sock_path = sock_dir.join(format!("{sock_prefix}-{id}.sock"));
        Self {
            id,
            sock_path,
            php_bin,
            worker_script,
            app_root,
            child: None,
            forked_pid: None,
            stream: None,
            requests_handled: 0,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.forked_pid
            .or_else(|| self.child.as_ref().and_then(|c| c.id()))
    }

    /// Spawn the PHP process and wait until the socket accepts connections.
    pub async fn start(&mut self) -> Result<()> {
        #[cfg(not(unix))]
        {
            let _ = self.id;
            return Err(anyhow!(
                "rearm-server PHP workers require Unix domain sockets (Linux/WSL)"
            ));
        }

        #[cfg(unix)]
        {
            self.cleanup_sock().await?;

            let role = if self.sock_path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("donor-"))
            {
                "donor"
            } else {
                "worker"
            };

            // Do not leak rearm-server supervisor knobs into PHP. Keep Laravel /
            // app env (APP_*, DB_*, …) and only the REARM_* keys workers need.
            let mut cmd = Command::new(&self.php_bin);
            cmd.arg(&self.worker_script)
                .env_clear()
                .envs(std::env::vars().filter(|(key, _)| !key.starts_with("REARM_")))
                .env("REARM_SOCK", &self.sock_path)
                .env("REARM_APP_ROOT", &self.app_root)
                .env("REARM_WORKER_ID", self.id.to_string())
                .env("REARM_WORKER_ROLE", role)
                .env(
                    "REARM_UPLOAD_DIR",
                    std::env::var("REARM_UPLOAD_DIR").unwrap_or_else(|_| "/var/www/uploads".into()),
                )
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .kill_on_drop(true);

            info!(
                worker = self.id,
                sock = %self.sock_path.display(),
                "spawning PHP worker"
            );
            let child = cmd.spawn().with_context(|| {
                format!(
                    "spawn {} {}",
                    self.php_bin.display(),
                    self.worker_script.display()
                )
            })?;
            self.child = Some(child);
            self.forked_pid = None;
            self.requests_handled = 0;
            self.stream = None;

            self.connect_with_retry(Duration::from_secs(10)).await?;
            Ok(())
        }
    }

    /// Adopt a PHP process created by the donor via `pcntl_fork`.
    pub async fn attach_forked(&mut self, pid: u32) -> Result<()> {
        #[cfg(not(unix))]
        {
            let _ = pid;
            return Err(anyhow!(
                "rearm-server PHP workers require Unix domain sockets (Linux/WSL)"
            ));
        }

        #[cfg(unix)]
        {
            self.child = None;
            self.forked_pid = Some(pid);
            self.requests_handled = 0;
            self.stream = None;
            self.connect_with_retry(Duration::from_secs(10)).await?;
            info!(worker = self.id, pid, "attached forked PHP worker");
            Ok(())
        }
    }

    pub async fn cleanup_sock(&self) -> Result<()> {
        #[cfg(unix)]
        {
            if self.sock_path.exists() {
                let _ = tokio::fs::remove_file(&self.sock_path).await;
            }
            if let Some(parent) = self.sock_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    async fn connect_with_retry(&mut self, timeout: Duration) -> Result<()> {
        let start = tokio::time::Instant::now();
        loop {
            if let Some(child) = self.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(anyhow!(
                        "worker {} exited during startup with {status}",
                        self.id
                    ));
                }
            }

            match UnixStream::connect(&self.sock_path).await {
                Ok(stream) => {
                    self.stream = Some(stream);
                    info!(worker = self.id, "connected to PHP worker socket");
                    return Ok(());
                }
                Err(err) => {
                    if start.elapsed() > timeout {
                        return Err(anyhow!(
                            "timed out connecting to worker {} at {}: {err}",
                            self.id,
                            self.sock_path.display()
                        ));
                    }
                    sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    /// Ensure a live stream, reconnecting if needed.
    pub async fn ensure_connected(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        if self.child.is_none()
            || self
                .child
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten())
                .is_some()
        {
            self.start().await?;
            return Ok(());
        }
        #[cfg(unix)]
        {
            self.connect_with_retry(Duration::from_secs(5)).await
        }
        #[cfg(not(unix))]
        {
            Err(anyhow!(
                "rearm-server PHP workers require Unix domain sockets (Linux/WSL)"
            ))
        }
    }

    /// Send a request frame and await the matching response.
    pub async fn handle(
        &mut self,
        request: &RequestFrame,
    ) -> Result<rearm_protocol::ResponseFrame> {
        self.ensure_connected().await?;

        {
            let stream = self
                .stream
                .as_mut()
                .ok_or_else(|| anyhow!("worker {} has no stream", self.id))?;
            if let Err(err) = write_request(stream, request).await {
                warn!(worker = self.id, %err, "write failed; restarting worker");
                self.restart().await?;
                let stream = self
                    .stream
                    .as_mut()
                    .ok_or_else(|| anyhow!("worker {} restart failed", self.id))?;
                write_request(stream, request).await?;
            }
        }

        let response = match read_response(
            self.stream
                .as_mut()
                .ok_or_else(|| anyhow!("worker {} has no stream", self.id))?,
        )
        .await
        {
            Ok(r) => r,
            Err(err) => {
                warn!(worker = self.id, error = %err, "read failed; restarting worker");
                // Drop the broken stream; restart so the next request gets a live PHP.
                self.stream = None;
                let _ = self.restart().await;
                return Err(err);
            }
        };

        self.requests_handled = self.requests_handled.saturating_add(1);
        Ok(response)
    }

    /// Sync kill for cancellation paths (Drop). Clears stream and child handle.
    pub fn force_kill(&mut self) {
        self.stream = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
        if let Some(pid) = self.forked_pid.take() {
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = pid;
        }
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.sock_path);
        }
    }

    /// Best-effort graceful shutdown via control frame, then kill.
    pub async fn shutdown(&mut self) {
        if let Ok(()) = self.ensure_connected().await {
            let frame = RequestFrame::shutdown();
            if let Some(stream) = self.stream.as_mut() {
                let _ = write_frame(stream, &frame).await;
                sleep(Duration::from_millis(200)).await;
            }
        }
        self.force_kill();
        info!(worker = self.id, "worker shut down");
    }

    /// Force restart: shutdown then start.
    pub async fn restart(&mut self) -> Result<()> {
        warn!(worker = self.id, "restarting worker");
        self.shutdown().await;
        self.start().await
    }

    /// True when the OS process has exited.
    pub fn has_exited(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            return matches!(child.try_wait(), Ok(Some(_)));
        }
        !self.is_alive()
    }

    /// Best-effort liveness via `kill(pid, 0)` / child presence (no wait).
    pub fn is_alive(&self) -> bool {
        if let Some(pid) = self.pid() {
            #[cfg(unix)]
            {
                return unsafe { libc::kill(pid as i32, 0) } == 0;
            }
            #[cfg(not(unix))]
            {
                let _ = pid;
                return self.child.is_some();
            }
        }
        false
    }

    /// RSS of the worker process in MiB, via `/proc` then sysinfo.
    pub fn rss_mb(&self) -> Option<u64> {
        let pid = self.pid()?;
        if let Some(mb) = rss_mb_from_proc(pid) {
            return Some(mb);
        }
        rss_mb_from_sysinfo(pid)
    }

    pub fn needs_recycle(&self, memory_limit_mb: u64, max_requests: u64) -> bool {
        if max_requests > 0 && self.requests_handled >= max_requests {
            return true;
        }
        if memory_limit_mb > 0 {
            if let Some(rss) = self.rss_mb() {
                if rss >= memory_limit_mb {
                    return true;
                }
            }
        }
        false
    }
}

fn rss_mb_from_proc(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

fn rss_mb_from_sysinfo(pid: u32) -> Option<u64> {
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    let proc = sys.process(Pid::from_u32(pid))?;
    Some(proc.memory() / 1024 / 1024)
}
