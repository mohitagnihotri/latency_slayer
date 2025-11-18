use crate::types::Message;
use sha2::{Digest, Sha256};

pub fn canonicalize(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| format!("{}:{}", m.role.trim(), m.content.trim()))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

pub fn fingerprint(prompt: &str, model: &str, route: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update(model.as_bytes());
    hasher.update(route.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn normalize_tag(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

pub fn f32s_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_ne_bytes());
    }
    out
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
