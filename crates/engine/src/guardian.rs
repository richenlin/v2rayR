//! Private executable mode. The pipe is a lifetime lease, not a PID lookup:
//! SIGKILL closes it just as normal application termination does. This process
//! is the core's parent and reaps only its own child.
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

static AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn available() -> bool {
    AVAILABLE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn dispatch() {
    AVAILABLE.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--internal-core-guardian")) {
        return;
    }
    let Some(journal) = args.next() else {
        std::process::exit(2)
    };
    let Some(program) = args.next() else {
        std::process::exit(2)
    };
    let (tx, rx) = std::sync::mpsc::channel();
    #[cfg(unix)]
    install_signals(tx.clone());
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Ignored dispositions and the blocked TERM mask survive exec.
        unsafe {
            command.pre_exec(|| {
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGHUP, libc::SIG_DFL);
                libc::signal(libc::SIGTERM, libc::SIG_DFL);
                let mut set = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                let result = libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
                if result != 0 {
                    return Err(std::io::Error::from_raw_os_error(result));
                }
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    crate::process::hide_console(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            eprintln!("core guardian: {error}");
            std::process::exit(1);
        }
    };
    // Both waits block in the kernel, so the guardian costs no wakeups for as
    // long as the session runs. Only the Unix path signals by pid; Windows
    // uses the process handle.
    #[cfg(unix)]
    let pid = child.id();
    let lease = tx.clone();
    std::thread::spawn(move || {
        let mut byte = [0];
        while matches!(std::io::stdin().read(&mut byte), Ok(n) if n > 0) {}
        let _ = lease.send(Event::LeaseClosed);
    });
    // Observe exit without reaping. The main thread retains Child and the PID
    // remains reserved until all signals have been sent.
    observe_exit(&child, tx);
    match rx.recv() {
        Ok(Event::ChildExited) => {
            let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(1);
            std::process::exit(code);
        }
        Err(_) => std::process::exit(1),
        Ok(Event::LeaseClosed) => {}
    }
    // Retry transient failures. Permanent failures retain the journal
    // for startup recovery; never discard the user's original settings.
    let (done, recovery) = std::sync::mpsc::channel();
    let original = std::fs::read(&journal).ok();
    std::thread::spawn(move || {
        for attempt in 0..3 {
            if restore(Path::new(&journal), original.as_deref()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(250 * (attempt + 1)));
        }
        let _ = done.send(());
    });
    let _ = recovery.recv_timeout(Duration::from_secs(8));
    if matches!(child.try_wait(), Ok(Some(_))) {
        std::process::exit(0);
    }
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(pid.to_string())
            .status();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Event::ChildExited) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = child.wait();
                    std::process::exit(0)
                }
                _ => {}
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    std::process::exit(0);
}

enum Event {
    LeaseClosed,
    ChildExited,
}

fn restore(path: &Path, original: Option<&[u8]>) -> bool {
    restore_with(path, original, |snapshot| {
        v2rayr_platform::proxy::system_proxy().restore(snapshot)
    })
}

fn restore_with(
    path: &Path,
    original: Option<&[u8]>,
    apply: impl FnOnce(&v2rayr_platform::TakeoverSnapshot) -> Result<(), v2rayr_platform::PlatformError>,
) -> bool {
    let Some(raw) = original else {
        return true;
    };
    // A replacement belongs to another session. Never parse or restore it.
    if std::fs::read(path).ok().as_deref() != Some(raw) {
        return true;
    }
    let Ok(snapshot) = serde_json::from_slice::<v2rayr_platform::TakeoverSnapshot>(raw) else {
        return false;
    };
    if let Err(error) = apply(&snapshot) {
        eprintln!("core guardian: proxy recovery pending: {error}");
        return false;
    }
    // Do not remove a journal replaced by another session.
    if std::fs::read(path).ok().as_deref() == Some(raw) {
        let _ = std::fs::remove_file(path);
    }
    true
}

#[cfg(unix)]
fn install_signals(tx: std::sync::mpsc::Sender<Event>) {
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        let mut set = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        assert_eq!(
            libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()),
            0
        );
    }
    std::thread::spawn(move || unsafe {
        let mut set = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        loop {
            let mut signal = 0;
            if libc::sigwait(&set, &mut signal) != 0 {
                break;
            }
            if tx.send(Event::LeaseClosed).is_err() {
                break;
            }
        }
    });
}

#[cfg(unix)]
fn observe_exit(child: &std::process::Child, tx: std::sync::mpsc::Sender<Event>) {
    let pid = child.id();
    std::thread::spawn(move || unsafe {
        let mut info = std::mem::zeroed();
        loop {
            if libc::waitid(libc::P_PID, pid, &mut info, libc::WEXITED | libc::WNOWAIT) == 0 {
                break;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                return;
            }
        }
        let _ = tx.send(Event::ChildExited);
    });
}

#[cfg(windows)]
fn observe_exit(child: &std::process::Child, tx: std::sync::mpsc::Sender<Event>) {
    use std::os::windows::io::AsRawHandle;
    let handle = child.as_raw_handle() as usize;
    std::thread::spawn(move || {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn WaitForSingleObject(handle: *mut std::ffi::c_void, millis: u32) -> u32;
        }
        if unsafe { WaitForSingleObject(handle as *mut _, u32::MAX) } == 0 {
            let _ = tx.send(Event::ChildExited);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    const SNAPSHOT: &[u8] = br#"{"services":[],"applied_at_ms":1}"#;

    #[test]
    fn valid_journal_is_restored_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        std::fs::write(&path, SNAPSHOT).unwrap();
        let mut called = false;
        assert!(restore_with(&path, Some(SNAPSHOT), |snapshot| {
            called = true;
            assert_eq!(snapshot.applied_at_ms, 1);
            Ok(())
        }));
        assert!(called);
        assert!(!path.exists());
    }

    #[test]
    fn retry_does_not_restore_replaced_journal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journal");
        std::fs::write(&path, SNAPSHOT).unwrap();
        let replacement = br#"{"services":[],"applied_at_ms":2}"#;
        assert!(!restore_with(&path, Some(SNAPSHOT), |_| {
            std::fs::write(&path, replacement).unwrap();
            Err(v2rayr_platform::PlatformError::InvalidState(
                "injected failure".into(),
            ))
        }));
        assert!(restore_with(&path, Some(SNAPSHOT), |_| panic!(
            "new session must not be restored"
        )));
        assert_eq!(std::fs::read(&path).unwrap(), replacement);
    }
}
