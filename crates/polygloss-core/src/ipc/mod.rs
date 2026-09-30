//! The app socket: protocol types, blocking client and server loop (T4.1, design §13.3).
//!
//! - [`protocol`]: [`Op`], [`Request`], [`Response`], [`IpcError`] (JSON Lines).
//! - [`client`]: [`IpcClient`], used by the CLI, `polygloss mcp` and a second
//!   dev instance of the app.
//! - [`server`]: [`serve`], the app's accept loop on a `std::thread`.
//! - [`socket_path`]: `<data_dir>/polygloss.sock`, or a per-user `$TMPDIR`
//!   fallback when that is longer than macOS's `sun_path` (design §13.2).

pub mod client;
pub mod protocol;
pub mod server;

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

pub use client::IpcClient;
pub use protocol::{
    IpcError, MAX_LINE_BYTES, OP_NAMES, Op, PROTOCOL_VERSION, Request, Response, TEST_ENV, codes,
    parse_request,
};
pub use server::{ServerConfig, ServerHandle, serve, serve_at, serve_with};

use crate::paths::{DataPaths, socket_path_fits};

/// Where the app listens: [`DataPaths::socket`] when it fits in `sun_path`
/// (104 bytes), else `$TMPDIR/polygloss-<uid>/<hash>.sock` (see
/// [`socket_path_with`]).
pub fn socket_path(paths: &DataPaths) -> PathBuf {
    socket_path_with(
        paths,
        std::env::var_os("TMPDIR").map(PathBuf::from).as_deref(),
    )
}

/// [`socket_path`] with an explicit `$TMPDIR`. The fallback lives in a
/// per-user directory `<tmpdir>/polygloss-<uid>` (`/tmp` when `tmpdir` is
/// unset, relative or itself too long) that the server creates with mode
/// `0700`. The file name is `polygloss-<16 hex of sha256(data_dir)>.sock`, so
/// two data dirs that both need the fallback never share a socket.
pub fn socket_path_with(paths: &DataPaths, tmpdir: Option<&Path>) -> PathBuf {
    if socket_path_fits(&paths.socket) {
        return paths.socket.clone();
    }
    let hash = Sha256::digest(paths.data_dir.as_os_str().as_encoded_bytes());
    let name = format!("polygloss-{}.sock", &hex::encode(hash)[..16]);
    let dir = format!("polygloss-{}", current_uid());
    let in_tmpdir = tmpdir
        .filter(|t| t.is_absolute())
        .map(|t| t.join(&dir).join(&name))
        .filter(|p| socket_path_fits(p));
    in_tmpdir.unwrap_or_else(|| Path::new("/tmp").join(dir).join(name))
}

/// This process's effective uid.
#[allow(unsafe_code)]
pub fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, cannot fail and touches no memory.
    unsafe { libc::geteuid() }
}
