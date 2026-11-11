#![cfg(unix)]
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "subprocess fixture; invoked explicitly by the parent-death test"]
fn guardian_parent() {
    let Some(dir) = std::env::var_os("V2RAYR_GUARDIAN_TEST") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let _guardian = Command::new(env!("CARGO_BIN_EXE_v2rayr"))
        .arg("--internal-core-guardian")
        .arg(dir.join("journal"))
        .args(["/bin/sh", "-c"])
        .arg("echo $$ > \"$1\"; exec /bin/sleep 60")
        .arg("core")
        .arg(dir.join("pid"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(60));
}

fn wait_for(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(12);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "guardian cleanup deadline exceeded"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn killing_parent_reaps_core_and_retains_unreadable_journal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("journal"), "invalid journal").unwrap();
    let mut parent = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "guardian_parent", "--ignored", "--nocapture"])
        .env("V2RAYR_GUARDIAN_TEST", dir.path())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    wait_for(|| dir.path().join("pid").exists());
    let pid: i32 = std::fs::read_to_string(dir.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    parent.kill().unwrap();
    parent.wait().unwrap();
    wait_for(|| unsafe { libc::kill(pid, 0) } == -1);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("journal")).unwrap(),
        "invalid journal"
    );
}

#[test]
fn closing_lease_stops_core_without_a_recovery_journal() {
    let dir = tempfile::tempdir().unwrap();
    let mut guardian = Command::new(env!("CARGO_BIN_EXE_v2rayr"))
        .arg("--internal-core-guardian")
        .arg(dir.path().join("journal"))
        .args(["/bin/sh", "-c"])
        .arg("echo $$ > \"$1\"; exec /bin/sleep 60")
        .arg("core")
        .arg(dir.path().join("pid"))
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    wait_for(|| dir.path().join("pid").exists());
    let pid: i32 = std::fs::read_to_string(dir.path().join("pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    drop(guardian.stdin.take());
    wait_for(|| guardian.try_wait().unwrap().is_some());
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}

#[test]
fn guardian_ignores_hup_and_int_and_cleans_up_on_term() {
    let dir = tempfile::tempdir().unwrap();
    let mut guardian = Command::new(env!("CARGO_BIN_EXE_v2rayr"))
        .arg("--internal-core-guardian").arg(dir.path().join("journal"))
        .args(["/bin/sh", "-c"])
        .arg("echo $$ > \"$1\"; exec /bin/sleep 60")
        .arg("core").arg(dir.path().join("pid"))
        .stdin(Stdio::piped()).spawn().unwrap();
    wait_for(|| dir.path().join("pid").exists());
    let pid: i32 = std::fs::read_to_string(dir.path().join("pid")).unwrap().trim().parse().unwrap();
    for signal in [libc::SIGHUP, libc::SIGINT] {
        assert_eq!(unsafe { libc::kill(guardian.id() as i32, signal) }, 0);
        std::thread::sleep(Duration::from_millis(100));
        assert!(guardian.try_wait().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
    }
    assert_eq!(unsafe { libc::kill(guardian.id() as i32, libc::SIGTERM) }, 0);
    wait_for(|| guardian.try_wait().unwrap().is_some());
    assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
}
