//! Write committed cross-compat fixtures for PHPUnit.
//!
//! ```bash
//! cargo run -p rearm-protocol --example write_fixtures
//! ```

use rearm_protocol::{encode_frame, FileEntry, RequestFrame, ResponseFrame};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../packages/octane-driver/tests/fixtures")
        });

    fs::create_dir_all(&out).expect("create fixtures dir");

    let mut headers = HashMap::new();
    headers.insert("content-type".into(), "application/json".into());
    headers.insert("x-rearm-test".into(), "1".into());

    let mut form = HashMap::new();
    form.insert("name".into(), "rearm".into());

    let req = RequestFrame {
        method: "POST".into(),
        uri: "/api/upload?x=1".into(),
        headers,
        remote_addr: "127.0.0.1:54321".into(),
        body: b"{\"ok\":true}".to_vec(),
        form: Some(form),
        files: Some(vec![FileEntry {
            field: "avatar".into(),
            path: "/tmp/rearm-uploads/abc".into(),
            original_name: "photo.png".into(),
            mime: "image/png".into(),
            size: 1234,
        }]),
        control: None,
    };

    let mut resp_headers = HashMap::new();
    resp_headers.insert("content-type".into(), "text/plain".into());
    let resp = ResponseFrame {
        status: 200,
        headers: resp_headers,
        body: b"hello".to_vec(),
    };

    let shutdown = RequestFrame::shutdown();

    fs::write(out.join("request.bin"), encode_frame(&req).unwrap()).unwrap();
    fs::write(out.join("response.bin"), encode_frame(&resp).unwrap()).unwrap();
    fs::write(out.join("shutdown.bin"), encode_frame(&shutdown).unwrap()).unwrap();

    println!("wrote fixtures to {}", out.display());
}
