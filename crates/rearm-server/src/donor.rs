//! Shared warm PHP donor: boots Laravel once, then `pcntl_fork`s children for scale-up.

use crate::config::Config;
use crate::worker::Worker;
use anyhow::{anyhow, Context, Result};
use rearm_protocol::RequestFrame;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Process-wide donor used by every WorkerPool (short and long).
#[derive(Clone)]
pub struct DonorHandle {
    inner: Arc<Mutex<Worker>>,
}

#[derive(Debug, Deserialize)]
struct ForkResponseBody {
    ok: bool,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    error: Option<String>,
}

impl DonorHandle {
    /// Cold-boot one PHP process that only handles control `fork` / `shutdown`.
    pub async fn start(config: &Config) -> Result<Self> {
        tokio::fs::create_dir_all(&config.sock_dir).await?;
        let mut worker = Worker::new(
            0,
            &config.sock_dir,
            config.php_bin.clone(),
            config.worker_script.clone(),
            config.app_root.clone(),
            "donor",
        );
        worker.start().await.context("start PHP donor worker")?;
        info!(
            sock = %worker.sock_path.display(),
            pid = ?worker.pid(),
            "PHP donor ready (fork scale-up enabled)"
        );
        Ok(Self {
            inner: Arc::new(Mutex::new(worker)),
        })
    }

    /// Ask the donor to fork a child listening on `target.sock_path`.
    pub async fn fork_into(&self, target: &mut Worker) -> Result<()> {
        target.cleanup_sock().await?;

        let sock = target.sock_path.to_string_lossy().into_owned();
        let id = target.id.to_string();

        let mut headers = HashMap::new();
        headers.insert("rearm-fork-sock".into(), sock);
        headers.insert("rearm-fork-id".into(), id);

        let frame = RequestFrame {
            method: String::new(),
            uri: String::new(),
            headers,
            remote_addr: String::new(),
            body: Vec::new(),
            form: None,
            files: None,
            control: Some("fork".into()),
        };

        let response = {
            let mut donor = self.inner.lock().await;
            donor.handle(&frame).await.context("donor fork control")?
        };

        if response.status != 200 {
            let body = String::from_utf8_lossy(&response.body);
            return Err(anyhow!(
                "donor fork rejected status={} body={}",
                response.status,
                body
            ));
        }

        let parsed: ForkResponseBody = serde_json::from_slice(&response.body)
            .context("parse donor fork JSON body")?;
        if !parsed.ok {
            return Err(anyhow!(
                "donor fork failed: {}",
                parsed.error.unwrap_or_else(|| "unknown".into())
            ));
        }
        let pid = parsed
            .pid
            .ok_or_else(|| anyhow!("donor fork ok but missing pid"))?;

        if let Err(err) = target
            .attach_forked(pid)
            .await
            .with_context(|| format!("connect to forked worker pid={pid}"))
        {
            // Child never listened — reap so we do not leak PHP processes.
            #[cfg(unix)]
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
            return Err(err);
        }
        Ok(())
    }

    pub async fn shutdown(&self) {
        let mut donor = self.inner.lock().await;
        donor.shutdown().await;
    }

    /// Snapshot for `/__rearm/metrics`: pid + liveness (+ RSS when available).
    pub async fn metrics_status(&self) -> (Option<u32>, bool, Option<u64>) {
        let donor = self.inner.lock().await;
        let pid = donor.pid();
        let alive = donor.is_alive();
        let rss_mb = donor.rss_mb();
        (pid, alive, rss_mb)
    }
}

/// Boot `target` via donor fork, falling back to a cold spawn.
pub async fn start_worker(target: &mut Worker, donor: &Option<DonorHandle>) -> Result<()> {
    if let Some(donor) = donor {
        match donor.fork_into(target).await {
            Ok(()) => return Ok(()),
            Err(err) => {
                warn!(
                    worker = target.id,
                    sock = %target.sock_path.display(),
                    error = %err,
                    "donor fork failed; cold-starting worker"
                );
            }
        }
    }
    target.start().await
}
