use crate::config::Config;
use crate::donor::{start_worker, DonorHandle};
use crate::worker::Worker;
use anyhow::{anyhow, Result};
use rearm_protocol::{RequestFrame, ResponseFrame};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use sysinfo::{get_current_pid, Pid, ProcessesToUpdate, System};
use tokio::sync::{oneshot, Mutex};
use tokio::time::{interval, timeout, Duration, Instant};
use tracing::{error, info, warn};

/// Snapshot of one worker pool (PHP processes only; no rearm-server RSS/CPU).
#[derive(Debug, Clone)]
pub struct PoolMetrics {
    pub workers_min: usize,
    pub workers_max: usize,
    pub started: usize,
    pub busy: usize,
    pub idle: usize,
    pub waiters: usize,
    pub scale_ups: u64,
    pub scale_downs: u64,
    pub workers: Vec<WorkerMetric>,
    /// Sum of started PHP worker RSS (MiB).
    pub rss_mb_total: u64,
    /// PIDs of started PHP workers (for host CPU sample at PoolSet level).
    pub pids: Vec<Pid>,
}

#[derive(Debug, Clone)]
pub struct WorkerMetric {
    pub id: usize,
    pub pid: Option<u32>,
    pub rss_mb: Option<u64>,
    pub requests: u64,
    pub state: &'static str,
}

/// Pool of persistent PHP workers with acquire/release and elastic scale.
#[derive(Clone)]
pub struct WorkerPool {
    inner: Arc<PoolInner>,
}

/// FIFO waiter: oldest request gets the next free / scaled worker.
struct WaitEntry {
    alive: Arc<std::sync::atomic::AtomicBool>,
    tx: oneshot::Sender<usize>,
}

struct PoolInner {
    /// Socket / log name (`worker` or `long`).
    name: String,
    /// Shared warm PHP donor for fork scale-up (process-wide).
    donor: Option<DonorHandle>,
    slots: Vec<Mutex<Worker>>,
    /// Sync mutex so release can happen from any context.
    free: StdMutex<VecDeque<usize>>,
    /// Oldest-first queue of tasks waiting for a worker.
    wait_q: StdMutex<VecDeque<WaitEntry>>,
    /// Which slot ids currently have a live (or starting) process.
    started: StdMutex<Vec<bool>>,
    /// When each idle worker entered the free list.
    idle_since: StdMutex<Vec<Option<Instant>>>,
    waiters: AtomicUsize,
    busy: AtomicUsize,
    scale_ups: AtomicU64,
    scale_downs: AtomicU64,
    last_scale_up: StdMutex<Option<Instant>>,
    /// When waiters first crossed the scale-up threshold with an empty free list.
    queue_pressure_since: StdMutex<Option<Instant>>,
    min_workers: usize,
    max_workers: usize,
    scale_up_waiters: usize,
    scale_up_queue: Duration,
    scale_up_cooldown: Duration,
    scale_up_batch: usize,
    scale_down_idle: Duration,
    /// No scale-down until this long after the last scale-up burst.
    scale_down_grace: Duration,
    scale_down_batch: usize,
    memory_limit_mb: u64,
    max_requests: u64,
}

impl PoolInner {
    fn started_count(&self) -> usize {
        self.started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|s| **s)
            .count()
    }

    fn mark_busy(&self, id: usize) {
        {
            let mut idle = self.idle_since.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(slot) = idle.get_mut(id) {
                *slot = None;
            }
        }
        self.busy.fetch_add(1, Ordering::Relaxed);
        let _ = id;
    }

    fn park_idle(&self, id: usize) {
        {
            let mut idle = self.idle_since.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(slot) = idle.get_mut(id) {
                *slot = Some(Instant::now());
            }
        }
        self.free
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(id);
    }

    fn prune_dead_waiters(&self) {
        let mut q = self.wait_q.lock().unwrap_or_else(|e| e.into_inner());
        q.retain(|e| e.alive.load(Ordering::Relaxed));
    }

    /// Give `id` to the oldest live waiter, or park it in `free`.
    fn handoff_or_park(&self, id: usize) {
        loop {
            let entry = {
                let mut q = self.wait_q.lock().unwrap_or_else(|e| e.into_inner());
                q.pop_front()
            };
            let Some(entry) = entry else {
                self.park_idle(id);
                return;
            };
            if !entry.alive.load(Ordering::Relaxed) {
                continue;
            }
            match entry.tx.send(id) {
                Ok(()) => {
                    self.mark_busy(id);
                    return;
                }
                Err(_) => continue, // receiver dropped
            }
        }
    }

    fn release(&self, id: usize) {
        self.busy.fetch_sub(1, Ordering::Relaxed);
        self.handoff_or_park(id);
    }

    /// Move parked workers to FIFO waiters if any.
    fn dispatch_free_to_waiters(&self) {
        loop {
            let has_waiter = {
                let q = self.wait_q.lock().unwrap_or_else(|e| e.into_inner());
                q.iter().any(|e| e.alive.load(Ordering::Relaxed))
            };
            if !has_waiter {
                return;
            }
            let id = {
                let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
                free.pop_front()
            };
            let Some(id) = id else {
                return;
            };
            self.handoff_or_park(id);
        }
    }

    fn clear_queue_pressure(&self) {
        let mut since = self
            .queue_pressure_since
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *since = None;
    }

    /// Sustained backlog: enough waiters for long enough, plus burst cooldown.
    fn can_scale_up(&self, waiting: usize) -> bool {
        if self.started_count() >= self.max_workers {
            self.clear_queue_pressure();
            return false;
        }
        if waiting < self.scale_up_waiters {
            self.clear_queue_pressure();
            return false;
        }

        let now = Instant::now();
        {
            let mut since = self
                .queue_pressure_since
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            match *since {
                None => {
                    *since = Some(now);
                    return false;
                }
                Some(t) if now.duration_since(t) < self.scale_up_queue => {
                    return false;
                }
                Some(_) => {}
            }
        }

        let last = self.last_scale_up.lock().unwrap_or_else(|e| e.into_inner());
        match *last {
            Some(t) if t.elapsed() < self.scale_up_cooldown => false,
            _ => true,
        }
    }

    /// Reserve and start up to `want` stopped workers in parallel.
    /// Returns successfully started ids (may be fewer than want).
    async fn scale_up_burst(self: &Arc<Self>, want: usize) -> Vec<usize> {
        let want = want.max(1).min(self.scale_up_batch);
        let ids: Vec<usize> = {
            let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
            let mut ids = Vec::new();
            for (i, flag) in started.iter_mut().enumerate() {
                if !*flag {
                    *flag = true;
                    ids.push(i);
                    if ids.len() >= want {
                        break;
                    }
                }
            }
            ids
        };

        if ids.is_empty() {
            return Vec::new();
        }

        {
            let mut last = self.last_scale_up.lock().unwrap_or_else(|e| e.into_inner());
            *last = Some(Instant::now());
        }

        let mut join = Vec::with_capacity(ids.len());
        for id in ids {
            let inner = Arc::clone(self);
            join.push(tokio::spawn(async move {
                let start_result = {
                    let mut worker = inner.slots[id].lock().await;
                    start_worker(&mut worker, &inner.donor).await
                };
                match start_result {
                    Ok(()) => {
                        inner.scale_ups.fetch_add(1, Ordering::Relaxed);
                        info!(
                            pool = %inner.name,
                            worker = id,
                            started = inner.started_count(),
                            max = inner.max_workers,
                            via_fork = inner.donor.is_some(),
                            "scale_up: started PHP worker"
                        );
                        Some(id)
                    }
                    Err(err) => {
                        error!(worker = id, %err, "scale_up: worker start failed");
                        let mut started =
                            inner.started.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(slot) = started.get_mut(id) {
                            *slot = false;
                        }
                        None
                    }
                }
            }));
        }

        let mut ready = Vec::new();
        for handle in join {
            match handle.await {
                Ok(Some(id)) => ready.push(id),
                Ok(None) => {}
                Err(err) => error!(%err, "scale_up: join failed"),
            }
        }
        ready
    }

    /// Acquire a free (or newly scaled) worker id — FIFO among waiters.
    /// Returns `Ok(None)` if `cancelled` becomes true while waiting.
    async fn acquire_id(self: &Arc<Self>, cancelled: impl Fn() -> bool) -> Result<Option<usize>> {
        let deadline = Instant::now() + Duration::from_secs(30);

        // Fast path: take a parked worker only if nobody is already queued
        // (otherwise a newcomer would cut the FIFO line).
        {
            let mut q = self.wait_q.lock().unwrap_or_else(|e| e.into_inner());
            q.retain(|e| e.alive.load(Ordering::Relaxed));
            if q.is_empty() {
                drop(q);
                if let Some(id) = self
                    .free
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front()
                {
                    self.clear_queue_pressure();
                    self.mark_busy(id);
                    return Ok(Some(id));
                }
            }
        }

        if cancelled() {
            return Ok(None);
        }

        let (tx, rx) = oneshot::channel();
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        {
            let mut q = self.wait_q.lock().unwrap_or_else(|e| e.into_inner());
            q.push_back(WaitEntry {
                alive: Arc::clone(&alive),
                tx,
            });
        }
        self.waiters.fetch_add(1, Ordering::Relaxed);
        let _guard = WaitersGuard {
            waiters: &self.waiters,
        };

        // A release may have raced; drain free → FIFO.
        self.dispatch_free_to_waiters();

        let mut rx = rx;
        loop {
            if cancelled() {
                alive.store(false, Ordering::Relaxed);
                self.prune_dead_waiters();
                return Ok(None);
            }

            let waiting = self.waiters.load(Ordering::Relaxed);
            if self.can_scale_up(waiting) {
                self.clear_queue_pressure();
                let room = self.max_workers.saturating_sub(self.started_count());
                let want = waiting.min(room).min(self.scale_up_batch).max(1);
                let ready = self.scale_up_burst(want).await;
                for id in ready {
                    self.handoff_or_park(id);
                }
            }

            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                alive.store(false, Ordering::Relaxed);
                self.prune_dead_waiters();
                return Err(anyhow!("timed out waiting for a free PHP worker"));
            }

            let slice = left.min(Duration::from_millis(25));
            tokio::select! {
                biased;
                got = &mut rx => {
                    match got {
                        Ok(id) => {
                            self.clear_queue_pressure();
                            return Ok(Some(id));
                        }
                        Err(_) => {
                            alive.store(false, Ordering::Relaxed);
                            self.prune_dead_waiters();
                            return Ok(None);
                        }
                    }
                }
                _ = tokio::time::sleep(slice) => {
                    self.dispatch_free_to_waiters();
                }
            }
        }
    }

    /// Shut down up to `batch` idle workers above min that have been idle long enough.
    async fn scale_down_idle_workers(self: &Arc<Self>) {
        if self.started_count() <= self.min_workers {
            return;
        }

        // Never shrink while requests are queued — that amplifies backlog.
        if self.waiters.load(Ordering::Relaxed) > 0 {
            return;
        }

        // Anti-churn: keep the burst warm through short quiet gaps (bench drains, bursts).
        if !self.scale_down_grace.is_zero() {
            let last = self.last_scale_up.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(t) = *last {
                if t.elapsed() < self.scale_down_grace {
                    return;
                }
            }
        }

        let mut victims: Vec<usize> = Vec::new();
        {
            let idle_since = self.idle_since.lock().unwrap_or_else(|e| e.into_inner());
            let mut free = self.free.lock().unwrap_or_else(|e| e.into_inner());
            let mut kept = VecDeque::new();
            while let Some(id) = free.pop_front() {
                let eligible = self.started_count() - victims.len() > self.min_workers
                    && victims.len() < self.scale_down_batch
                    && idle_since
                        .get(id)
                        .and_then(|t| *t)
                        .is_some_and(|t| t.elapsed() >= self.scale_down_idle);
                if eligible {
                    victims.push(id);
                } else {
                    kept.push_back(id);
                }
            }
            *free = kept;
        }

        for id in victims {
            {
                let mut worker = self.slots[id].lock().await;
                worker.shutdown().await;
            }
            {
                let mut started = self.started.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = started.get_mut(id) {
                    *slot = false;
                }
                let mut idle = self.idle_since.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(slot) = idle.get_mut(id) {
                    *slot = None;
                }
            }
            self.scale_downs.fetch_add(1, Ordering::Relaxed);
            info!(
                pool = %self.name,
                worker = id,
                started = self.started_count(),
                min = self.min_workers,
                "scale_down: stopped idle PHP worker"
            );
        }
    }
}

struct WaitersGuard<'a> {
    waiters: &'a AtomicUsize,
}

impl Drop for WaitersGuard<'_> {
    fn drop(&mut self) {
        self.waiters.fetch_sub(1, Ordering::Relaxed);
    }
}

impl WorkerPool {
    /// Single-pool boot (legacy): sockets named `worker-{id}.sock`.
    pub async fn start(config: &Config, donor: Option<DonorHandle>) -> Result<Self> {
        Self::start_named(
            config,
            "worker",
            config.workers_min(),
            config.workers_max_effective(),
            donor,
        )
        .await
    }

    /// Boot a named pool; `sock_prefix` isolates UDS paths across short/long pools.
    pub async fn start_named(
        config: &Config,
        sock_prefix: &str,
        min: usize,
        max: usize,
        donor: Option<DonorHandle>,
    ) -> Result<Self> {
        tokio::fs::create_dir_all(&config.sock_dir).await?;
        tokio::fs::create_dir_all(&config.upload_dir).await?;

        let min = min.max(1);
        let max = max.max(min);
        let mut slots = Vec::with_capacity(max);
        let mut started = vec![false; max];
        let idle_since = vec![None; max];

        for id in 0..max {
            let worker = Worker::new(
                id,
                &config.sock_dir,
                config.php_bin.clone(),
                config.worker_script.clone(),
                config.app_root.clone(),
                sock_prefix,
            );
            slots.push(Mutex::new(worker));
        }

        // Boot warm workers. Donor forks serialize on one mutex (still ≪ cold boot).
        let slots = Arc::new(slots);
        let mut join = Vec::with_capacity(min);
        for id in 0..min {
            let slots = Arc::clone(&slots);
            let donor = donor.clone();
            join.push(tokio::spawn(async move {
                let start_result = {
                    let mut worker = slots[id].lock().await;
                    start_worker(&mut worker, &donor).await
                };
                (id, start_result)
            }));
        }

        let mut free = VecDeque::with_capacity(min);
        let mut idle_since = idle_since;
        for handle in join {
            match handle.await {
                Ok((id, Ok(()))) => {
                    started[id] = true;
                    idle_since[id] = Some(Instant::now());
                    free.push_back(id);
                }
                Ok((id, Err(err))) => {
                    warn!(
                        pool = sock_prefix,
                        worker = id,
                        error = %err,
                        "initial worker start failed; will retry on first request"
                    );
                    started[id] = true;
                    idle_since[id] = Some(Instant::now());
                    free.push_back(id);
                }
                Err(err) => {
                    error!(pool = sock_prefix, %err, "initial worker join failed");
                }
            }
        }

        let slots = Arc::try_unwrap(slots).map_err(|_| {
            anyhow!("worker slots still shared after parallel boot (bug)")
        })?;

        info!(
            pool = sock_prefix,
            min,
            max,
            fork_scale = donor.is_some(),
            scale_up_waiters = config.scale_up_waiters,
            scale_down_grace_ms = config.scale_down_grace_ms,
            "worker pool ready"
        );

        let pool = Self {
            inner: Arc::new(PoolInner {
                name: sock_prefix.to_string(),
                donor,
                slots,
                free: StdMutex::new(free),
                wait_q: StdMutex::new(VecDeque::new()),
                started: StdMutex::new(started),
                idle_since: StdMutex::new(idle_since),
                waiters: AtomicUsize::new(0),
                busy: AtomicUsize::new(0),
                scale_ups: AtomicU64::new(0),
                scale_downs: AtomicU64::new(0),
                last_scale_up: StdMutex::new(None),
                queue_pressure_since: StdMutex::new(None),
                min_workers: min,
                max_workers: max,
                scale_up_waiters: config.scale_up_waiters.max(1),
                scale_up_queue: config.scale_up_queue(),
                scale_up_cooldown: config.scale_up_cooldown(),
                scale_up_batch: config.scale_up_batch.max(1),
                scale_down_idle: config.scale_down_idle(),
                scale_down_grace: config.scale_down_grace(),
                scale_down_batch: config.scale_down_batch.max(1),
                memory_limit_mb: config.memory_limit_mb,
                max_requests: config.max_requests,
            }),
        };

        pool.spawn_monitor();
        Ok(pool)
    }

    /// Forward a request to a free worker and return its response.
    ///
    /// PHP work runs in a detached task. If the HTTP client disconnects we must
    /// NOT kill the PHP process — a cold Laravel boot is expensive. Cancelled
    /// callers drop the oneshot; in-flight work finishes and the slot returns.
    pub async fn forward(&self, request: RequestFrame) -> Result<ResponseFrame> {
        let inner = self.inner.clone();
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            if tx.is_closed() {
                return;
            }

            let id = match inner.acquire_id(|| tx.is_closed()).await {
                Ok(Some(id)) => id,
                Ok(None) => return, // client gone while queued — do not burn a PHP slot
                Err(err) => {
                    let _ = tx.send(Err(err));
                    return;
                }
            };

            if tx.is_closed() {
                inner.release(id);
                return;
            }

            let result = run_on_worker(&inner, id, request).await;
            inner.release(id);
            let _ = tx.send(result);
        });

        match rx.await {
            Ok(result) => result,
            Err(_) => Err(anyhow!("client cancelled before PHP worker responded")),
        }
    }

    pub async fn metrics(&self) -> PoolMetrics {
        let inner = &self.inner;
        let started_flags = inner
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let free_ids: std::collections::HashSet<usize> = inner
            .free
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .copied()
            .collect();

        let mut workers = Vec::with_capacity(inner.slots.len());
        let mut rss_mb_total = 0u64;
        let mut pids: Vec<Pid> = Vec::new();

        for (id, slot) in inner.slots.iter().enumerate() {
            let started = started_flags.get(id).copied().unwrap_or(false);
            // Skip stopped slots entirely — they only add noise to metrics.
            if !started {
                continue;
            }

            let Ok(worker) = slot.try_lock() else {
                workers.push(WorkerMetric {
                    id,
                    pid: None,
                    rss_mb: None,
                    requests: 0,
                    state: "busy",
                });
                continue;
            };

            let state = if free_ids.contains(&id) {
                "idle"
            } else {
                "busy"
            };

            let pid = worker.pid();
            let rss = worker.rss_mb();
            if let Some(mb) = rss {
                rss_mb_total = rss_mb_total.saturating_add(mb);
            }
            if let Some(p) = pid {
                pids.push(Pid::from_u32(p));
            }

            workers.push(WorkerMetric {
                id,
                pid,
                rss_mb: rss,
                requests: worker.requests_handled,
                state,
            });
        }

        let idle = free_ids.len();
        let started = started_flags.iter().filter(|s| **s).count();
        let busy = inner.busy.load(Ordering::Relaxed);

        PoolMetrics {
            workers_min: inner.min_workers,
            workers_max: inner.max_workers,
            started,
            busy,
            idle,
            waiters: inner.waiters.load(Ordering::Relaxed),
            scale_ups: inner.scale_ups.load(Ordering::Relaxed),
            scale_downs: inner.scale_downs.load(Ordering::Relaxed),
            workers,
            rss_mb_total,
            pids,
        }
    }

    fn spawn_monitor(&self) {
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let mut tick = interval(Duration::from_secs(2));
            loop {
                tick.tick().await;

                inner.scale_down_idle_workers().await;

                let started = inner
                    .started
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                for (id, slot) in inner.slots.iter().enumerate() {
                    if !started.get(id).copied().unwrap_or(false) {
                        continue;
                    }
                    let Ok(mut worker) = slot.try_lock() else {
                        continue;
                    };
                    if worker.has_exited() {
                        warn!(worker = id, "monitor: worker exited; restarting");
                        if let Err(err) = restart_worker(&mut worker, &inner.donor).await {
                            error!(worker = id, %err, "monitor restart failed");
                            let mut flags = inner.started.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(slot) = flags.get_mut(id) {
                                *slot = false;
                            }
                        }
                        continue;
                    }
                    if worker.needs_recycle(inner.memory_limit_mb, inner.max_requests) {
                        info!(
                            worker = id,
                            requests = worker.requests_handled,
                            rss_mb = ?worker.rss_mb(),
                            "monitor: recycling idle worker"
                        );
                        if let Err(err) = restart_worker(&mut worker, &inner.donor).await {
                            error!(worker = id, %err, "monitor recycle failed");
                        }
                    }
                }
            }
        });
    }

    pub async fn shutdown(&self) {
        info!(pool = %self.inner.name, "shutting down worker pool");
        let started = self
            .inner
            .started
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for (id, slot) in self.inner.slots.iter().enumerate() {
            if !started.get(id).copied().unwrap_or(false) {
                continue;
            }
            let mut worker = slot.lock().await;
            worker.shutdown().await;
        }
    }

    fn pool_snapshot_json(metrics: &PoolMetrics, include_workers: bool) -> Value {
        let mut snap = json!({
            "min": metrics.workers_min,
            "max": metrics.workers_max,
            "started": metrics.started,
            "busy": metrics.busy,
            "idle": metrics.idle,
            "waiters": metrics.waiters,
            "scale_ups": metrics.scale_ups,
            "scale_downs": metrics.scale_downs,
            "rss_mb": metrics.rss_mb_total,
        });
        if include_workers {
            snap["workers"] = json!(metrics
                .workers
                .iter()
                .map(|w| json!({
                    "id": w.id,
                    "pid": w.pid,
                    "state": w.state,
                    "requests": w.requests,
                    "rss_mb": w.rss_mb,
                }))
                .collect::<Vec<_>>());
        }
        snap
    }
}

/// Short (+ optional long) worker pools. Split only when `REARM_LONG_PATHS` is set.
#[derive(Clone)]
pub struct PoolSet {
    short: WorkerPool,
    long: Option<WorkerPool>,
    long_paths: Arc<Vec<String>>,
    donor: Option<DonorHandle>,
}

impl PoolSet {
    pub async fn start(config: &Config) -> Result<Self> {
        let donor = if config.fork_scale {
            match DonorHandle::start(config).await {
                Ok(d) => Some(d),
                Err(err) => {
                    warn!(error = %err, "PHP donor failed to start; using cold spawn only");
                    None
                }
            }
        } else {
            info!("fork scale-up disabled (REARM_FORK_SCALE=false)");
            None
        };

        let paths = config.long_paths_list();
        if paths.is_empty() {
            let short = WorkerPool::start(config, donor.clone()).await?;
            info!(fork_scale = donor.is_some(), "pool mode: single (REARM_LONG_PATHS empty)");
            return Ok(Self {
                short,
                long: None,
                long_paths: Arc::new(Vec::new()),
                donor,
            });
        }

        let short = WorkerPool::start_named(
            config,
            "worker",
            config.workers_min(),
            config.workers_max_effective(),
            donor.clone(),
        )
        .await?;
        let long = WorkerPool::start_named(
            config,
            "long",
            config.long_workers_min(),
            config.long_workers_max_effective(),
            donor.clone(),
        )
        .await?;
        info!(
            long_paths = ?paths,
            long_min = config.long_workers_min(),
            long_max = config.long_workers_max_effective(),
            fork_scale = donor.is_some(),
            "pool mode: split short/long"
        );
        Ok(Self {
            short,
            long: Some(long),
            long_paths: Arc::new(paths),
            donor,
        })
    }

    /// Route by path: allowlisted → long pool; everything else → short (or single).
    pub async fn forward(&self, path: &str, request: RequestFrame) -> Result<ResponseFrame> {
        if let Some(long) = &self.long {
            if path_matches_allowlist(path, &self.long_paths) {
                return long.forward(request).await;
            }
        }
        self.short.forward(request).await
    }

    /// Build `/__rearm/metrics` body. Set `include_workers=false` (`?workers=0`) for compact JSONL.
    pub async fn metrics_json(&self, include_workers: bool) -> Value {
        let short_m = self.short.metrics().await;
        let long_m = if let Some(long) = &self.long {
            Some(long.metrics().await)
        } else {
            None
        };

        let mut started = short_m.started;
        let mut busy = short_m.busy;
        let mut idle = short_m.idle;
        let mut waiters = short_m.waiters;
        let mut scale_ups = short_m.scale_ups;
        let mut scale_downs = short_m.scale_downs;
        let mut rss_mb = short_m.rss_mb_total;
        let mut pids = short_m.pids.clone();

        let mut pools = json!({
            "short": WorkerPool::pool_snapshot_json(&short_m, include_workers),
        });

        let mode = if let Some(ref lm) = long_m {
            started += lm.started;
            busy += lm.busy;
            idle += lm.idle;
            waiters += lm.waiters;
            scale_ups += lm.scale_ups;
            scale_downs += lm.scale_downs;
            rss_mb = rss_mb.saturating_add(lm.rss_mb_total);
            pids.extend(lm.pids.iter().copied());
            pools["long"] = WorkerPool::pool_snapshot_json(lm, include_workers);
            "split"
        } else {
            "single"
        };

        let mut donor_json = json!(null);
        if let Some(donor) = &self.donor {
            let (pid, alive, donor_rss) = donor.metrics_status().await;
            if let Some(mb) = donor_rss {
                rss_mb = rss_mb.saturating_add(mb);
            }
            if let Some(p) = pid {
                pids.push(Pid::from_u32(p));
            }
            donor_json = json!({
                "pid": pid,
                "alive": alive,
                "rss_mb": donor_rss,
            });
        }

        let mut server_rss = 0u64;
        if let Ok(server_pid) = get_current_pid() {
            pids.push(server_pid);
            if let Some(mb) = rss_mb_from_proc_pid(server_pid.as_u32()) {
                server_rss = mb;
                rss_mb = rss_mb.saturating_add(mb);
            }
        }

        let (cpu_pct, host_cpus) = sample_cpu(&pids);
        let cpu_pct = (cpu_pct * 10.0).round() / 10.0;

        let mut body = json!({
            "ok": true,
            "service": "rearm-server",
            "mode": mode,
            "fork_scale": self.donor.is_some(),
            "totals": {
                "started": started,
                "busy": busy,
                "idle": idle,
                "waiters": waiters,
                "rss_mb": rss_mb,
                "scale_ups": scale_ups,
                "scale_downs": scale_downs,
            },
            "donor": donor_json,
            "pools": pools,
            "host": {
                "cpus": host_cpus,
                "cpu_pct": cpu_pct,
                "server_rss_mb": server_rss,
            },
        });

        if mode == "split" {
            body["long_paths"] = json!(&*self.long_paths);
        }

        body
    }

    /// Human-readable multi-line summary for `?format=text`.
    pub fn metrics_text(value: &Value) -> String {
        let totals = &value["totals"];
        let pools = &value["pools"];
        let short = &pools["short"];
        let host = &value["host"];
        let donor = &value["donor"];

        let mut lines = Vec::new();
        lines.push(format!(
            "rearm-server mode={} fork_scale={}",
            value["mode"].as_str().unwrap_or("?"),
            value["fork_scale"]
        ));
        lines.push(format!(
            "totals started={} busy={} idle={} waiters={} rss_mb={} ups={} downs={}",
            totals["started"],
            totals["busy"],
            totals["idle"],
            totals["waiters"],
            totals["rss_mb"],
            totals["scale_ups"],
            totals["scale_downs"],
        ));
        lines.push(format!(
            "short {}/{} busy={} idle={} wait={} ups={} downs={} rss={}",
            short["started"],
            short["max"],
            short["busy"],
            short["idle"],
            short["waiters"],
            short["scale_ups"],
            short["scale_downs"],
            short["rss_mb"],
        ));
        if let Some(long) = pools.get("long") {
            lines.push(format!(
                "long {}/{} busy={} idle={} wait={} ups={} downs={} rss={}",
                long["started"],
                long["max"],
                long["busy"],
                long["idle"],
                long["waiters"],
                long["scale_ups"],
                long["scale_downs"],
                long["rss_mb"],
            ));
        }
        if !donor.is_null() {
            lines.push(format!(
                "donor pid={} alive={} rss_mb={}",
                donor["pid"],
                donor["alive"],
                donor["rss_mb"]
            ));
        }
        lines.push(format!(
            "host cpus={} cpu_pct={} server_rss_mb={}",
            host["cpus"], host["cpu_pct"], host["server_rss_mb"]
        ));
        lines.join("\n") + "\n"
    }

    pub async fn shutdown(&self) {
        self.short.shutdown().await;
        if let Some(long) = &self.long {
            long.shutdown().await;
        }
        if let Some(donor) = &self.donor {
            donor.shutdown().await;
        }
    }
}

/// Match request path against allowlist patterns (`/io-slow`, `/report*`, `/export/*`).
pub fn path_matches_allowlist(path: &str, patterns: &[String]) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    patterns.iter().any(|pat| path_matches(path, pat))
}

fn path_matches(path: &str, pattern: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        return path.starts_with(prefix);
    }
    path == pattern || path.starts_with(&format!("{pattern}/"))
}

#[cfg(test)]
mod allowlist_tests {
    use super::*;

    #[test]
    fn exact_and_prefix_and_glob() {
        let patterns = vec![
            "/io-slow".into(),
            "/report*".into(),
            "/export".into(),
        ];
        assert!(path_matches_allowlist("/io-slow", &patterns));
        assert!(path_matches_allowlist("/io-slow/extra", &patterns));
        assert!(path_matches_allowlist("/report", &patterns));
        assert!(path_matches_allowlist("/report-csv", &patterns));
        assert!(path_matches_allowlist("/export/x", &patterns));
        assert!(!path_matches_allowlist("/fast", &patterns));
        assert!(!path_matches_allowlist("/io", &patterns));
    }

    #[test]
    fn strips_query_string() {
        let patterns = vec!["/slow".into()];
        assert!(path_matches_allowlist("/slow?x=1", &patterns));
    }
}

async fn restart_worker(worker: &mut Worker, donor: &Option<DonorHandle>) -> Result<()> {
    worker.shutdown().await;
    start_worker(worker, donor).await
}

async fn run_on_worker(
    inner: &PoolInner,
    id: usize,
    request: RequestFrame,
) -> Result<ResponseFrame> {
    let mut worker = inner.slots[id].lock().await;
    if worker.has_exited() {
        restart_worker(&mut worker, &inner.donor).await?;
    }

    match timeout(Duration::from_secs(30), worker.handle(&request)).await {
        Ok(Ok(response)) => {
            let recycle = worker.needs_recycle(inner.memory_limit_mb, inner.max_requests);
            if recycle {
                info!(worker = id, "recycling worker after request");
                if let Err(err) = restart_worker(&mut worker, &inner.donor).await {
                    error!(worker = id, %err, "failed to recycle worker");
                }
            }
            Ok(response)
        }
        Ok(Err(err)) => {
            worker.stream = None;
            let _ = restart_worker(&mut worker, &inner.donor).await;
            Err(err)
        }
        Err(_) => {
            warn!(worker = id, "PHP worker handle timed out; restarting");
            worker.stream = None;
            let _ = restart_worker(&mut worker, &inner.donor).await;
            Err(anyhow!("PHP worker timed out after 30s"))
        }
    }
}

fn rss_mb_from_proc_pid(pid: u32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

fn sample_cpu(pids: &[Pid]) -> (f32, usize) {
    let mut sys = System::new();
    let host_cpus = sys.cpus().len().max(1);
    if pids.is_empty() {
        return (0.0, host_cpus);
    }
    sys.refresh_processes(ProcessesToUpdate::Some(pids), true);
    std::thread::sleep(Duration::from_millis(50));
    sys.refresh_processes(ProcessesToUpdate::Some(pids), true);
    let mut cpu = 0.0f32;
    for pid in pids {
        if let Some(proc) = sys.process(*pid) {
            cpu += proc.cpu_usage();
        }
    }
    (cpu, host_cpus)
}
