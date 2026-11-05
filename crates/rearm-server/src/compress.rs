use crate::config::CompressMode;
use axum::http::{header, HeaderMap, HeaderValue};
use brotli::enc::BrotliEncoderParams;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::io::Write;

/// Bodies smaller than this are left uncompressed.
pub const MIN_COMPRESS_BYTES: usize = 256;

/// Compress `body` according to CLI preference and the request `Accept-Encoding`.
///
/// Returns `(body, optional Content-Encoding value)`.
pub fn maybe_compress(
    mode: CompressMode,
    request_headers: &HeaderMap,
    response_headers: &HeaderMap,
    body: Vec<u8>,
) -> (Vec<u8>, Option<&'static str>) {
    if mode == CompressMode::Off || body.len() < MIN_COMPRESS_BYTES {
        return (body, None);
    }

    if response_headers.contains_key(header::CONTENT_ENCODING) {
        return (body, None);
    }

    if let Some(ct) = response_headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        let ct = ct.to_ascii_lowercase();
        if ct.contains("image/")
            || ct.contains("video/")
            || ct.contains("audio/")
            || ct.contains("zip")
            || ct.contains("gzip")
            || ct.contains("octet-stream")
        {
            return (body, None);
        }
    }

    let accept = request_headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();

    let wants_br = accept.contains("br");
    let wants_gzip = accept.contains("gzip");

    let try_br = || {
        brotli_compress(&body).ok().and_then(|out| {
            if out.len() < body.len() {
                Some((out, "br"))
            } else {
                None
            }
        })
    };
    let try_gzip = || {
        gzip_compress(&body).ok().and_then(|out| {
            if out.len() < body.len() {
                Some((out, "gzip"))
            } else {
                None
            }
        })
    };

    let preferred = match mode {
        CompressMode::Off => None,
        CompressMode::Brotli if wants_br => try_br().or_else(|| wants_gzip.then(|| try_gzip()).flatten()),
        CompressMode::Brotli if wants_gzip => try_gzip(),
        CompressMode::Gzip if wants_gzip => try_gzip().or_else(|| wants_br.then(|| try_br()).flatten()),
        CompressMode::Gzip if wants_br => try_br(),
        _ => None,
    };

    match preferred {
        Some((out, enc)) => (out, Some(enc)),
        None => (body, None),
    }
}

fn brotli_compress(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut params = BrotliEncoderParams::default();
    params.quality = 4;
    let mut out = Vec::with_capacity(input.len() / 2 + 64);
    {
        let mut writer = brotli::CompressorWriter::with_params(&mut out, 4096, &params);
        writer.write_all(input)?;
        writer.flush()?;
    }
    Ok(out)
}

fn gzip_compress(input: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(input.len() / 2 + 64), Compression::fast());
    encoder.write_all(input)?;
    encoder.finish()
}

/// Apply `Content-Encoding` / `Vary` when compression was applied.
pub fn apply_encoding_headers(headers: &mut HeaderMap, encoding: Option<&str>) {
    let Some(enc) = encoding else {
        return;
    };
    if let Ok(v) = HeaderValue::from_str(enc) {
        headers.insert(header::CONTENT_ENCODING, v);
    }
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    headers.remove(header::CONTENT_LENGTH);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_tiny_bodies() {
        let req = HeaderMap::new();
        let resp = HeaderMap::new();
        let (out, enc) = maybe_compress(CompressMode::Brotli, &req, &resp, b"tiny".to_vec());
        assert_eq!(out, b"tiny");
        assert!(enc.is_none());
    }

    #[test]
    fn compresses_when_accepted() {
        let mut req = HeaderMap::new();
        req.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, br"),
        );
        let resp = HeaderMap::new();
        let body = vec![b'a'; 4096];
        let (out, enc) = maybe_compress(CompressMode::Brotli, &req, &resp, body.clone());
        assert!(enc.is_some());
        assert!(out.len() < body.len());
    }

    #[test]
    fn off_mode_never_compresses() {
        let mut req = HeaderMap::new();
        req.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip"),
        );
        let resp = HeaderMap::new();
        let body = vec![b'x'; 4096];
        let (out, enc) = maybe_compress(CompressMode::Off, &req, &resp, body.clone());
        assert_eq!(out, body);
        assert!(enc.is_none());
    }

    #[test]
    fn skips_already_encoded_or_binary_types() {
        let mut req = HeaderMap::new();
        req.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br"),
        );
        let mut resp = HeaderMap::new();
        resp.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("image/png"),
        );
        let body = vec![b'x'; 4096];
        let (out, enc) = maybe_compress(CompressMode::Brotli, &req, &resp, body.clone());
        assert_eq!(out, body);
        assert!(enc.is_none());
    }

    #[test]
    fn apply_encoding_headers_sets_vary() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("10"));
        apply_encoding_headers(&mut headers, Some("br"));
        assert_eq!(
            headers.get(header::CONTENT_ENCODING).unwrap(),
            "br"
        );
        assert_eq!(headers.get(header::VARY).unwrap(), "accept-encoding");
        assert!(!headers.contains_key(header::CONTENT_LENGTH));
    }
}
