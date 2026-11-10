//! One window per user, even when the icon is clicked twice.
//!
//! The OS owns the lock for the lifetime of an open file descriptor.
//! The PID is advisory for the second launch; stale PID text never owns a lock.

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
    _file: fs::File,
}

impl InstanceGuard {
    pub fn path(&self) -> &Path {
        &self.path
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

    lock_instance(lock)
}

fn lock_instance(lock: &Path) -> Result<InstanceOutcome, PlatformError> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock)?;
    if !try_lock(&file)? {
        return Ok(InstanceOutcome::AlreadyRunning(read_pid(lock).unwrap_or(0)));
    }
    #[cfg(windows)]
    {
        use std::io::{Seek, SeekFrom};
        file.set_len(1)?;
        file.seek(SeekFrom::Start(1))?;
    }
    #[cfg(unix)]
    file.set_len(0)?;
    writeln!(&mut file, "{}", std::process::id())?;
    // Do not unlink a locked inode: another process could lock a new inode at
    // the same path while the first one still owns its descriptor.
    Ok(InstanceOutcome::Primary(InstanceGuard {
        path: lock.to_path_buf(),
        _file: file,
    }))
}

#[cfg(unix)]
fn try_lock(file: &fs::File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    // SAFETY: the file owns this live descriptor; closing it releases the lock.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(windows)]
fn try_lock(file: &fs::File) -> std::io::Result<bool> {
    use std::{ffi::c_void, os::windows::io::AsRawHandle};
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LockFileEx(
            handle: *mut c_void,
            flags: u32,
            reserved: u32,
            low: u32,
            high: u32,
            data: *mut Overlapped,
        ) -> i32;
    }
    // Lock byte 0 only; PID text begins at byte 1, so other readers can report
    // the PID without requiring permission to read the locked byte.
    let mut data = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    // SAFETY: synchronous handle, live OVERLAPPED and one byte at offset zero.
    if unsafe { LockFileEx(file.as_raw_handle(), 3, 0, 1, 0, &mut data) } != 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(33) {
        Ok(false)
    } else {
        Err(error)
    }
}

fn read_pid(lock: &Path) -> Option<u32> {
    #[cfg(unix)]
    let text = fs::read_to_string(lock).ok()?;
    #[cfg(windows)]
    let text = {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = fs::File::open(lock).ok()?;
        file.seek(SeekFrom::Start(1)).ok()?;
        let mut text = String::new();
        file.read_to_string(&mut text).ok()?;
        text
    };
    text.trim().parse().ok()
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
