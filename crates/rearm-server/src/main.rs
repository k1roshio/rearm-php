mod compress;
mod config;
mod donor;
mod handler;
mod http1;
mod http3;
mod multipart;
mod pool;
mod protocol_io;
mod static_files;
mod worker;

use crate::config::Config;
use crate::handler::AppState;
use crate::pool::PoolSet;
use anyhow::Result;
use clap::Parser;
use std::sync::Arc;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::parse();
    info!(
        listen = %config.listen,
        http3 = config.http3,
        http3_listen = %config.http3_listen,
        workers_min = config.workers_min(),
        workers_max = config.workers_max_effective(),
        long_pool = config.long_pool_enabled(),
        fork_scale = config.fork_scale,
        "starting rearm-server"
    );

    let pools = PoolSet::start(&config).await?;
    let state = AppState {
        pools: pools.clone(),
        config: Arc::new(config.clone()),
    };

    let http3_handle = http3::maybe_serve(
        config.http3,
        config.http3_listen,
        config.cert.as_deref(),
        config.key.as_deref(),
        state.clone(),
    )
    .await?;

    let listen = config.listen;
    let serve_state = state.clone();
    let http1_handle = tokio::spawn(async move {
        if let Err(err) = http1::serve(listen, serve_state).await {
            error!(error = %err, "HTTP/1.1 server exited");
        }
    });

    shutdown_signal().await;
    info!("received shutdown signal");
    pools.shutdown().await;
    http1_handle.abort();
    if let Some(handle) = http3_handle {
        handle.abort();
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            error!(error = %err, "failed to install Ctrl+C handler");
            // Fall through to pending so we do not exit the process.
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => {
                error!(error = %err, "failed to install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
