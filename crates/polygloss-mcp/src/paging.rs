//! Pagination for every agent list (design §15.1 "Size"): opaque cursors and a
//! page budget of about 60k characters, so one page stays well under Claude Code's
//! default 25k-token tool output cap.
//!
//! A cursor is base64url (no padding) of `{"v":1,"c":<Cursor>,"h":<check>}`, where
//! `check` is the first 16 hex digits of SHA-256 over the cursor's JSON. The check
//! catches edited or truncated cursors; it is not a secret (a cursor only ever
//! names a position in a list the caller may read anyway), so the JSON CLI can
//! continue a page an MCP call started and vice versa.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::errors::ApiError;

/// The page budget in characters of serialized JSON (design §15.1).
pub const PAGE_MAX_CHARS: usize = 60_000;

/// Cursor format version.
const CURSOR_VERSION: u32 = 1;

/// A position in one paginated list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Which list it continues: `reviews`, `threads`, …
    pub list: String,
    /// A fingerprint of the query (scope and filters). A cursor is only valid for
    /// the query that produced it ([`Cursor::check_query`]).
    pub query: String,
    /// The list-specific keyset position after which the next page starts.
    pub after: Value,
}

impl Cursor {
    /// A cursor continuing `list` for `query` after `after`.
    pub fn new(
        list: impl Into<String>,
        query: impl Into<String>,
        after: &impl Serialize,
    ) -> Result<Cursor, ApiError> {
        Ok(Cursor {
            list: list.into(),
            query: query.into(),
            after: serde_json::to_value(after)
                .map_err(|e| ApiError::internal(format!("encoding cursor: {e}")))?,
        })
    }

    /// `Conflict` unless this cursor belongs to `list` and `query`.
    pub fn check_query(&self, list: &str, query: &str) -> Result<(), ApiError> {
        if self.list == list && self.query == query {
            Ok(())
        } else {
            Err(ApiError::conflict(
                "cursor belongs to a different list or filters; start again without a cursor",
            ))
        }
    }

    /// The keyset position as `T`.
    pub fn after_as<T: DeserializeOwned>(&self) -> Result<T, ApiError> {
        serde_json::from_value(self.after.clone()).map_err(|_| invalid_cursor())
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    v: u32,
    c: Cursor,
    h: String,
}

fn check_of(c: &Cursor) -> String {
    // serde_json writes struct fields in declaration order and `Value` objects in
    // key order, so the encoding is canonical for a given cursor.
    let json = serde_json::to_vec(c).unwrap_or_default();
    let digest = Sha256::digest(&json);
    hex_prefix(&digest, 8)
}

fn hex_prefix(bytes: &[u8], n: usize) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(n * 2);
    for b in bytes.iter().take(n) {
        s.push(char::from(HEX[usize::from(b >> 4)]));
        s.push(char::from(HEX[usize::from(b & 0xf)]));
    }
    s
}

fn invalid_cursor() -> ApiError {
    ApiError::conflict("invalid cursor; pass next_cursor exactly as returned")
}

/// Encodes `c` as an opaque cursor string.
pub fn encode_cursor(c: &Cursor) -> String {
    let env = Envelope {
        v: CURSOR_VERSION,
        c: c.clone(),
        h: check_of(c),
    };
    let json = serde_json::to_vec(&env).unwrap_or_default();
    base64url_encode(&json)
}

/// Decodes a cursor from [`encode_cursor`]. An edited, truncated or foreign string
/// is `Conflict` ("invalid cursor").
pub fn decode_cursor(s: &str) -> Result<Cursor, ApiError> {
    let bytes = base64url_decode(s.trim()).ok_or_else(invalid_cursor)?;
    let env: Envelope = serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
    if env.v != CURSOR_VERSION || env.h != check_of(&env.c) {
        return Err(invalid_cursor());
    }
    Ok(env.c)
}

/// The longest prefix of `items` whose JSON array encoding stays within
/// `max_chars` characters, and whether any item was dropped. The first item is
/// always kept, so a caller paging with the returned length always makes
/// progress; items are bounded (excerpts ≤ 300 chars, bodies ≤ 20k) so one item
/// never approaches the budget.
pub fn fit_page<T: Serialize>(items: Vec<T>, max_chars: usize) -> (Vec<T>, bool) {
    let mut used = 2; // `[` and `]`
    let mut keep = 0;
    for (i, item) in items.iter().enumerate() {
        let len = serde_json::to_string(item)
            .map(|s| s.chars().count())
            .unwrap_or(0);
        let sep = usize::from(i > 0);
        if i > 0 && used + sep + len > max_chars {
            break;
        }
        used += sep + len;
        keep = i + 1;
    }
    let truncated = keep < items.len();
    let mut items = items;
    items.truncate(keep);
    (items, truncated)
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// RFC 4648 §5 base64url without padding.
pub fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let chars = chunk.len() + 1;
        for k in 0..chars {
            let idx = (n >> (18 - 6 * k)) & 0x3f;
            out.push(char::from(B64[idx as usize]));
        }
    }
    out
}

/// Inverse of [`base64url_encode`]; `None` for characters outside the alphabet,
/// padding, or an impossible length.
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        B64.iter().position(|&b| b == c).map(|p| p as u32)
    }
    let bytes = s.as_bytes();
    if bytes.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (k, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * k);
        }
        let produced = chunk.len() - 1;
        for k in 0..produced {
            out.push(((n >> (16 - 8 * k)) & 0xff) as u8);
        }
        // Non-canonical trailing bits mean the string was edited.
        let used_bits = 8 * produced;
        let total_bits = 6 * chunk.len();
        let spare = total_bits - used_bits;
        if spare > 0 {
            let mask = (1u32 << spare) - 1;
            let tail = n >> (24 - total_bits);
            if tail & mask != 0 {
                return None;
            }
        }
    }
    Some(out)
}
