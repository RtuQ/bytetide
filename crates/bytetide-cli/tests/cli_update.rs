use std::process::Command;

#[test]
fn update_help_is_offline_and_documents_check() {
    let output = Command::new(env!("CARGO_BIN_EXE_bytetide"))
        .args(["update", "--help"])
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--check"));
}
