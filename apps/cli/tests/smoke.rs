//! End-to-end smoke tests for the binary the README advertises.

use std::process::Command;

fn run(home: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_v2rayr"))
        .args(args)
        .env("V2RAYR_HOME", home)
        .output()
        .expect("run v2rayr");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

const LINK: &str = "vless://8a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d@tokyo.example.com:443\
?security=tls&sni=tokyo.example.com&type=ws&path=%2Fws#Tokyo";

#[test]
fn inline_links_are_imported_and_exported_back() {
    let home = tempfile::tempdir().expect("tempdir");
    let (ok, stdout, stderr) = run(home.path(), &["import", LINK]);
    assert!(ok, "import failed: {stderr}");
    assert!(stdout.contains("新增 1"), "unexpected summary: {stdout}");

    let (ok, listing, stderr) = run(home.path(), &["nodes"]);
    assert!(ok, "nodes failed: {stderr}");
    assert!(listing.contains("Tokyo"), "missing node: {listing}");

    let (ok, link, stderr) = run(home.path(), &["export", "Tokyo"]);
    assert!(ok, "export failed: {stderr}");
    assert!(
        link.trim().starts_with("vless://"),
        "unexpected link: {link}"
    );
}

#[test]
fn missing_kernel_is_reported_with_a_stable_error_code() {
    let home = tempfile::tempdir().expect("tempdir");
    run(home.path(), &["import", LINK]);

    let (ok, _stdout, stderr) = run(home.path(), &["connect", "Tokyo", "--mode", "manual"]);
    assert!(!ok, "connect must fail without a kernel");
    assert!(
        stderr.contains("error.core_missing"),
        "expected a structured error code, got: {stderr}"
    );
}

#[test]
fn status_on_a_fresh_home_explains_the_gaps() {
    let home = tempfile::tempdir().expect("tempdir");
    let (ok, stdout, stderr) = run(home.path(), &["status"]);
    assert!(ok, "status failed: {stderr}");
    assert!(stdout.contains("未安装"), "expected kernel gaps: {stdout}");
}
