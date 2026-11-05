use axum::body::Body;
use axum::http::{header, HeaderValue, Method, Response, StatusCode};
use axum::response::IntoResponse;
use std::path::{Component, Path, PathBuf};
use tokio::fs;
use tracing::debug;

/// Resolve a safe path under `public_dir` for `uri_path`, or `None` if not a static asset.
pub async fn try_serve(public_dir: &Path, uri_path: &str) -> Option<Response<Body>> {
    let rel = normalize_uri_path(uri_path)?;
    if rel.as_os_str().is_empty() {
        return None;
    }

    let candidate = public_dir.join(&rel);
    let meta = match fs::metadata(&candidate).await {
        Ok(m) if m.is_file() => m,
        _ => return None,
    };
    let _ = meta;

    let canonical = fs::canonicalize(&candidate).await.ok()?;
    let root = fs::canonicalize(public_dir).await.ok()?;
    if !canonical.starts_with(&root) {
        return None;
    }

    let bytes = fs::read(&canonical).await.ok()?;
    let mime = guess_mime(&canonical);
    debug!(path = %canonical.display(), "serving static file");

    let mut res = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .body(Body::from(bytes))
        .ok()?;

    res.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=0"),
    );
    Some(res)
}

fn normalize_uri_path(uri_path: &str) -> Option<PathBuf> {
    let path = uri_path.split('?').next().unwrap_or(uri_path);
    let path = percent_decode(path);
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return None;
    }

    let mut out = PathBuf::new();
    for comp in Path::new(path).components() {
        match comp {
            Component::Normal(seg) => out.push(seg),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn guess_mime(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

/// Serve a static asset for GET/HEAD when the file exists under `public_dir`.
pub async fn maybe_static_response(
    public_dir: &Path,
    method: &Method,
    path: &str,
) -> Option<axum::response::Response> {
    if *method != Method::GET && *method != Method::HEAD {
        return None;
    }
    try_serve(public_dir, path)
        .await
        .map(IntoResponse::into_response)
}
