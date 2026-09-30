//! Small process queries for `polygloss wait` (T4.8, design §16.3, §16.4): the
//! executable of a pid (to check that a replaced waiter really is
//! `polygloss-cli` before signaling it), a pid's parent (to find the agent host
//! behind a hook's shell), liveness, and `SIGTERM`.
//!
//! macOS only (`proc_pidpath`, `proc_pidinfo`); other targets answer `None`.
//! Each FFI call sits behind a narrow `#[allow(unsafe_code)]` (OQ-P8).

use std::path::PathBuf;

/// The executable image of process `pid` (symlinks resolved by the kernel:
/// `bin/polygloss` → `…/polygloss-cli`). `None` when the process does not exist,
/// belongs to someone we may not inspect, or `pid <= 0`.
pub fn executable_path(pid: i32) -> Option<PathBuf> {
    if pid <= 0 {
        return None;
    }
    imp::executable_path(pid)
}

/// The parent pid of process `pid`, `None` when it cannot be read.
pub fn parent_pid(pid: i32) -> Option<i32> {
    if pid <= 0 {
        return None;
    }
    imp::parent_pid(pid)
}

/// Whether process `pid` exists (`kill(pid, 0)`; `EPERM` still means it exists).
pub fn is_alive(pid: i32) -> bool {
    pid > 0 && imp::is_alive(pid)
}

/// Sends `SIGTERM` to process `pid` (never to a process group: `pid <= 0` is an
/// error).
pub fn terminate(pid: i32) -> std::io::Result<()> {
    if pid <= 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("refusing to signal pid {pid}"),
        ));
    }
    imp::terminate(pid)
}

/// The file name of process `pid`'s executable, e.g. `polygloss-cli`.
pub fn executable_name(pid: i32) -> Option<String> {
    executable_path(pid)?
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::PathBuf;

    #[allow(unsafe_code)]
    pub fn executable_path(pid: i32) -> Option<PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: `buf` is a writable buffer of exactly the length passed; the
        // kernel writes at most that many bytes and returns how many it wrote.
        let n = unsafe {
            libc::proc_pidpath(
                pid,
                buf.as_mut_ptr().cast::<libc::c_void>(),
                u32::try_from(buf.len()).ok()?,
            )
        };
        let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
        buf.truncate(n);
        Some(PathBuf::from(OsStr::from_bytes(&buf)))
    }

    #[allow(unsafe_code)]
    pub fn parent_pid(pid: i32) -> Option<i32> {
        let size = std::mem::size_of::<libc::proc_bsdinfo>();
        // SAFETY: `proc_bsdinfo` is plain old data; all-zero is a valid value.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        // SAFETY: `info` is a writable `proc_bsdinfo` and `size` is its size, which
        // is what `PROC_PIDTBSDINFO` fills.
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&raw mut info).cast::<libc::c_void>(),
                i32::try_from(size).ok()?,
            )
        };
        if usize::try_from(n).ok() != Some(size) {
            return None;
        }
        i32::try_from(info.pbi_ppid).ok()
    }

    #[allow(unsafe_code)]
    pub fn is_alive(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that `pid` (> 0, checked by the caller)
        // exists and may be signaled; nothing is delivered.
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[allow(unsafe_code)]
    pub fn terminate(pid: i32) -> std::io::Result<()> {
        // SAFETY: `pid` > 0 (checked by the caller) addresses one process.
        let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::PathBuf;

    pub fn executable_path(_pid: i32) -> Option<PathBuf> {
        None
    }

    pub fn parent_pid(_pid: i32) -> Option<i32> {
        None
    }

    pub fn is_alive(_pid: i32) -> bool {
        false
    }

    pub fn terminate(_pid: i32) -> std::io::Result<()> {
        Err(std::io::Error::other("unsupported platform"))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::{Command, Stdio};

    use super::*;

    fn own_pid() -> i32 {
        i32::try_from(std::process::id()).unwrap()
    }

    #[test]
    fn executable_path_of_this_process_is_the_test_binary() {
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        let found = executable_path(own_pid()).unwrap().canonicalize().unwrap();
        assert_eq!(found, exe);
        assert_eq!(
            executable_name(own_pid()).as_deref(),
            exe.file_name().and_then(|n| n.to_str())
        );
    }

    #[test]
    fn parent_pid_of_this_process_is_our_parent() {
        let expected = i32::try_from(std::os::unix::process::parent_id()).unwrap();
        assert_eq!(parent_pid(own_pid()), Some(expected));
    }

    #[test]
    fn nonpositive_and_dead_pids_have_no_executable() {
        assert_eq!(executable_path(0), None);
        assert_eq!(executable_path(-1), None);
        assert_eq!(parent_pid(0), None);
        assert!(!is_alive(0));
        assert!(!is_alive(-1));
        assert!(terminate(0).is_err());
        assert!(terminate(-1).is_err());

        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        child.wait().unwrap();
        assert!(!is_alive(pid));
        assert_eq!(executable_path(pid), None);
    }

    #[test]
    fn terminate_sends_sigterm() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        assert!(is_alive(pid));
        assert_eq!(executable_name(pid).as_deref(), Some("sleep"));
        terminate(pid).unwrap();
        let status = child.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }
}
