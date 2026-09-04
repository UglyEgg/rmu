#[path = "../src/test_support.rs"]
mod support;

use std::fs;
use std::process::Command;

use support::Sandbox;

const SUBJECT: &str = env!("CARGO_BIN_EXE_rmuctl");
const RMU: &str = env!("CARGO_BIN_EXE_rmu");

#[test]
fn help_does_not_discover_or_read_any_trash_root() {
    let output = Command::new(SUBJECT)
        .env("RMU_PROGRESS", "invalid")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("Usage: rmuctl")
    );
}

#[test]
fn version_does_not_discover_or_read_any_trash_root() {
    let output = Command::new(SUBJECT).arg("--version").output().unwrap();
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("rmuctl ")
    );
}

#[test]
fn completions_print_the_packaged_definitions_without_discovering_trash() {
    let cases: [(&str, &str, &[u8]); 6] = [
        ("bash", "rmu", include_bytes!("../completions/rmu.bash")),
        (
            "bash",
            "rmuctl",
            include_bytes!("../completions/rmuctl.bash"),
        ),
        ("zsh", "rmu", include_bytes!("../completions/_rmu")),
        ("zsh", "rmuctl", include_bytes!("../completions/_rmuctl")),
        ("fish", "rmu", include_bytes!("../completions/rmu.fish")),
        (
            "fish",
            "rmuctl",
            include_bytes!("../completions/rmuctl.fish"),
        ),
    ];

    for (shell, target, expected) in cases {
        let output = Command::new(SUBJECT)
            .env_remove("HOME")
            .env_remove("XDG_DATA_HOME")
            .env("RMU_PROGRESS", "invalid-but-irrelevant")
            .args(["completions", shell, target])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {target}: {}",
            shell,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(output.stdout, expected, "{shell} {target}");
    }
}

#[test]
fn successful_orphan_recovery_reports_resolution_and_exits_zero() {
    let sandbox = Sandbox::new("rmuctl-doctor-resolved");
    let home = sandbox.path().join("home");
    let data = sandbox.path().join("data");
    fs::create_dir(&home).unwrap();
    let source = sandbox.path().join("document");
    fs::write(&source, b"recoverable").unwrap();
    let trashed = Command::new(RMU)
        .arg(&source)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env_remove("RMU_MODE")
        .env_remove("RMU_PROGRESS")
        .output()
        .unwrap();
    assert!(trashed.status.success(), "{:?}", trashed.stderr);
    let info = fs::read_dir(data.join("Trash/info"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::remove_file(info).unwrap();
    let recovery = sandbox.path().join("recovery");

    let output = Command::new(SUBJECT)
        .args(["doctor", "--recover-orphans"])
        .arg(&recovery)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(String::from_utf8_lossy(&output.stdout).contains("resolved:"));
    assert_eq!(fs::read_dir(recovery).unwrap().count(), 1);
}
