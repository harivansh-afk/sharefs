//! The request shared by the web API and the local `share` client.

use serde::{Deserialize, Serialize};

pub const DEFAULT_EXPIRY: u64 = 7 * 24 * 60 * 60;
pub const MAX_MESSAGE_BYTES: u64 = 16 * 1024;
pub const DEFAULT_SOCKET: &str = "/run/sharefs/control.sock";

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ShareRequest {
    pub path: String,
    #[serde(default = "default_expiry")]
    pub expires_in: u64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ShareLink {
    pub url: String,
    pub expires_at: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ShareReply {
    Link(ShareLink),
    Error { error: String },
}

fn default_expiry() -> u64 {
    DEFAULT_EXPIRY
}

/// A positive integer followed by s, m, h, d or w. Bare integers mean seconds.
pub fn parse_expiry(value: &str) -> Result<u64, String> {
    let (number, scale) = match value.as_bytes().last() {
        Some(b's') => (&value[..value.len() - 1], 1),
        Some(b'm') => (&value[..value.len() - 1], 60),
        Some(b'h') => (&value[..value.len() - 1], 60 * 60),
        Some(b'd') => (&value[..value.len() - 1], 24 * 60 * 60),
        Some(b'w') => (&value[..value.len() - 1], 7 * 24 * 60 * 60),
        _ => (value, 1),
    };

    if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) {
        if let Some(seconds) = number
            .parse::<u64>()
            .ok()
            .and_then(|number| number.checked_mul(scale))
            .filter(|seconds| *seconds > 0)
        {
            return Ok(seconds);
        }
    }

    Err("expected a positive duration such as 30m, 2h or 7d".into())
}
