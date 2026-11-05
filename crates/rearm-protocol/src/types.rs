use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Metadata for a multipart file uploaded to disk by the Rust server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub field: String,
    pub path: String,
    pub original_name: String,
    pub mime: String,
    pub size: u64,
}

/// Request frame sent from the Rust server to a PHP worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestFrame {
    pub method: String,
    pub uri: String,
    pub headers: HashMap<String, String>,
    pub remote_addr: String,
    /// Raw body bytes for non-multipart requests.
    #[serde(with = "serde_bytes_compat")]
    pub body: Vec<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileEntry>>,
    /// Optional control command, e.g. `"shutdown"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
}

impl RequestFrame {
    /// Build a graceful shutdown control frame.
    pub fn shutdown() -> Self {
        Self {
            method: String::new(),
            uri: String::new(),
            headers: HashMap::new(),
            remote_addr: String::new(),
            body: Vec::new(),
            form: None,
            files: None,
            control: Some("shutdown".into()),
        }
    }
}

/// Response frame returned by a PHP worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseFrame {
    pub status: u16,
    pub headers: HashMap<String, String>,
    #[serde(with = "serde_bytes_compat")]
    pub body: Vec<u8>,
}

/// Serialize `Vec<u8>` as MessagePack bin; accept bin or array on decode.
mod serde_bytes_compat {
    use serde::de::{Error, SeqAccess, Visitor};
    use serde::{Deserializer, Serializer};
    use std::fmt;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BytesVisitor;

        impl<'de> Visitor<'de> for BytesVisitor {
            type Value = Vec<u8>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a byte array")
            }

            fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Ok(v.to_vec())
            }

            fn visit_byte_buf<E>(self, v: Vec<u8>) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Ok(v)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    out.push(b);
                }
                Ok(out)
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: Error,
            {
                Ok(v.as_bytes().to_vec())
            }
        }

        deserializer.deserialize_any(BytesVisitor)
    }
}
