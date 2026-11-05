use crate::compress::{apply_encoding_headers, maybe_compress};
use crate::config::{CompressMode, Config};
use crate::multipart::{self, is_multipart};
use crate::pool::PoolSet;
use crate::static_files;
use axum::body::Body;
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use rearm_protocol::RequestFrame;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::error;

/// Shared application state for HTTP/1.1 and HTTP/3.
#[derive(Clone)]
pub struct AppState {
    pub pools: PoolSet,
    pub config: Arc<Config>,
}

pub async fn handle_request(
    state: AppState,
    remote_addr: String,
    req: Request<Body>,
) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();

    if method == Method::GET && path == "/__rearm/health" {
        return health_response();
    }

    if method == Method::GET && path == "/__rearm/metrics" {
        return metrics_response(state, req.uri().query()).await;
    }

    if let Some(static_res) =
        static_files::maybe_static_response(&state.config.public_dir(), &method, &path).await
    {
        return static_res;
    }

    match dispatch_php(state, remote_addr, req).await {
        Ok(res) => res,
        Err(err) => {
            error!(error = %err, "request failed");
            (
                StatusCode::BAD_GATEWAY,
                [("content-type", "text/plain; charset=utf-8")],
                format!("bad gateway: {err}"),
            )
                .into_response()
        }
    }
}

fn health_response() -> Response {
    let body = serde_json::json!({
        "status": "ok",
        "service": "rearm-server",
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn metrics_response(state: AppState, query: Option<&str>) -> Response {
    let q = query.unwrap_or("");
    let include_workers = !query_flag(q, "workers", "0");
    let as_text = query_flag(q, "format", "text");

    let value = state.pools.metrics_json(include_workers).await;
    if as_text {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            PoolSet::metrics_text(&value),
        )
            .into_response();
    }

    let body = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

/// True when query string contains `key=value` (exact pair).
fn query_flag(query: &str, key: &str, value: &str) -> bool {
    let needle = format!("{key}={value}");
    query.split('&').any(|part| part == needle)
}

async fn dispatch_php(
    state: AppState,
    remote_addr: String,
    req: Request<Body>,
) -> anyhow::Result<Response> {
    let (parts, body) = req.into_parts();
    let method = parts.method.as_str().to_string();
    let route_path = parts.uri.path().to_string();
    let uri = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str().to_string())
        .unwrap_or_else(|| route_path.clone());

    let headers_map = header_map_to_string_map(&parts.headers);
    let request_headers = parts.headers.clone();

    let frame = if is_multipart(&parts.headers) {
        let reconstructed = Request::from_parts(parts, body);
        let parsed = multipart::parse_multipart(reconstructed, &state.config.upload_dir).await?;
        RequestFrame {
            method,
            uri,
            headers: headers_map,
            remote_addr,
            body: parsed.body,
            form: Some(parsed.form).filter(|m| !m.is_empty()),
            files: Some(parsed.files).filter(|f| !f.is_empty()),
            control: None,
        }
    } else {
        let bytes = axum::body::to_bytes(body, 64 * 1024 * 1024).await?;
        RequestFrame {
            method,
            uri,
            headers: headers_map,
            remote_addr,
            body: bytes.to_vec(),
            form: None,
            files: None,
            control: None,
        }
    };

    let mut php_response = state.pools.forward(&route_path, frame).await?;

    // Worker is already released. Park the HTTP client in chunked sleeps so a
    // long wait is "sleep again" without dropping the connection.
    if let Some(total) = take_defer_ms(&mut php_response.headers, state.config.defer_max_ms) {
        park_client(total, state.config.defer_chunk_ms_effective()).await;
    }
    Ok(php_response_to_http(
        state.config.compress,
        &request_headers,
        php_response,
    ))
}

/// Hold the client connection for `total_ms`, sleeping in `chunk_ms` slices.
async fn park_client(total_ms: u64, chunk_ms: u64) {
    let chunk = chunk_ms.max(1);
    let mut left = total_ms;
    while left > 0 {
        let step = left.min(chunk);
        tokio::time::sleep(std::time::Duration::from_millis(step)).await;
        left -= step;
    }
}

/// Pull `X-Rearm-Defer-Ms` (internal) out of the PHP response headers.
/// Caps at `defer_max_ms`. Returns `None` when absent / invalid.
fn take_defer_ms(headers: &mut HashMap<String, String>, defer_max_ms: u64) -> Option<u64> {
    const NAME: &str = "x-rearm-defer-ms";
    let key = headers.keys().find(|k| k.eq_ignore_ascii_case(NAME))?.clone();
    let raw = headers.remove(&key)?;
    let ms: u64 = raw.trim().parse().ok()?;
    if ms == 0 {
        return None;
    }
    Some(ms.min(defer_max_ms.max(1)))
}

fn header_map_to_string_map(headers: &HeaderMap) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for (name, value) in headers.iter() {
        if let Ok(v) = value.to_str() {
            map.insert(name.as_str().to_string(), v.to_string());
        }
    }
    map
}

fn php_response_to_http(
    compress: CompressMode,
    request_headers: &HeaderMap,
    php: rearm_protocol::ResponseFrame,
) -> Response {
    let mut headers = HeaderMap::new();
    for (k, v) in &php.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            let lower = k.to_ascii_lowercase();
            if lower == "transfer-encoding" || lower == "connection" {
                continue;
            }
            headers.append(name, value);
        }
    }

    let (body, encoding) = maybe_compress(compress, request_headers, &headers, php.body);
    apply_encoding_headers(&mut headers, encoding);
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&body.len().to_string()).unwrap_or(HeaderValue::from_static("0")),
    );
    // Explicit keep-alive helps wrk/HTTP clients reuse connections cleanly.
    headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    headers.insert(
        HeaderName::from_static("keep-alive"),
        HeaderValue::from_static("timeout=60"),
    );

    let status = StatusCode::from_u16(php.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    *res.headers_mut() = headers;
    res
}

/// Axum adapter; peer address comes from connect-info extensions when present.
pub async fn axum_entry(State(state): State<AppState>, req: Request<Body>) -> Response {
    let remote = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.to_string())
        .unwrap_or_else(|| "0.0.0.0:0".into());
    handle_request(state, remote, req).await
}
