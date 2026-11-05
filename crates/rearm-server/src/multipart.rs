use anyhow::{Context, Result};
use axum::extract::Request;
use axum::http::header;
use rearm_protocol::FileEntry;
use futures::StreamExt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

/// Parsed multipart payload ready for the PHP worker protocol.
#[derive(Debug, Default)]
pub struct ParsedMultipart {
    pub form: HashMap<String, String>,
    pub files: Vec<FileEntry>,
    /// Empty body — file bytes live on disk.
    pub body: Vec<u8>,
}

/// Returns true when the request looks like `multipart/form-data`.
pub fn is_multipart(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.to_ascii_lowercase().starts_with("multipart/form-data"))
        .unwrap_or(false)
}

/// Stream multipart parts: text fields into `form`, file parts onto `upload_dir`.
pub async fn parse_multipart(req: Request, upload_dir: &Path) -> Result<ParsedMultipart> {
    fs::create_dir_all(upload_dir)
        .await
        .with_context(|| format!("create upload dir {}", upload_dir.display()))?;

    let boundary = multer::parse_boundary(
        req.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or(""),
    )
    .context("missing multipart boundary")?;

    let stream = req.into_body().into_data_stream();
    let mut multipart = multer::Multipart::new(stream, boundary);

    let mut parsed = ParsedMultipart::default();

    while let Some(field) = multipart.next_field().await.context("multipart field")? {
        let name = field.name().unwrap_or("").to_string();
        let filename = field.file_name().map(|s| s.to_string());
        let content_type = field
            .content_type()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "application/octet-stream".into());

        if let Some(original_name) = filename {
            let dest = unique_upload_path(upload_dir, &original_name);
            let size = stream_field_to_file(field, &dest).await?;
            parsed.files.push(FileEntry {
                field: name,
                path: dest.to_string_lossy().into_owned(),
                original_name,
                mime: content_type,
                size,
            });
        } else {
            let text = field.text().await.context("read form field")?;
            parsed.form.insert(name, text);
        }
    }

    Ok(parsed)
}

fn unique_upload_path(upload_dir: &Path, original_name: &str) -> PathBuf {
    let safe: String = Path::new(original_name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("upload")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
            c
        } else {
            '_'
        })
        .collect();
    upload_dir.join(format!("{}-{}", Uuid::new_v4(), safe))
}

async fn stream_field_to_file(
    mut field: multer::Field<'_>,
    dest: &Path,
) -> Result<u64> {
    let mut file = File::create(dest)
        .await
        .with_context(|| format!("create upload {}", dest.display()))?;
    let mut size = 0u64;
    while let Some(chunk) = field.next().await {
        let chunk = chunk.context("read multipart chunk")?;
        size += chunk.len() as u64;
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(size)
}
