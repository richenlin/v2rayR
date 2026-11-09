//! One window per user, even when the icon is clicked twice.
//!
//! A lock file is enough: the file holds the owning pid, and a second start
//! either finds a live pid (and hands off) or a stale file (and takes over).
//! Doing this without a socket keeps the failure modes boring — no port to
//! collide with, no listener to leak.

use std::fs;
use std::path::{Path, PathBuf};

use crate::PlatformError;

/// What happened when this process tried to become the running instance.
#[derive(Debug)]
pub enum InstanceOutcome {
    /// This process owns the lock; drop the guard to release it.
    Primary(InstanceGuard),
    /// Another process owns it; the pid is returned so the caller can raise its
    /// window (or simply exit).
    AlreadyRunning(u32),
}

/// Keeps the lock file alive for as long as the process runs.
#[derive(Debug)]
pub struct InstanceGuard {
    path: PathBuf,
}

impl InstanceGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        // Best effort: a leftover file is detected as stale on the next start.
        let _ = fs::remove_file(&self.path);
    }
}

/// Try to become the one instance.
///
/// `lock` is normally `runtime_dir/v2rayr.lock`. A directory that does not
/// exist yet is created, because the first start on a fresh machine has no
/// runtime directory.
pub fn acquire(lock: &Path) -> Result<InstanceOutcome, PlatformError> {
    if let Some(parent) = lock.parent() {
        fs::create_dir_all(parent).map_err(PlatformError::Io)?;
    }

    if let Some(pid) = read_pid(lock) {
        if is_alive(pid) {
            return Ok(InstanceOutcome::AlreadyRunning(pid));
        }
    }

    fs::write(lock, format!("{}\n", std::process::id())).map_err(PlatformError::Io)?;

    Ok(InstanceOutcome::Primary(InstanceGuard {
        path: lock.to_path_buf(),
    }))
}

fn read_pid(lock: &Path) -> Option<u32> {
    let text = fs::read_to_string(lock).ok()?;
    text.trim().parse().ok()
}

/// `kill(pid, 0)` is the portable "is this pid alive" probe on unix; on other
/// platforms we fall back to assuming a foreign lock is live, which can only be
/// fixed by removing the file.
#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 performs error checking only and delivers nothing.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(not(unix))]
fn is_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_acquire_sees_the_first_process() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("v2rayr.lock");
        let guard = match acquire(&lock).expect("first acquire") {
            InstanceOutcome::Primary(guard) => guard,
            InstanceOutcome::AlreadyRunning(pid) => panic!("unexpected foreign lock {pid}"),
        };
        match acquire(&lock).expect("second acquire") {
            InstanceOutcome::AlreadyRunning(pid) => assert_eq!(pid, std::process::id()),
            InstanceOutcome::Primary(_) => panic!("two primaries"),
        }
        drop(guard);
        assert!(matches!(
            acquire(&lock).expect("after release"),
            InstanceOutcome::Primary(_)
        ));
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("v2rayr.lock");
        // pid 1 exists but is not us; use an impossible pid so the file is stale.
        fs::write(&lock, "4294967290\n").expect("write");
        assert!(matches!(
            acquire(&lock).expect("acquire"),
            InstanceOutcome::Primary(_)
        ));
    }
}
