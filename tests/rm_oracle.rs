#[path = "../src/test_support.rs"]
mod support;

use std::ffi::{OsStr, c_int, c_uint};
use std::fs;
use std::io::Write;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use support::Sandbox;

const SUBJECT: &str = env!("CARGO_BIN_EXE_rmu");
const SIGPIPE: c_int = 13;
const SIG_IGN: usize = 1;
const SIG_ERR: usize = usize::MAX;

unsafe extern "C" {
    fn pipe(descriptors: *mut c_int) -> c_int;
    fn umask(mask: c_uint) -> c_uint;
    fn signal(signal: c_int, handler: usize) -> usize;
}

#[derive(Clone, Copy)]
enum Fixture {
    Empty,
    File,
    EmptyDirectory,
    NonemptyDirectory,
    Symlink,
    BrokenSymlink,
    FourFiles,
    Mixed,
    HyphenFile,
}

struct Case<'a> {
    name: &'a str,
    arguments: &'a [&'a str],
    input: &'a [u8],
    fixture: Fixture,
    checks: &'a [&'a str],
}

#[test]
#[allow(clippy::too_many_lines)]
fn matches_gnu_rm_top_level_decisions() {
    let gnu_rm = require_gnu_rm();
    let cases = [
        Case {
            name: "file",
            arguments: &["file"],
            input: b"",
            fixture: Fixture::File,
            checks: &["file"],
        },
        Case {
            name: "force-missing",
            arguments: &["-f", "missing"],
            input: b"",
            fixture: Fixture::Empty,
            checks: &["missing"],
        },
        Case {
            name: "force-not-a-directory-component",
            arguments: &["-f", "file/child"],
            input: b"",
            fixture: Fixture::File,
            checks: &["file"],
        },
        Case {
            name: "force-trailing-slash-file",
            arguments: &["-f", "file/"],
            input: b"",
            fixture: Fixture::File,
            checks: &["file"],
        },
        Case {
            name: "force-trailing-slash-broken-symlink",
            arguments: &["-f", "broken/"],
            input: b"",
            fixture: Fixture::BrokenSymlink,
            checks: &["broken"],
        },
        Case {
            name: "recursive-force-trailing-slash-broken-symlink",
            arguments: &["-rf", "broken/"],
            input: b"",
            fixture: Fixture::BrokenSymlink,
            checks: &["broken"],
        },
        Case {
            name: "force-trailing-slash-symlink-to-file",
            arguments: &["-f", "link/"],
            input: b"",
            fixture: Fixture::Symlink,
            checks: &["link", "target"],
        },
        Case {
            name: "directory-refused",
            arguments: &["dir"],
            input: b"",
            fixture: Fixture::EmptyDirectory,
            checks: &["dir"],
        },
        Case {
            name: "empty-directory-d",
            arguments: &["-d", "dir"],
            input: b"",
            fixture: Fixture::EmptyDirectory,
            checks: &["dir"],
        },
        Case {
            name: "empty-directory-d-interactive-yes",
            arguments: &["-di", "dir"],
            input: b"y\n",
            fixture: Fixture::EmptyDirectory,
            checks: &["dir"],
        },
        Case {
            name: "empty-directory-d-interactive-no",
            arguments: &["-di", "dir"],
            input: b"n\n",
            fixture: Fixture::EmptyDirectory,
            checks: &["dir"],
        },
        Case {
            name: "nonempty-directory-d",
            arguments: &["-d", "dir"],
            input: b"",
            fixture: Fixture::NonemptyDirectory,
            checks: &["dir", "dir/child"],
        },
        Case {
            name: "recursive-directory",
            arguments: &["-r", "dir"],
            input: b"",
            fixture: Fixture::NonemptyDirectory,
            checks: &["dir"],
        },
        Case {
            name: "symlink",
            arguments: &["link"],
            input: b"",
            fixture: Fixture::Symlink,
            checks: &["link", "target"],
        },
        Case {
            name: "broken-symlink",
            arguments: &["broken"],
            input: b"",
            fixture: Fixture::BrokenSymlink,
            checks: &["broken"],
        },
        Case {
            name: "hyphen-file",
            arguments: &["--", "-file"],
            input: b"",
            fixture: Fixture::HyphenFile,
            checks: &["-file"],
        },
        Case {
            name: "mixed-partial-failure",
            arguments: &["file", "dir"],
            input: b"",
            fixture: Fixture::Mixed,
            checks: &["file", "dir"],
        },
        Case {
            name: "interactive-no",
            arguments: &["-i", "file"],
            input: b"n\n",
            fixture: Fixture::File,
            checks: &["file"],
        },
        Case {
            name: "interactive-yes",
            arguments: &["-i", "file"],
            input: b"y\n",
            fixture: Fixture::File,
            checks: &["file"],
        },
        Case {
            name: "interactive-invalid-byte",
            arguments: &["-i", "file"],
            input: b"\xff\n",
            fixture: Fixture::File,
            checks: &["file"],
        },
        Case {
            name: "once-no",
            arguments: &["-I", "a", "b", "c", "d"],
            input: b"n\n",
            fixture: Fixture::FourFiles,
            checks: &["a", "b", "c", "d"],
        },
        Case {
            name: "option-after-operand",
            arguments: &["file", "-v"],
            input: b"",
            fixture: Fixture::File,
            checks: &["file"],
        },
    ];

    for case in cases {
        compare_case(&case, &gnu_rm);
    }
}

#[test]
fn empty_directory_d_does_not_require_a_trash_environment() {
    let sandbox = Sandbox::new("empty-directory-d-no-trash-environment");
    let root = sandbox.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("dir")).unwrap();

    let output = Command::new(SUBJECT)
        .args(["-d", "dir"])
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .env_remove("RMU_PROGRESS")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(!root.join("dir").exists());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn dir_mode_continues_after_a_nonempty_directory_without_a_trash_environment() {
    let sandbox = Sandbox::new("directory-d-partial-failure-no-trash-environment");
    let root = sandbox.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("nonempty")).unwrap();
    fs::write(root.join("nonempty/child"), b"keep").unwrap();
    fs::create_dir(root.join("empty")).unwrap();

    let output = Command::new(SUBJECT)
        .args(["-d", "nonempty", "empty"])
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .env_remove("RMU_PROGRESS")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(root.join("nonempty/child").is_file());
    assert!(!root.join("empty").exists());
    assert!(!output.stderr.is_empty());
}

#[test]
fn matches_gnu_rm_parser_exit_decisions() {
    let gnu_rm = require_gnu_rm();
    let cases: &[&[&str]] = &[
        &[],
        &["-f"],
        &["--force"],
        &["-fi", "missing"],
        &["-if", "missing"],
        &["-f", "--interactive=never", "missing"],
        &["--interactive=never", "-f", "missing"],
        &["--interactive=yes", "-f", "missing"],
        &["--interactive=no", "-f", "missing"],
        &["--interactive=none", "-f", "missing"],
        &["--interactive=", "missing"],
        &["--preserve-root=", "missing"],
        &["--rec", "-f", "missing"],
        &["--one", "-f", "missing"],
        &["--ver"],
        &["--definitely-invalid"],
        &["--help"],
        &["--version"],
    ];

    for (index, arguments) in cases.iter().enumerate() {
        let sandbox = Sandbox::new(&format!("parser-{index}"));
        let oracle = run(&gnu_rm, arguments, b"", sandbox.path(), sandbox.path());
        let subject = run(SUBJECT, arguments, b"", sandbox.path(), sandbox.path());
        assert_eq!(
            oracle.status.success(),
            subject.status.success(),
            "parser decision differs for {arguments:?}\nrm stderr: {}\nrmu stderr: {}",
            String::from_utf8_lossy(&oracle.stderr),
            String::from_utf8_lossy(&subject.stderr)
        );
    }
}

#[test]
fn no_preserve_root_abbreviation_policy_matches_gnu_rm() {
    let gnu_rm = require_gnu_rm();
    let name = "no-preserve-root";
    for length in 1..name.len() {
        let sandbox = Sandbox::new(&format!("no-preserve-prefix-{length}"));
        let option = format!("--{}", &name[..length]);
        let arguments = [option.as_str(), "-f", "missing"];
        let oracle = run(&gnu_rm, &arguments, b"", sandbox.path(), sandbox.path());
        let subject = run(SUBJECT, &arguments, b"", sandbox.path(), sandbox.path());
        assert_eq!(
            oracle.status.success(),
            subject.status.success(),
            "parser decision differs for {option}"
        );
    }

    let sandbox = Sandbox::new("no-preserve-exact");
    let arguments = ["--no-preserve-root", "-f", "missing"];
    let oracle = run(&gnu_rm, &arguments, b"", sandbox.path(), sandbox.path());
    let subject = run(SUBJECT, &arguments, b"", sandbox.path(), sandbox.path());
    assert_eq!(oracle.status.success(), subject.status.success());
}

#[test]
fn posixly_correct_stops_option_parsing_after_the_first_operand_like_gnu_rm() {
    let gnu_rm = require_gnu_rm();
    let sandbox = Sandbox::new("posix-option-order");
    let oracle_root = sandbox.path().join("oracle");
    let subject_root = sandbox.path().join("subject");
    fs::create_dir(&oracle_root).unwrap();
    fs::create_dir(&subject_root).unwrap();
    for root in [&oracle_root, &subject_root] {
        fs::write(root.join("file"), b"ordinary").unwrap();
        fs::write(root.join("-v"), b"option-looking operand").unwrap();
    }
    let invoke = |program: &OsStr, root: &Path| {
        Command::new(program)
            .args(["file", "-v"])
            .current_dir(root)
            .env("LC_ALL", "C")
            .env("POSIXLY_CORRECT", "1")
            .env("XDG_DATA_HOME", sandbox.path().join("xdg-data"))
            .env("HOME", sandbox.path().join("home"))
            .env_remove("RMU_PROGRESS")
            .output()
            .unwrap()
    };

    let oracle = invoke(gnu_rm.as_os_str(), &oracle_root);
    let subject = invoke(OsStr::new(SUBJECT), &subject_root);

    assert_eq!(oracle.status.success(), subject.status.success());
    assert_eq!(oracle.stdout.is_empty(), subject.stdout.is_empty());
    for name in ["file", "-v"] {
        assert_eq!(
            path_kind(&oracle_root.join(name)),
            path_kind(&subject_root.join(name)),
            "POSIXLY_CORRECT decision differed for {name}"
        );
    }
}

#[test]
fn order_sensitive_prompt_flags_match_gnu_rm() {
    let gnu_rm = require_gnu_rm();
    let flags = ["-f", "-i", "-I", "--interactive=never"];
    let sandbox = Sandbox::new("prompt-order");
    for first in flags {
        for second in flags {
            for third in flags {
                let arguments = [first, second, third, "missing"];
                let oracle = run(&gnu_rm, &arguments, b"n\n", sandbox.path(), sandbox.path());
                let subject = run(SUBJECT, &arguments, b"n\n", sandbox.path(), sandbox.path());
                assert_eq!(
                    oracle.status.success(),
                    subject.status.success(),
                    "prompt option ordering differs for {arguments:?}\nrm stderr: {}\nrmu stderr: {}",
                    String::from_utf8_lossy(&oracle.stderr),
                    String::from_utf8_lossy(&subject.stderr)
                );
            }
        }
    }
}

#[test]
fn an_earlier_diagnostic_precedes_a_later_prompt() {
    let gnu_rm = require_gnu_rm();
    let sandbox = Sandbox::new("diagnostic-order");
    let oracle_root = sandbox.path().join("oracle");
    let subject_root = sandbox.path().join("subject");
    fs::create_dir(&oracle_root).unwrap();
    fs::create_dir(&subject_root).unwrap();
    fs::write(oracle_root.join("file"), b"keep").unwrap();
    fs::write(subject_root.join("file"), b"keep").unwrap();

    let oracle = run(
        &gnu_rm,
        &["missing", "-i", "file"],
        b"n\n",
        &oracle_root,
        sandbox.path(),
    );
    let subject = run(
        SUBJECT,
        &["missing", "-i", "file"],
        b"n\n",
        &subject_root,
        sandbox.path(),
    );

    for (label, stderr) in [("GNU rm", oracle.stderr), ("rmu", subject.stderr)] {
        let stderr = String::from_utf8_lossy(&stderr);
        let diagnostic = stderr.find("missing").unwrap();
        let prompt = stderr.rfind("remove").unwrap();
        assert!(
            diagnostic < prompt,
            "{label} emitted its later prompt before the earlier diagnostic: {stderr}"
        );
    }
    assert!(subject_root.join("file").exists());
}

#[test]
fn recursive_trailing_slash_symlink_is_never_followed() {
    let sandbox = Sandbox::new("trailing-symlink");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("target")).unwrap();
    fs::write(root.join("target/keep"), b"keep").unwrap();
    symlink("target", root.join("link")).unwrap();

    let output = run(SUBJECT, &["-r", "link/"], b"", &root, sandbox.path());

    assert!(!output.status.success());
    assert_eq!(path_kind(&root.join("link")), "symlink");
    assert_eq!(fs::read(root.join("target/keep")).unwrap(), b"keep");
}

#[test]
fn namespaced_progress_reports_scan_and_move() {
    let sandbox = Sandbox::new("progress");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    fs::write(root.join("dir/file"), b"file").unwrap();

    let output = run(
        SUBJECT,
        &["--rmu-progress=always", "-r", "dir"],
        b"",
        &root,
        sandbox.path(),
    );

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("scan complete: 2 entries"), "{stderr}");
    assert!(stderr.contains("items/s"), "{stderr}");
    assert!(stderr.contains("ETA"), "{stderr}");
    assert!(stderr.contains("moving 'dir' into Trash"), "{stderr}");
    assert!(stderr.contains("moved 'dir'"), "{stderr}");
    assert!(stderr.contains("summary: 2 items"), "{stderr}");
    assert!(stderr.contains("0 failures"), "{stderr}");
}

#[test]
fn progress_changes_during_the_initial_counting_pass() {
    let sandbox = Sandbox::new("progress-planning-updates");
    let root = sandbox.path().join("subject");
    fs::create_dir_all(root.join("dir")).unwrap();
    for index in 0..300 {
        fs::write(root.join(format!("dir/file-{index}")), b"file").unwrap();
    }

    let output = run(
        SUBJECT,
        &["--rmu-progress=always", "-r", "dir"],
        b"",
        &root,
        sandbox.path(),
    );

    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    let execution_scan = stderr.find("rmu: scanning 'dir'").unwrap();
    assert!(
        stderr[..execution_scan].contains("rmu: scanned at least"),
        "the initial counting pass emitted no changing progress: {stderr}"
    );
}

#[test]
fn progress_summary_counts_failures_without_changing_the_exit_decision() {
    let sandbox = Sandbox::new("progress-failure-summary");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();

    let output = run(
        SUBJECT,
        &["--rmu-progress=always", "missing"],
        b"",
        &root,
        sandbox.path(),
    );

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("summary: 0 items"), "{stderr}");
    assert!(stderr.contains("1 failure"), "{stderr}");
}

#[test]
fn progress_summary_counts_declined_and_force_ignored_operands() {
    let sandbox = Sandbox::new("progress-skipped-summary");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), b"keep").unwrap();

    let declined = run(
        SUBJECT,
        &["--rmu-progress=always", "-i", "file"],
        b"n\n",
        &root,
        sandbox.path(),
    );
    assert!(declined.status.success());
    assert!(root.join("file").is_file());
    let stderr = String::from_utf8_lossy(&declined.stderr);
    assert!(stderr.contains("0 failures, 1 skipped"), "{stderr}");

    let ignored = run(
        SUBJECT,
        &["--rmu-progress=always", "-f", "missing"],
        b"",
        &root,
        sandbox.path(),
    );
    assert!(ignored.status.success());
    let stderr = String::from_utf8_lossy(&ignored.stderr);
    assert!(stderr.contains("0 failures, 1 skipped"), "{stderr}");
}

#[test]
fn incomplete_progress_uses_lower_bounds_and_suppresses_eta() {
    let sandbox = Sandbox::new("progress-incomplete");
    let root = sandbox.path().join("subject");
    let blocked = root.join("dir/blocked");
    fs::create_dir_all(&blocked).unwrap();
    fs::write(blocked.join("hidden"), b"hidden").unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();

    let output = run(
        SUBJECT,
        &["--rmu-progress=always", "-r", "dir"],
        b"",
        &root,
        sandbox.path(),
    );

    let trashed_blocked = sandbox.path().join("xdg-data/Trash/files/dir/blocked");
    fs::set_permissions(&trashed_blocked, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("scanned at least 2 items"), "{stderr}");
    assert!(stderr.contains("ETA unavailable"), "{stderr}");
    assert!(stderr.contains("summary: at least 2 items"), "{stderr}");
    assert!(stderr.contains("totals incomplete"), "{stderr}");
}

#[test]
fn environment_progress_reports_scan_and_cli_can_disable_it() {
    let sandbox = Sandbox::new("progress-environment");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("environment-dir")).unwrap();
    fs::write(root.join("environment-dir/file"), b"file").unwrap();

    let environment_output = Command::new(SUBJECT)
        .args(["-r", "environment-dir"])
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env("RMU_PROGRESS", "always")
        .env("XDG_DATA_HOME", sandbox.path().join("xdg-data"))
        .env("HOME", sandbox.path().join("home"))
        .output()
        .unwrap();
    assert!(environment_output.status.success());
    assert!(String::from_utf8_lossy(&environment_output.stderr).contains("scan complete"));

    fs::create_dir(root.join("quiet-dir")).unwrap();
    let quiet_output = Command::new(SUBJECT)
        .args(["--rmu-progress=never", "-r", "quiet-dir"])
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env("RMU_PROGRESS", "always")
        .env("XDG_DATA_HOME", sandbox.path().join("xdg-data"))
        .env("HOME", sandbox.path().join("home"))
        .output()
        .unwrap();
    assert!(quiet_output.status.success());
    assert!(quiet_output.stderr.is_empty());
}

#[test]
fn forced_missing_operands_do_not_require_a_trash_environment() {
    let sandbox = Sandbox::new("force-missing-environment");
    let output = Command::new(SUBJECT)
        .args(["-f", "missing"])
        .current_dir(sandbox.path())
        .env("LC_ALL", "C")
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .env_remove("RMU_PROGRESS")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn forced_enotdir_operands_do_not_initialize_trash() {
    let sandbox = Sandbox::new("force-enotdir-environment");
    fs::write(sandbox.path().join("file"), b"keep").unwrap();
    symlink("missing", sandbox.path().join("broken")).unwrap();
    let data_home = sandbox.path().join("xdg-data");

    let output = Command::new(SUBJECT)
        .args(["-rf", "file/child", "file/", "broken/"])
        .current_dir(sandbox.path())
        .env("LC_ALL", "C")
        .env("XDG_DATA_HOME", &data_home)
        .env("HOME", sandbox.path().join("home"))
        .env_remove("RMU_PROGRESS")
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
    assert_eq!(fs::read(sandbox.path().join("file")).unwrap(), b"keep");
    assert!(fs::symlink_metadata(sandbox.path().join("broken")).is_ok());
    assert!(!data_home.exists());
}

#[test]
fn newly_created_trash_layout_ignores_a_restrictive_umask() {
    let sandbox = Sandbox::new("restrictive-umask");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), b"recoverable").unwrap();
    let data_home = sandbox.path().join("new-data-home");
    let mut command = Command::new(SUBJECT);
    command
        .arg("file")
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env("XDG_DATA_HOME", &data_home)
        .env("HOME", sandbox.path().join("home"))
        .env_remove("RMU_PROGRESS");
    // SAFETY: umask is async-signal-safe and this closure runs in the child
    // immediately before exec without allocating or touching shared state.
    unsafe {
        command.pre_exec(|| {
            umask(0o777);
            Ok(())
        });
    }

    let output = command.output().unwrap();

    assert!(output.status.success(), "{:?}", output.stderr);
    let trash = data_home.join("Trash");
    for directory in [
        &data_home,
        &trash,
        &trash.join("files"),
        &trash.join("info"),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700,
            "unexpected permissions on {directory:?}"
        );
    }
    let info = fs::read_dir(trash.join("info"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::metadata(info).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_closed_verbose_pipe_never_abandons_later_operands_or_panics() {
    let sandbox = Sandbox::new("closed-verbose-pipe");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    let names: Vec<String> = (0..200).map(|index| format!("file-{index}")).collect();
    for name in &names {
        fs::write(root.join(name), b"recoverable").unwrap();
    }
    let mut child = Command::new(SUBJECT)
        .arg("-v")
        .args(&names)
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env("XDG_DATA_HOME", sandbox.path().join("xdg-data"))
        .env("HOME", sandbox.path().join("home"))
        .env_remove("RMU_PROGRESS")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());

    let output = child.wait_with_output().unwrap();

    assert!(!output.status.success());
    assert!(names.iter().all(|name| !root.join(name).exists()));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("write error: Broken pipe"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

#[test]
fn a_closed_diagnostic_pipe_never_abandons_later_operands_or_panics() {
    let sandbox = Sandbox::new("closed-diagnostic-pipe");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), b"recoverable").unwrap();
    let mut child = Command::new(SUBJECT)
        .args(["missing", "file"])
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env("XDG_DATA_HOME", sandbox.path().join("xdg-data"))
        .env("HOME", sandbox.path().join("home"))
        .env_remove("RMU_PROGRESS")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stderr.take());

    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(1));
    assert!(!root.join("file").exists());
}

#[test]
fn help_with_a_closed_pipe_never_panics() {
    let mut child = Command::new(SUBJECT)
        .arg("--help")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    drop(child.stdout.take());

    let status = child.wait().unwrap();

    assert_ne!(status.code(), Some(101));
}

#[test]
fn a_closed_progress_pipe_moves_every_operand_before_failing_output() {
    let sandbox = Sandbox::new("closed-progress-pipe");
    let root = sandbox.path().join("subject");
    fs::create_dir(&root).unwrap();
    let names: Vec<String> = (0..100).map(|index| format!("file-{index}")).collect();
    for name in &names {
        fs::write(root.join(name), b"recoverable").unwrap();
    }
    let mut child = Command::new(SUBJECT)
        .args(&names)
        .current_dir(&root)
        .env("LC_ALL", "C")
        .env("RMU_PROGRESS", "always")
        .env("XDG_DATA_HOME", sandbox.path().join("xdg-data"))
        .env("HOME", sandbox.path().join("home"))
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stderr.take());

    let status = child.wait().unwrap();

    assert_eq!(status.code(), Some(1));
    assert!(names.iter().all(|name| !root.join(name).exists()));
}

#[test]
fn delegate_flag_matches_gnu_rm_output_status_and_filesystem_effects() {
    let gnu_rm = require_gnu_rm();
    let sandbox = Sandbox::new("permanent-delegation");
    let oracle_root = sandbox.path().join("oracle");
    let subject_root = sandbox.path().join("subject");
    for root in [&oracle_root, &subject_root] {
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::write(root.join("dir/child"), b"child").unwrap();
        fs::write(root.join("file"), b"file").unwrap();
    }

    let oracle = run(
        &gnu_rm,
        &["-rv", "dir", "file"],
        b"",
        &oracle_root,
        sandbox.path(),
    );
    let subject = Command::new(SUBJECT)
        .args(["--delegate", "-rv", "dir", "file"])
        .current_dir(&subject_root)
        .env("LC_ALL", "C")
        .env("RMU_GNU_RM", &gnu_rm)
        .env_remove("RMU_MODE")
        .env_remove("RMU_PROGRESS")
        .output()
        .unwrap();

    assert_eq!(subject.status.code(), oracle.status.code());
    assert_eq!(subject.stdout, oracle.stdout);
    assert_eq!(subject.stderr, oracle.stderr);
    assert_eq!(path_kind(&subject_root.join("dir")), "missing");
    assert_eq!(path_kind(&subject_root.join("file")), "missing");
}

#[test]
fn environment_can_select_delegate_mode_and_trash_flag_can_override_it() {
    let gnu_rm = require_gnu_rm();
    let sandbox = Sandbox::new("environment-mode");
    fs::write(sandbox.path().join("file"), b"file").unwrap();
    let delegated = Command::new(SUBJECT)
        .arg("file")
        .current_dir(sandbox.path())
        .env("RMU_MODE", "delegate")
        .env("RMU_GNU_RM", &gnu_rm)
        .output()
        .unwrap();
    assert!(delegated.status.success());
    assert!(!sandbox.path().join("file").exists());

    let trash_override = Command::new(SUBJECT)
        .args(["--trash", "-f", "missing"])
        .current_dir(sandbox.path())
        .env("RMU_MODE", "not-a-mode")
        .env_remove("RMU_PROGRESS")
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(trash_override.status.success());
}

#[test]
fn delegate_mode_refuses_to_delegate_back_to_rmu() {
    let output = Command::new(SUBJECT)
        .args(["--delegate", "-f", "missing"])
        .env("RMU_GNU_RM", SUBJECT)
        .env_remove("RMU_MODE")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not identify GNU coreutils rm"));
}

#[test]
fn delegate_probe_abandons_a_nonresponsive_override() {
    let sandbox = Sandbox::new("delegate-probe-timeout");
    let candidate = sandbox.path().join("rm");
    fs::write(&candidate, b"#!/bin/sh\nexec sleep 30\n").unwrap();
    fs::set_permissions(&candidate, fs::Permissions::from_mode(0o755)).unwrap();

    let started = Instant::now();
    let output = Command::new(SUBJECT)
        .args(["--delegate", "-f", "missing"])
        .env("RMU_GNU_RM", &candidate)
        .env_remove("RMU_MODE")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not identify GNU coreutils rm"));
}

#[test]
fn delegate_probe_does_not_reenter_rmu_through_a_path_wrapper() {
    let gnu_rm = require_gnu_rm();
    let sandbox = Sandbox::new("delegate-probe-wrapper");
    let fake_bin = sandbox.path().join("fake-bin");
    fs::create_dir(&fake_bin).unwrap();
    let wrapper = fake_bin.join("rm");
    let log = sandbox.path().join("wrapper.log");
    fs::write(
        &wrapper,
        b"#!/bin/sh\nprintf 'x\\n' >> \"$RMU_TEST_LOG\"\ndepth=${RMU_TEST_DEPTH:-0}\nif [ \"$depth\" -ge 3 ]; then exit 98; fi\nRMU_TEST_DEPTH=$((depth + 1))\nexport RMU_TEST_DEPTH\nexec \"$RMU_TEST_SUBJECT\" \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths([fake_bin.as_path(), gnu_rm.parent().unwrap()]).unwrap();

    let output = Command::new(SUBJECT)
        .args(["-f", "missing"])
        .current_dir(sandbox.path())
        .env("PATH", path)
        .env("RMU_MODE", "delegate")
        .env("RMU_PROGRESS", "not-a-mode")
        .env("RMU_TEST_DEPTH", "0")
        .env("RMU_TEST_LOG", &log)
        .env("RMU_TEST_SUBJECT", SUBJECT)
        .env_remove("RMU_GNU_RM")
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(fs::read(&log).unwrap(), b"x\n");
}

#[test]
fn delegate_executes_the_same_file_that_passed_the_gnu_probe() {
    let sandbox = Sandbox::new("delegate-pinned-executable");
    let executable = sandbox.path().join("rm");
    let original = sandbox.path().join("rm.probed");
    let marker = sandbox.path().join("executed");
    fs::write(
        &executable,
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  mv \"$RMU_TEST_PROGRAM\" \"$RMU_TEST_ORIGINAL\"\n  printf '#!/bin/sh\\nprintf replacement > \"$RMU_TEST_MARKER\"\\nexit 91\\n' > \"$RMU_TEST_PROGRAM\"\n  chmod 0755 \"$RMU_TEST_PROGRAM\"\n  printf '%s\\n' 'rm (GNU coreutils) 9.99'\n  exit 0\nfi\nprintf original > \"$RMU_TEST_MARKER\"\nexit 0\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(SUBJECT)
        .args(["--delegate", "-f", "missing"])
        .env("RMU_GNU_RM", &executable)
        .env("RMU_TEST_PROGRAM", &executable)
        .env("RMU_TEST_ORIGINAL", &original)
        .env("RMU_TEST_MARKER", &marker)
        .env_remove("RMU_MODE")
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(fs::read(marker).unwrap(), b"original");
}

#[test]
fn delegate_exec_failure_escapes_the_verified_executable_path() {
    let sandbox = Sandbox::new("delegate-exec-diagnostic");
    let executable = sandbox.path().join(OsStr::from_bytes(b"gnu-rm\n\x1b[31m"));
    fs::write(
        &executable,
        b"#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf '%s\\n' 'rm (GNU coreutils) 9.99'\n  chmod 0644 \"$0\"\n  exit 0\nfi\nexit 99\n",
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();

    let output = Command::new(SUBJECT)
        .args(["--delegate", "-f", "missing"])
        .env("RMU_GNU_RM", &executable)
        .env_remove("RMU_MODE")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!output.stderr.contains(&b'\x1b'));
    let diagnostic = output.stderr.strip_suffix(b"\n").unwrap();
    assert!(!diagnostic.contains(&b'\n'));
    assert!(String::from_utf8_lossy(&output.stderr).contains("gnu-rm\\n\\x1B[31m"));
}

#[test]
fn delegate_preserves_gnu_rm_closed_pipe_behavior() {
    let gnu_rm = require_gnu_rm();
    let sandbox = Sandbox::new("delegate-closed-pipe");
    let padding = "x".repeat(180);
    let names: Vec<String> = (0..200)
        .map(|index| format!("file-{index:04}-{padding}"))
        .collect();

    for ignore_sigpipe in [false, true] {
        let suffix = if ignore_sigpipe { "ignored" } else { "default" };
        let oracle_root = sandbox.path().join(format!("oracle-{suffix}"));
        let subject_root = sandbox.path().join(format!("subject-{suffix}"));
        fs::create_dir(&oracle_root).unwrap();
        fs::create_dir(&subject_root).unwrap();
        for root in [&oracle_root, &subject_root] {
            for name in &names {
                fs::write(root.join(name), b"file").unwrap();
            }
        }

        let mut oracle = Command::new(&gnu_rm);
        oracle
            .arg("-v")
            .args(&names)
            .current_dir(&oracle_root)
            .env("LC_ALL", "C");
        let oracle = run_with_closed_stdout(oracle, ignore_sigpipe);

        let mut subject = Command::new(SUBJECT);
        subject
            .args(["--delegate", "-v"])
            .args(&names)
            .current_dir(&subject_root)
            .env("LC_ALL", "C")
            .env("RMU_GNU_RM", &gnu_rm)
            .env_remove("RMU_MODE")
            .env_remove("RMU_PROGRESS");
        let subject = run_with_closed_stdout(subject, ignore_sigpipe);

        assert_eq!(subject.status.code(), oracle.status.code());
        assert_eq!(subject.status.signal(), oracle.status.signal());
        assert_eq!(subject.stderr, oracle.stderr);
        for name in &names {
            assert_eq!(
                subject_root.join(name).exists(),
                oracle_root.join(name).exists(),
                "filesystem effect differs for {name} with {suffix} SIGPIPE"
            );
        }
        if ignore_sigpipe {
            assert!(names.iter().all(|name| !subject_root.join(name).exists()));
        } else {
            assert!(names.iter().any(|name| subject_root.join(name).exists()));
        }
    }
}

fn run_with_closed_stdout(mut command: Command, ignore_sigpipe: bool) -> Output {
    let mut descriptors = [0; 2];
    // SAFETY: pipe initializes both descriptors on success. Each descriptor is
    // immediately given one File owner, and the read end is closed before the
    // child is spawned so its first write deterministically sees a closed pipe.
    assert_eq!(unsafe { pipe(descriptors.as_mut_ptr()) }, 0);
    let reader = unsafe { fs::File::from_raw_fd(descriptors[0]) };
    let writer = unsafe { fs::File::from_raw_fd(descriptors[1]) };
    drop(reader);
    command.stdout(Stdio::from(writer)).stderr(Stdio::piped());
    let handler = if ignore_sigpipe { SIG_IGN } else { 0 };
    // SAFETY: signal is async-signal-safe and the closure touches no shared
    // application state between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if signal(SIGPIPE, handler) == SIG_ERR {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn().unwrap().wait_with_output().unwrap()
}

fn compare_case(case: &Case<'_>, gnu_rm: &Path) {
    let sandbox = Sandbox::new(case.name);
    let oracle_root = sandbox.path().join("oracle");
    let subject_root = sandbox.path().join("subject");
    fs::create_dir(&oracle_root).unwrap();
    fs::create_dir(&subject_root).unwrap();
    setup(case.fixture, &oracle_root);
    setup(case.fixture, &subject_root);

    let oracle = run(
        gnu_rm,
        case.arguments,
        case.input,
        &oracle_root,
        sandbox.path(),
    );
    let subject = run(
        SUBJECT,
        case.arguments,
        case.input,
        &subject_root,
        sandbox.path(),
    );

    assert_eq!(
        oracle.status.success(),
        subject.status.success(),
        "status differs for {}\nrm stderr: {}\nrmu stderr: {}",
        case.name,
        String::from_utf8_lossy(&oracle.stderr),
        String::from_utf8_lossy(&subject.stderr)
    );
    for check in case.checks {
        assert_eq!(
            path_kind(&oracle_root.join(check)),
            path_kind(&subject_root.join(check)),
            "filesystem decision differs for {} at {check}",
            case.name
        );
    }
    assert_eq!(
        oracle.stderr.is_empty(),
        subject.stderr.is_empty(),
        "diagnostic presence differs for {}\nrm stderr: {}\nrmu stderr: {}",
        case.name,
        String::from_utf8_lossy(&oracle.stderr),
        String::from_utf8_lossy(&subject.stderr)
    );
}

fn run(
    program: impl AsRef<OsStr>,
    arguments: &[&str],
    input: &[u8],
    cwd: &Path,
    sandbox: &Path,
) -> Output {
    let mut child = Command::new(program)
        .args(arguments)
        .current_dir(cwd)
        .env("LC_ALL", "C")
        .env("XDG_DATA_HOME", sandbox.join("xdg-data"))
        .env("HOME", sandbox.join("home"))
        .env_remove("RMU_MODE")
        .env_remove("RMU_PROGRESS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Err(error) = child.stdin.take().unwrap().write_all(input) {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe,
            "could not provide subprocess input"
        );
    }
    child.wait_with_output().unwrap()
}

fn require_gnu_rm() -> PathBuf {
    find_gnu_rm()
        .unwrap_or_else(|reason| panic!("GNU rm differential oracle unavailable: {reason}"))
}

fn find_gnu_rm() -> Result<PathBuf, String> {
    let override_path = std::env::var_os("RMU_GNU_RM");
    let candidates: Vec<PathBuf> = if let Some(path) = &override_path {
        vec![PathBuf::from(path)]
    } else {
        std::env::var_os("PATH")
            .map(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("rm"))
                    .collect()
            })
            .unwrap_or_default()
    };

    for candidate in candidates {
        let Ok(output) = Command::new(&candidate)
            .arg("--version")
            .env("LC_ALL", "C")
            .output()
        else {
            continue;
        };
        if output.status.success()
            && output
                .stdout
                .windows(b"GNU coreutils".len())
                .any(|window| window == b"GNU coreutils")
        {
            return Ok(candidate);
        }
    }

    if let Some(path) = override_path {
        Err(format!(
            "RMU_GNU_RM={path:?} does not identify GNU coreutils rm"
        ))
    } else {
        Err("no GNU coreutils rm was found on PATH; set RMU_GNU_RM to its path".into())
    }
}

fn setup(fixture: Fixture, root: &Path) {
    match fixture {
        Fixture::Empty => {}
        Fixture::File => fs::write(root.join("file"), b"file").unwrap(),
        Fixture::EmptyDirectory => fs::create_dir(root.join("dir")).unwrap(),
        Fixture::NonemptyDirectory => {
            fs::create_dir(root.join("dir")).unwrap();
            fs::write(root.join("dir/child"), b"child").unwrap();
        }
        Fixture::Symlink => {
            fs::write(root.join("target"), b"target").unwrap();
            symlink("target", root.join("link")).unwrap();
        }
        Fixture::BrokenSymlink => symlink("missing", root.join("broken")).unwrap(),
        Fixture::FourFiles => {
            for name in ["a", "b", "c", "d"] {
                fs::write(root.join(name), name).unwrap();
            }
        }
        Fixture::Mixed => {
            fs::write(root.join("file"), b"file").unwrap();
            fs::create_dir(root.join("dir")).unwrap();
        }
        Fixture::HyphenFile => fs::write(root.join("-file"), b"file").unwrap(),
    }
}

fn path_kind(path: &Path) -> &'static str {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => "missing",
        Err(_) => "error",
        Ok(metadata) if metadata.file_type().is_symlink() => "symlink",
        Ok(metadata) if metadata.is_dir() => "directory",
        Ok(metadata) if metadata.is_file() => "file",
        Ok(_) => "other",
    }
}
