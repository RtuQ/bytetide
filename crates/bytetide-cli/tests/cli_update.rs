use std::process::Command;

#[test]
fn english_update_help_follows_locale() {
    let output = Command::new(env!("CARGO_BIN_EXE_bytetide"))
        .args(["update", "--help"])
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(
        help.contains("Check for updates without installing"),
        "{help}"
    );
}

#[test]
fn english_monitor_help_follows_locale() {
    let output = Command::new(env!("CARGO_BIN_EXE_bytetide"))
        .args(["monitor", "--help"])
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .unwrap();
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("Serial port path"), "{help}");
}

#[test]
fn update_help_is_offline_and_documents_check() {
    let output = Command::new(env!("CARGO_BIN_EXE_bytetide"))
        .args(["update", "--help"])
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--check"));
}
