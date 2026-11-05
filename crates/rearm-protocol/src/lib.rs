//! Rearm wire protocol: MessagePack payloads with a 4-byte big-endian length prefix.
//!
//! Frame layout: `[u32 BE length][msgpack body]`

mod error;
mod frame;
mod types;

pub use error::ProtocolError;
pub use frame::{decode_frame, encode_frame, split_length_prefix, FrameReader, MAX_FRAME_SIZE};
pub use types::{FileEntry, RequestFrame, ResponseFrame};

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn request_roundtrip() {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "application/json".into());
        headers.insert("host".into(), "example.com".into());

        let mut form = HashMap::new();
        form.insert("name".into(), "rearm".into());

        let files = vec![FileEntry {
            field: "avatar".into(),
            path: "/tmp/rearm-uploads/abc".into(),
            original_name: "photo.png".into(),
            mime: "image/png".into(),
            size: 1234,
        }];

        let req = RequestFrame {
            method: "POST".into(),
            uri: "/api/upload?x=1".into(),
            headers,
            remote_addr: "127.0.0.1:54321".into(),
            body: b"{\"ok\":true}".to_vec(),
            form: Some(form),
            files: Some(files),
            control: None,
        };

        let encoded = encode_frame(&req).expect("encode");
        let (len, payload) = split_length_prefix(&encoded).expect("split");
        assert_eq!(len as usize, payload.len());
        assert_eq!(encoded.len(), 4 + payload.len());

        let decoded: RequestFrame = decode_frame(payload).expect("decode");
        assert_eq!(decoded.method, "POST");
        assert_eq!(decoded.uri, "/api/upload?x=1");
        assert_eq!(decoded.remote_addr, "127.0.0.1:54321");
        assert_eq!(decoded.body, b"{\"ok\":true}");
        assert_eq!(
            decoded.form.as_ref().unwrap().get("name").unwrap(),
            "rearm"
        );
        let f = &decoded.files.as_ref().unwrap()[0];
        assert_eq!(f.field, "avatar");
        assert_eq!(f.size, 1234);
        assert!(decoded.control.is_none());
    }

    #[test]
    fn response_roundtrip() {
        let mut headers = HashMap::new();
        headers.insert("content-type".into(), "text/plain".into());

        let resp = ResponseFrame {
            status: 200,
            headers,
            body: b"hello".to_vec(),
        };

        let encoded = encode_frame(&resp).expect("encode");
        let (_, payload) = split_length_prefix(&encoded).expect("split");
        let decoded: ResponseFrame = decode_frame(payload).expect("decode");
        assert_eq!(decoded.status, 200);
        assert_eq!(decoded.body, b"hello");
        assert_eq!(
            decoded.headers.get("content-type").unwrap(),
            "text/plain"
        );
    }

    #[test]
    fn shutdown_control_frame() {
        let req = RequestFrame::shutdown();
        let encoded = encode_frame(&req).expect("encode");
        let (_, payload) = split_length_prefix(&encoded).expect("split");
        let decoded: RequestFrame = decode_frame(payload).expect("decode");
        assert_eq!(decoded.control.as_deref(), Some("shutdown"));
    }

    #[test]
    fn frame_reader_assembles_partial_reads() {
        let resp = ResponseFrame {
            status: 204,
            headers: HashMap::new(),
            body: Vec::new(),
        };
        let encoded = encode_frame(&resp).expect("encode");

        let mut reader = FrameReader::new();
        let mut got = None;
        let mut offset = 0;
        while offset < encoded.len() {
            let end = (offset + 3).min(encoded.len());
            if let Some(frame) = reader.push(&encoded[offset..end]).unwrap() {
                got = Some(frame);
                break;
            }
            offset = end;
        }
        let frame = got.expect("full frame");
        let decoded: ResponseFrame = decode_frame(&frame).expect("decode");
        assert_eq!(decoded.status, 204);
    }

    #[test]
    fn empty_body_roundtrip() {
        let req = RequestFrame {
            method: "GET".into(),
            uri: "/".into(),
            headers: HashMap::new(),
            remote_addr: "10.0.0.1:80".into(),
            body: Vec::new(),
            form: None,
            files: None,
            control: None,
        };
        let encoded = encode_frame(&req).unwrap();
        let (_, payload) = split_length_prefix(&encoded).unwrap();
        let decoded: RequestFrame = decode_frame(payload).unwrap();
        assert!(decoded.body.is_empty());
        assert!(decoded.form.is_none());
        assert!(decoded.files.is_none());
    }

    #[test]
    fn truncated_header_errors() {
        let err = split_length_prefix(&[0x00, 0x01]).unwrap_err();
        assert!(matches!(err, ProtocolError::TruncatedHeader));
    }

    #[test]
    fn incomplete_frame_errors() {
        let mut buf = vec![0, 0, 0, 10];
        buf.extend_from_slice(b"short");
        let err = split_length_prefix(&buf).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::Incomplete { have: 9, need: 14 }
        ));
    }

    #[test]
    fn frame_reader_rejects_oversized_declared_length() {
        let mut reader = FrameReader::with_max_frame_size(8);
        // length prefix = 100 bytes payload
        let err = reader
            .push(&(100u32.to_be_bytes()))
            .expect_err("declared length too large");
        assert!(matches!(err, ProtocolError::FrameTooLarge(100)));
    }

    #[test]
    fn binary_body_roundtrip() {
        let body: Vec<u8> = (0u8..=255).collect();
        let req = RequestFrame {
            method: "PUT".into(),
            uri: "/bin".into(),
            headers: HashMap::new(),
            remote_addr: "127.0.0.1:1".into(),
            body: body.clone(),
            form: None,
            files: None,
            control: None,
        };
        let encoded = encode_frame(&req).unwrap();
        let (_, payload) = split_length_prefix(&encoded).unwrap();
        let decoded: RequestFrame = decode_frame(payload).unwrap();
        assert_eq!(decoded.body, body);
    }

    #[test]
    fn empty_form_map_roundtrip() {
        let req = RequestFrame {
            method: "POST".into(),
            uri: "/form".into(),
            headers: HashMap::new(),
            remote_addr: "127.0.0.1:1".into(),
            body: Vec::new(),
            form: Some(HashMap::new()),
            files: Some(Vec::new()),
            control: None,
        };
        let encoded = encode_frame(&req).unwrap();
        let (_, payload) = split_length_prefix(&encoded).unwrap();
        let decoded: RequestFrame = decode_frame(payload).unwrap();
        assert_eq!(decoded.form.as_ref().map(|m| m.is_empty()), Some(true));
        assert_eq!(decoded.files.as_ref().map(|f| f.is_empty()), Some(true));
    }
}
