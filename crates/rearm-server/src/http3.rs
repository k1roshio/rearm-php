use crate::handler::{handle_request, AppState};
use anyhow::{Context, Result};
use axum::body::Body;
use axum::http::{header, HeaderValue, Request, Response};
use bytes::{Buf, Bytes};
use http_body_util::BodyExt;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tracing::{error, info, warn};

/// Start an HTTP/3 listener when explicitly enabled and cert+key are present.
pub async fn maybe_serve(
    enabled: bool,
    listen: SocketAddr,
    cert: Option<&Path>,
    key: Option<&Path>,
    state: AppState,
) -> Result<Option<tokio::task::JoinHandle<()>>> {
    if !enabled {
        info!("HTTP/3 disabled (REARM_HTTP3=false / --http3 not set)");
        return Ok(None);
    }

    let (Some(cert_path), Some(key_path)) = (cert, key) else {
        warn!("HTTP/3 enabled but inactive: --cert and --key not both provided");
        return Ok(None);
    };

    if !cert_path.exists() || !key_path.exists() {
        warn!(
            cert = %cert_path.display(),
            key = %key_path.display(),
            "HTTP/3 disabled: certificate or key file missing"
        );
        return Ok(None);
    }

    let server_config =
        build_server_config(cert_path, key_path).context("build HTTP/3 TLS config")?;
    let endpoint =
        Endpoint::server(server_config, listen).context("bind HTTP/3 endpoint")?;
    info!(%listen, "HTTP/3 listening");

    let handle = tokio::spawn(async move {
        loop {
            let Some(incoming) = endpoint.accept().await else {
                break;
            };
            let state = state.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(incoming, state).await {
                    error!(error = %err, "HTTP/3 connection error");
                }
            });
        }
    });

    Ok(Some(handle))
}

fn build_server_config(cert_path: &Path, key_path: &Path) -> Result<ServerConfig> {
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;

    let mut tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("rustls single cert")?;
    tls.alpn_protocols = vec![b"h3".to_vec()];
    tls.max_early_data_size = u32::MAX;

    let quic_tls = QuicServerConfig::try_from(tls).context("quic server config")?;
    let mut server = ServerConfig::with_crypto(Arc::new(quic_tls));
    server.transport = Arc::new(quinn::TransportConfig::default());
    Ok(server)
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = File::open(path).with_context(|| format!("open cert {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse certs")?;
    if certs.is_empty() {
        anyhow::bail!("no certificates in {}", path.display());
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = File::open(path).with_context(|| format!("open key {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let keys: Vec<_> = rustls_pemfile::pkcs8_private_keys(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse pkcs8 keys")?;
    if let Some(key) = keys.into_iter().next() {
        return Ok(PrivateKeyDer::Pkcs8(key));
    }

    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let keys: Vec<_> = rustls_pemfile::rsa_private_keys(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse rsa keys")?;
    if let Some(key) = keys.into_iter().next() {
        return Ok(PrivateKeyDer::Pkcs1(key));
    }

    anyhow::bail!("no private key found in {}", path.display())
}

async fn handle_connection(incoming: quinn::Incoming, state: AppState) -> Result<()> {
    let conn = incoming.await.context("accept quic connection")?;
    let remote = conn.remote_address().to_string();

    let mut h3_conn = h3::server::Connection::new(h3_quinn::Connection::new(conn))
        .await
        .context("h3 handshake")?;

    loop {
        match h3_conn.accept().await {
            Ok(Some(resolver)) => {
                let (req, stream) = resolver
                    .resolve_request()
                    .await
                    .context("resolve h3 request")?;
                let state = state.clone();
                let remote = remote.clone();
                tokio::spawn(async move {
                    if let Err(err) = handle_h3_request(state, remote, req, stream).await {
                        error!(error = %err, "HTTP/3 request error");
                    }
                });
            }
            Ok(None) => break,
            Err(err) => {
                warn!(error = %err, "HTTP/3 accept ended");
                break;
            }
        }
    }
    Ok(())
}

async fn handle_h3_request<T>(
    state: AppState,
    remote: String,
    req: Request<()>,
    mut stream: h3::server::RequestStream<T, Bytes>,
) -> Result<()>
where
    T: h3::quic::RecvStream + h3::quic::SendStream<Bytes>,
{
    let (parts, _) = req.into_parts();
    let mut body_bytes = Vec::new();
    while let Some(mut chunk) = stream.recv_data().await.context("recv h3 body")? {
        let bytes = chunk.copy_to_bytes(chunk.remaining());
        body_bytes.extend_from_slice(&bytes);
    }

    let axum_req = Request::from_parts(parts, Body::from(body_bytes));
    let response = handle_request(state, remote, axum_req).await;
    send_h3_response(&mut stream, response).await
}

async fn send_h3_response<T>(
    stream: &mut h3::server::RequestStream<T, Bytes>,
    response: Response<Body>,
) -> Result<()>
where
    T: h3::quic::RecvStream + h3::quic::SendStream<Bytes>,
{
    let (parts, body) = response.into_parts();
    let collected = body
        .collect()
        .await
        .context("collect response body")?
        .to_bytes();

    let mut h3_res = Response::from_parts(parts, ());
    h3_res.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&collected.len().to_string())
            .unwrap_or_else(|_| HeaderValue::from_static("0")),
    );

    stream
        .send_response(h3_res)
        .await
        .context("send h3 response headers")?;
    if !collected.is_empty() {
        stream
            .send_data(collected)
            .await
            .context("send h3 response body")?;
    }
    stream.finish().await.context("finish h3 response")?;
    Ok(())
}
