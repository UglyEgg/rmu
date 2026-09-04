use std::env;
use std::ffi::{CString, OsStr, OsString, c_char, c_int, c_uint};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::display::escape_bytes;
use crate::linux::{O_NONBLOCK, O_PATH};

const GNU_PROBE_LIMIT: usize = 64 * 1024;
const GNU_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;

unsafe extern "C" {
    fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
    fn fcntl(descriptor: c_int, command: c_int, ...) -> c_int;
    fn poll(descriptors: *mut PollDescriptor, count: usize, timeout: c_int) -> c_int;
    fn fexecve(
        descriptor: c_int,
        arguments: *const *const c_char,
        environment: *const *const c_char,
    ) -> c_int;
}

#[repr(C)]
struct PollDescriptor {
    descriptor: c_int,
    events: i16,
    returned_events: i16,
}

const POLL_INPUT: i16 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationMode {
    Trash,
    Delegate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModeSelection {
    pub mode: OperationMode,
    pub arguments: Vec<OsString>,
}

pub fn select_mode(
    arguments: impl IntoIterator<Item = OsString>,
    environment: Option<&OsStr>,
    posixly_correct: bool,
) -> Result<ModeSelection, String> {
    let mut selected = None;
    let mut filtered = Vec::new();
    let mut options = true;
    for argument in arguments {
        let bytes = argument.as_bytes();
        if options && bytes == b"--" {
            options = false;
            filtered.push(argument);
            continue;
        }
        if options && bytes == b"--trash" {
            selected = Some(OperationMode::Trash);
            continue;
        }
        if options && bytes == b"--delegate" {
            selected = Some(OperationMode::Delegate);
            continue;
        }
        let option_like = bytes.starts_with(b"-") && bytes.len() > 1;
        filtered.push(argument);
        if posixly_correct && !option_like {
            options = false;
        }
    }

    let mode = selected.map_or_else(|| mode_from_environment(environment), Ok)?;
    Ok(ModeSelection {
        mode,
        arguments: filtered,
    })
}

fn mode_from_environment(value: Option<&OsStr>) -> Result<OperationMode, String> {
    match value.map(OsStrExt::as_bytes) {
        None | Some(b"trash") => Ok(OperationMode::Trash),
        Some(b"delegate") => Ok(OperationMode::Delegate),
        Some(other) => Err(format!(
            "invalid value '{}' for RMU_MODE; expected trash or delegate",
            escape_bytes(other)
        )),
    }
}

pub fn execute_gnu_rm(arguments: &[OsString]) -> Result<(), String> {
    let executable = find_gnu_rm()?;
    let argument_values = std::iter::once(executable.path.as_os_str())
        .chain(arguments.iter().map(OsString::as_os_str))
        .map(|argument| CString::new(argument.as_bytes()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "cannot delegate an argument containing NUL".to_string())?;
    let mut argument_pointers: Vec<_> = argument_values
        .iter()
        .map(|argument| argument.as_ptr())
        .collect();
    argument_pointers.push(std::ptr::null());

    let environment_values = env::vars_os()
        .map(|(name, value)| {
            let mut assignment = name.as_bytes().to_vec();
            assignment.push(b'=');
            assignment.extend_from_slice(value.as_bytes());
            CString::new(assignment)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "cannot delegate with an environment value containing NUL".to_string())?;
    let mut environment_pointers: Vec<_> = environment_values
        .iter()
        .map(|assignment| assignment.as_ptr())
        .collect();
    environment_pointers.push(std::ptr::null());

    crate::linux::restore_inherited_sigpipe_for_exec()
        .map_err(|error| format!("cannot restore GNU rm signal handling: {error}"))?;
    // SAFETY: the descriptor pins the executable accepted by the GNU probe;
    // both pointer arrays are NUL-terminated and their CString storage remains
    // live for the call. fexecve replaces the process on success.
    unsafe {
        fexecve(
            executable.file.as_raw_fd(),
            argument_pointers.as_ptr(),
            environment_pointers.as_ptr(),
        );
    }
    let error = std::io::Error::last_os_error();
    Err(format!(
        "cannot execute GNU rm at {}: {error}",
        escape_bytes(executable.path.as_os_str().as_bytes())
    ))
}

struct GnuRm {
    path: PathBuf,
    file: File,
}

fn find_gnu_rm() -> Result<GnuRm, String> {
    let override_path = env::var_os("RMU_GNU_RM");
    let candidates = if let Some(path) = &override_path {
        vec![PathBuf::from(path)]
    } else {
        env::var_os("PATH")
            .map(|path| {
                let current_directory = env::current_dir().ok();
                env::split_paths(&path)
                    .map(|directory| {
                        if directory.as_os_str().is_empty() {
                            current_directory
                                .clone()
                                .unwrap_or_else(|| PathBuf::from("."))
                                .join("rm")
                        } else {
                            directory.join("rm")
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let current = fs::metadata("/proc/self/exe").ok();

    for candidate in candidates {
        let Ok(file) = open_executable(&candidate) else {
            continue;
        };
        let Ok(metadata) = file.metadata() else {
            continue;
        };
        if !metadata.is_file()
            || current.as_ref().is_some_and(|current| {
                metadata.dev() == current.dev() && metadata.ino() == current.ino()
            })
        {
            continue;
        }
        let descriptor_path = PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd()));
        let Ok(mut child) = Command::new(&descriptor_path)
            .arg("--version")
            .env("LC_ALL", "C")
            .env_remove("RMU_MODE")
            .env_remove("RMU_GNU_RM")
            .env_remove("RMU_PROGRESS")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let Ok(Some(stdout)) = read_probe(&mut child) else {
            continue;
        };
        if stdout
            .windows(b"GNU coreutils".len())
            .any(|window| window == b"GNU coreutils")
        {
            return Ok(GnuRm {
                path: candidate,
                file,
            });
        }
    }

    if let Some(path) = override_path {
        Err(format!(
            "RMU_GNU_RM={} does not identify GNU coreutils rm",
            escape_bytes(path.as_bytes())
        ))
    } else {
        Err("delegate mode requires GNU coreutils rm on PATH; set RMU_GNU_RM to its path".into())
    }
}

fn read_probe(child: &mut Child) -> io::Result<Option<Vec<u8>>> {
    let mut pipe = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("GNU rm probe stdout was not piped"))?;
    let descriptor = pipe.as_raw_fd();
    // SAFETY: descriptor is a live pipe and fcntl does not retain arguments.
    let flags = unsafe { fcntl(descriptor, F_GETFL) };
    if flags < 0 {
        stop_probe(child);
        return Err(io::Error::last_os_error());
    }
    // SAFETY: descriptor is live and F_SETFL accepts the retrieved flags.
    if unsafe { fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) } < 0 {
        stop_probe(child);
        return Err(io::Error::last_os_error());
    }

    let deadline = Instant::now() + GNU_PROBE_TIMEOUT;
    let mut output = Vec::new();
    let mut pipe_open = true;
    let mut status = None;
    loop {
        if pipe_open {
            let mut buffer = [0; 4096];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => {
                        pipe_open = false;
                        break;
                    }
                    Ok(length) => {
                        if output.len().saturating_add(length) > GNU_PROBE_LIMIT {
                            stop_probe(child);
                            return Ok(None);
                        }
                        output.extend_from_slice(&buffer[..length]);
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) => {
                        stop_probe(child);
                        return Err(error);
                    }
                }
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(result) => status = result,
                Err(error) => {
                    stop_probe(child);
                    return Err(error);
                }
            }
        }
        if !pipe_open && status.is_some() {
            return Ok(status
                .is_some_and(|status| status.success())
                .then_some(output));
        }
        let now = Instant::now();
        if now >= deadline {
            stop_probe(child);
            return Ok(None);
        }
        let remaining = deadline - now;
        if pipe_open {
            if let Err(error) = wait_for_probe_output(descriptor, remaining) {
                stop_probe(child);
                return Err(error);
            }
        } else {
            thread::sleep(remaining.min(Duration::from_millis(1)));
        }
    }
}

fn wait_for_probe_output(descriptor: c_int, remaining: Duration) -> io::Result<()> {
    let milliseconds = c_int::try_from(remaining.as_millis())
        .unwrap_or(c_int::MAX)
        .max(1);
    let mut poll_descriptor = PollDescriptor {
        descriptor,
        events: POLL_INPUT,
        returned_events: 0,
    };
    // SAFETY: poll_descriptor points to one initialized poll descriptor for a
    // live pipe and is retained for the duration of the call.
    let result = unsafe { poll(&raw mut poll_descriptor, 1, milliseconds) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

fn stop_probe(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        drop(child.kill());
    }
    drop(child.wait());
}

fn open_executable(path: &Path) -> std::io::Result<File> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    // Deliberately omit O_CLOEXEC: script interpreters must be able to reopen
    // the pinned descriptor after fexecve resolves an interpreter directive.
    // SAFETY: path is NUL-terminated and open does not retain the pointer.
    let descriptor = unsafe { open(path.as_ptr(), O_PATH, 0) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: open returned a new owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn select(arguments: &[&str], environment: Option<&str>, posix: bool) -> ModeSelection {
        select_mode(
            arguments.iter().map(OsString::from),
            environment.map(OsStr::new),
            posix,
        )
        .unwrap()
    }

    #[test]
    fn trash_is_the_default_and_selectors_are_removed() {
        assert_eq!(select(&["file"], None, false).mode, OperationMode::Trash);
        let selected = select(&["--delegate", "--trash", "file"], None, false);
        assert_eq!(selected.mode, OperationMode::Trash);
        assert_eq!(selected.arguments, [OsString::from("file")]);
    }

    #[test]
    fn environment_selects_a_mode_and_cli_takes_precedence() {
        assert_eq!(
            select(&["file"], Some("delegate"), false).mode,
            OperationMode::Delegate
        );
        assert_eq!(
            select(&["--trash", "file"], Some("invalid"), false).mode,
            OperationMode::Trash
        );
        assert!(select_mode([], Some(OsStr::new("yes")), false).is_err());
    }

    #[test]
    fn double_dash_and_posix_order_protect_option_looking_filenames() {
        let selected = select(&["--", "--delegate"], None, false);
        assert_eq!(selected.mode, OperationMode::Trash);
        assert_eq!(
            selected.arguments,
            [OsString::from("--"), OsString::from("--delegate")]
        );

        let selected = select(&["file", "--delegate"], None, true);
        assert_eq!(selected.mode, OperationMode::Trash);
        assert_eq!(
            selected.arguments,
            [OsString::from("file"), OsString::from("--delegate")]
        );
    }

    #[test]
    fn invalid_environment_values_are_terminal_safe() {
        let error = select_mode([], Some(OsStr::from_bytes(b"bad\n\x1b\xff")), false).unwrap_err();
        assert!(error.contains("bad\\n\\x1B\\xFF"));
        assert!(!error.contains('\n'));
    }
}
