use crate::handler::{axum_entry, AppState};
use anyhow::Result;
use axum::routing::any;
use axum::Router;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tracing::info;

/// Build the shared Axum router used by HTTP/1.1 (and mirrored by HTTP/3 logic).
pub fn router(state: AppState) -> Router {
    Router::new()
        .fallback(any(axum_entry))
        .with_state(state)
}

/// Serve HTTP/1.1 on `listen`.
pub async fn serve(listen: SocketAddr, state: AppState) -> Result<()> {
    let app = router(state);
    let listener = TcpListener::bind(listen).await?;
    info!(%listen, "HTTP/1.1 listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
