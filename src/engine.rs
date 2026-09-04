#[cfg(target_env = "gnu")]
use std::ffi::CString;
use std::ffi::{OsString, c_char, c_int};
use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::args::{Invocation, PreserveRoot, ProgressMode, PromptMode};
use crate::display::quote;
use crate::freedesktop::{
    DirectorySizeUpdate, SourceIdentity, TrashBackend, TrashReceipt,
    remove_empty_directory_verified,
};
use crate::linux::{AccessMode, effective_access};
use crate::scanner::{ScanSummary, scan_tree};

#[cfg(test)]
mod identity_race_tests;

#[cfg(not(target_env = "ohos"))]
const LC_ALL: c_int = 6;
#[cfg(target_env = "ohos")]
const LC_ALL: c_int = 12;

unsafe extern "C" {
    #[cfg(target_env = "gnu")]
    fn rpmatch(response: *const c_char) -> c_int;
    fn setlocale(category: c_int, locale: *const c_char) -> *mut c_char;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Validated {
        operand: OsString,
        path: PathBuf,
    },
    Moving {
        operand: OsString,
        path: PathBuf,
    },
    Trashed {
        operand: OsString,
        receipt: TrashReceipt,
    },
    RemovedEmptyDirectory {
        operand: OsString,
        path: PathBuf,
    },
    RemoveEmptyDirectoryFailed {
        operand: OsString,
        message: String,
    },
    ScanStarted {
        operand: OsString,
        path: PathBuf,
        total: Option<ScanSummary>,
    },
    ScanPlanning {
        operand: OsString,
        path: PathBuf,
    },
    ScanProgress {
        operand: OsString,
        summary: ScanSummary,
    },
    ScanFinished {
        operand: OsString,
        summary: ScanSummary,
    },
    ScanFailed {
        operand: OsString,
        message: String,
    },
    Failed {
        operand: OsString,
        message: String,
    },
    Warning {
        message: String,
    },
}

pub trait Observer {
    fn observe(&mut self, event: &Event);

    fn wants_scan(&self) -> bool {
        false
    }
}

pub trait Prompter {
    fn confirm(&mut self, prompt: &str) -> io::Result<bool>;
    fn stdin_is_terminal(&self) -> bool;
}

pub struct StdioPrompter;

impl Prompter for StdioPrompter {
    fn confirm(&mut self, prompt: &str) -> io::Result<bool> {
        let mut stderr = io::stderr().lock();
        write!(stderr, "{prompt}")?;
        stderr.flush()?;
        let mut response = Vec::new();
        io::stdin().lock().read_until(b'\n', &mut response)?;
        Ok(response_is_affirmative(&response))
    }

    fn stdin_is_terminal(&self) -> bool {
        io::stdin().is_terminal()
    }
}

pub fn initialize_locale() {
    // SAFETY: the empty C string requests the process locale from the
    // environment. This is called once at startup before worker threads exist.
    let _ = unsafe { setlocale(LC_ALL, c"".as_ptr()) };
}

fn response_is_affirmative(response: &[u8]) -> bool {
    let response = response
        .iter()
        .position(|byte| *byte == 0)
        .map_or(response, |index| &response[..index]);
    #[cfg(target_env = "gnu")]
    {
        let Ok(response) = CString::new(response) else {
            return false;
        };
        // SAFETY: response is NUL-terminated and rpmatch does not retain it.
        unsafe { rpmatch(response.as_ptr()) == 1 }
    }
    #[cfg(not(target_env = "gnu"))]
    {
        let response = response.strip_suffix(b"\n").unwrap_or(response);
        let response = response.strip_suffix(b"\r").unwrap_or(response);
        response.eq_ignore_ascii_case(b"y") || response.eq_ignore_ascii_case(b"yes")
    }
}

pub struct ConsoleObserver {
    verbose: bool,
    progress: bool,
    terminal: bool,
    output_error: Option<io::Error>,
    diagnostic_error: Option<io::Error>,
    scan_started: Option<Instant>,
    last_scan_progress: Option<Instant>,
    pending_scan_progress: Option<ScanSummary>,
    scan_total: Option<ScanSummary>,
}

impl ConsoleObserver {
    #[must_use]
    pub fn new(verbose: bool, progress: ProgressMode) -> Self {
        let terminal = io::stderr().is_terminal();
        let progress = match progress {
            ProgressMode::Never => false,
            ProgressMode::Auto => terminal,
            ProgressMode::Always => true,
        };
        Self {
            verbose,
            progress,
            terminal,
            output_error: None,
            diagnostic_error: None,
            scan_started: None,
            last_scan_progress: None,
            pending_scan_progress: None,
            scan_total: None,
        }
    }

    pub fn finish(&mut self, result: &ExecutionResult) -> io::Result<()> {
        if self.progress {
            let item_count = if result.totals_complete {
                format!(
                    "{} item{}",
                    result.items,
                    if result.items == 1 { "" } else { "s" }
                )
            } else {
                format!(
                    "at least {} item{}",
                    result.items,
                    if result.items == 1 { "" } else { "s" }
                )
            };
            let byte_count = if result.totals_complete {
                format_bytes(result.bytes)
            } else {
                format!("at least {}", format_bytes(result.bytes))
            };
            self.write_diagnostic(
                format_args!(
                    "rmu: summary: {}, {}, {}, {} failure{}, {} skipped{}",
                    item_count,
                    byte_count,
                    format_duration(result.elapsed),
                    result.failures.len(),
                    if result.failures.len() == 1 { "" } else { "s" },
                    result.skipped,
                    if result.totals_complete {
                        ""
                    } else {
                        "; totals incomplete"
                    }
                ),
                true,
            );
        }
        if let Some(error) = self.output_error.take() {
            return Err(error);
        }
        if self.verbose {
            io::stdout().flush()?;
        }
        if let Some(error) = self.diagnostic_error.take() {
            return Err(error);
        }
        io::stderr().flush()?;
        Ok(())
    }

    fn write_diagnostic(&mut self, arguments: std::fmt::Arguments<'_>, newline: bool) {
        if self.diagnostic_error.is_some() {
            return;
        }
        let mut stderr = io::stderr().lock();
        let result = if newline {
            writeln!(stderr, "{arguments}")
        } else {
            write!(stderr, "{arguments}")
        };
        if let Err(error) = result.and_then(|()| stderr.flush()) {
            self.diagnostic_error = Some(error);
        }
    }

    fn report_removed_empty_directory(&mut self, operand: &OsString) {
        if self.progress {
            self.write_diagnostic(
                format_args!(
                    "rmu: permanently removed empty directory {}",
                    quote(operand)
                ),
                true,
            );
        }
        if self.verbose && self.output_error.is_none() {
            let mut stdout = io::stdout().lock();
            if let Err(error) = writeln!(stdout, "removed directory {}", quote(operand)) {
                self.output_error = Some(error);
            }
        }
    }

    fn report_scan_progress(&mut self, summary: &ScanSummary) {
        let elapsed = self
            .scan_started
            .map_or(Duration::ZERO, |started| started.elapsed());
        let status = scan_progress_status(summary, self.scan_total.as_ref(), elapsed);
        if self.terminal {
            self.write_diagnostic(format_args!("\r{status}"), false);
        } else {
            self.write_diagnostic(format_args!("{status}"), true);
        }
    }

    fn report_scan_finished(&mut self, summary: &ScanSummary) {
        if self.terminal {
            self.write_diagnostic(format_args!("\r\x1b[2K"), false);
        }
        let status = if summary.complete() {
            format!(
                "rmu: scan complete: {} entr{}, {}",
                summary.entries,
                if summary.entries == 1 { "y" } else { "ies" },
                format_bytes(summary.bytes)
            )
        } else {
            format!(
                "rmu: scan incomplete: at least {} entr{}, at least {}, {} unreadable",
                summary.entries,
                if summary.entries == 1 { "y" } else { "ies" },
                format_bytes(summary.bytes),
                summary.unreadable
            )
        };
        self.write_diagnostic(format_args!("{status}"), true);
    }
}

fn scan_progress_status(
    summary: &ScanSummary,
    total: Option<&ScanSummary>,
    elapsed: Duration,
) -> String {
    let item_rate = rate_per_second(summary.entries, elapsed);
    let byte_rate = rate_per_second(summary.bytes, elapsed);
    if let Some(total) = total.filter(|_| summary.complete()) {
        let item_eta = estimate_remaining(total.entries.saturating_sub(summary.entries), item_rate);
        let byte_eta = estimate_remaining(total.bytes.saturating_sub(summary.bytes), byte_rate);
        let eta = maximum_known_duration(item_eta, byte_eta)
            .map_or_else(|| "unavailable".into(), format_duration);
        format!(
            "rmu: scanned {}/{} items ({}/{}) at {} items/s, {}/s, ETA {}",
            summary.entries,
            total.entries,
            format_bytes(summary.bytes),
            format_bytes(total.bytes),
            format_unit_rate(item_rate),
            format_byte_rate(byte_rate),
            eta
        )
    } else {
        format!(
            "rmu: scanned at least {} item{} ({}) at {} items/s, {}/s, ETA unavailable",
            summary.entries,
            if summary.entries == 1 { "" } else { "s" },
            format_bytes(summary.bytes),
            format_unit_rate(item_rate),
            format_byte_rate(byte_rate)
        )
    }
}

impl Observer for ConsoleObserver {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::ScanPlanning { operand, .. } if self.progress => {
                self.scan_started = Some(Instant::now());
                self.last_scan_progress = None;
                self.pending_scan_progress = None;
                self.scan_total = None;
                self.write_diagnostic(
                    format_args!("rmu: counting {} for progress", quote(operand)),
                    true,
                );
            }
            Event::ScanStarted { operand, total, .. } if self.progress => {
                if self.terminal {
                    self.write_diagnostic(format_args!("\r\x1b[2K"), false);
                }
                self.scan_started = Some(Instant::now());
                self.last_scan_progress = None;
                self.pending_scan_progress = None;
                self.scan_total.clone_from(total);
                if let Some(total) = total {
                    self.write_diagnostic(
                        format_args!(
                            "rmu: scanning {} ({} item{}, {})",
                            quote(operand),
                            total.entries,
                            if total.entries == 1 { "" } else { "s" },
                            format_bytes(total.bytes)
                        ),
                        true,
                    );
                } else {
                    self.write_diagnostic(format_args!("rmu: scanning {}", quote(operand)), true);
                }
            }
            Event::ScanProgress { summary, .. } if self.progress => {
                let now = Instant::now();
                if self.last_scan_progress.is_none_or(|previous| {
                    now.duration_since(previous) >= Duration::from_millis(200)
                }) {
                    self.last_scan_progress = Some(now);
                    self.pending_scan_progress = None;
                    self.report_scan_progress(summary);
                } else {
                    self.pending_scan_progress = Some(summary.clone());
                }
            }
            Event::ScanFinished { summary, .. } if self.progress => {
                self.pending_scan_progress = None;
                self.report_scan_finished(summary);
            }
            Event::ScanFailed { message, .. } if self.progress => {
                if let Some(summary) = self.pending_scan_progress.take() {
                    self.report_scan_progress(&summary);
                }
                self.write_diagnostic(
                    format_args!("rmu: progress scan unavailable: {message}"),
                    true,
                );
            }
            Event::Moving { operand, .. } if self.progress => {
                self.write_diagnostic(
                    format_args!("rmu: moving {} into Trash", quote(operand)),
                    true,
                );
            }
            Event::Trashed { operand, receipt } => {
                if self.progress {
                    self.write_diagnostic(format_args!("rmu: moved {}", quote(operand)), true);
                }
                if self.verbose && self.output_error.is_none() {
                    let mut stdout = io::stdout().lock();
                    if let Err(error) = writeln!(stdout, "trashed {}", quote(operand)) {
                        self.output_error = Some(error);
                    }
                }
                for warning in &receipt.warnings {
                    self.write_diagnostic(format_args!("rmu: warning: {warning}"), true);
                }
            }
            Event::RemovedEmptyDirectory { operand, .. } => {
                self.report_removed_empty_directory(operand);
            }
            Event::RemoveEmptyDirectoryFailed { operand, message } => {
                self.write_diagnostic(
                    format_args!("rmu: cannot remove directory {}: {message}", quote(operand)),
                    true,
                );
            }
            Event::Failed { operand, message } => {
                self.write_diagnostic(
                    format_args!("rmu: cannot trash {}: {message}", quote(operand)),
                    true,
                );
            }
            Event::Warning { message } => {
                self.write_diagnostic(format_args!("rmu: warning: {message}"), true);
            }
            _ => {}
        }
    }

    fn wants_scan(&self) -> bool {
        self.progress
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    let mut decimal = 0;
    while value >= 1024 && unit < UNITS.len() - 1 {
        decimal = value % 1024 * 10 / 1024;
        value /= 1024;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value}.{decimal} {}", UNITS[unit])
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let tenths = duration.subsec_millis() / 100;
    if seconds >= 3600 {
        format!(
            "{}h{:02}m{:02}s",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}.{tenths}s")
    }
}

fn rate_per_second(value: u64, elapsed: Duration) -> u128 {
    let milliseconds = elapsed.as_millis().max(1);
    u128::from(value).saturating_mul(1_000_000) / milliseconds
}

fn estimate_remaining(remaining: u64, rate: u128) -> Option<Duration> {
    if remaining == 0 {
        return Some(Duration::ZERO);
    }
    if rate == 0 {
        return None;
    }
    let milliseconds = u128::from(remaining)
        .saturating_mul(1_000_000)
        .div_ceil(rate);
    Some(Duration::from_millis(
        u64::try_from(milliseconds).unwrap_or(u64::MAX),
    ))
}

fn maximum_known_duration(left: Option<Duration>, right: Option<Duration>) -> Option<Duration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        _ => None,
    }
}

fn format_unit_rate(rate: u128) -> String {
    format!("{}.{:01}", rate / 1000, rate % 1000 / 100)
}

fn format_byte_rate(rate: u128) -> String {
    if rate < 1000 {
        format!("{} B", format_unit_rate(rate))
    } else {
        let rounded = rate.saturating_add(500) / 1000;
        format_bytes(u64::try_from(rounded).unwrap_or(u64::MAX))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub operand: OsString,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub trashed: usize,
    pub removed: usize,
    pub skipped: usize,
    pub items: u64,
    pub bytes: u64,
    pub elapsed: Duration,
    pub failures: Vec<Failure>,
    pub totals_complete: bool,
}

impl Default for ExecutionResult {
    fn default() -> Self {
        Self {
            trashed: 0,
            removed: 0,
            skipped: 0,
            items: 0,
            bytes: 0,
            elapsed: Duration::ZERO,
            failures: Vec::new(),
            totals_complete: true,
        }
    }
}

impl ExecutionResult {
    #[must_use]
    pub fn success(&self) -> bool {
        self.failures.is_empty()
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn execute(
    invocation: &Invocation,
    backend: &TrashBackend,
    prompter: &mut impl Prompter,
    observer: &mut impl Observer,
) -> ExecutionResult {
    execute_with_backend_source(
        invocation,
        &mut BackendSource::Borrowed(backend),
        prompter,
        observer,
    )
}

pub fn execute_from_environment(
    invocation: &Invocation,
    prompter: &mut impl Prompter,
    observer: &mut impl Observer,
) -> ExecutionResult {
    execute_with_backend_source(
        invocation,
        &mut BackendSource::Environment(None),
        prompter,
        observer,
    )
}

enum BackendSource<'a> {
    Borrowed(&'a TrashBackend),
    Environment(Option<Result<TrashBackend, String>>),
}

impl BackendSource<'_> {
    fn get(&mut self) -> Result<&TrashBackend, String> {
        match self {
            Self::Borrowed(backend) => Ok(backend),
            Self::Environment(backend) => {
                if backend.is_none() {
                    *backend =
                        Some(TrashBackend::from_environment().map_err(|error| error.to_string()));
                }
                backend
                    .as_ref()
                    .expect("environment backend was initialized")
                    .as_ref()
                    .map_err(Clone::clone)
            }
        }
    }
}

fn execute_with_backend_source(
    invocation: &Invocation,
    backend: &mut BackendSource<'_>,
    prompter: &mut impl Prompter,
    observer: &mut impl Observer,
) -> ExecutionResult {
    let started = Instant::now();
    let mut result = ExecutionResult::default();
    let mut cache_updates = Vec::new();

    if invocation.prompt == PromptMode::Once
        && (invocation.operands.len() > 3 || invocation.recursive)
    {
        let prompt = if invocation.recursive {
            format!(
                "rmu: remove {} argument{} recursively? ",
                invocation.operands.len(),
                if invocation.operands.len() == 1 {
                    ""
                } else {
                    "s"
                }
            )
        } else {
            format!("rmu: remove {} arguments? ", invocation.operands.len())
        };
        match prompter.confirm(&prompt) {
            Ok(true) => {}
            Ok(false) => {
                result.skipped = invocation.operands.len();
                result.elapsed = started.elapsed();
                return result;
            }
            Err(error) => {
                let failure = Failure {
                    operand: OsString::from("<stdin>"),
                    message: format!("cannot read response: {error}"),
                };
                observer.observe(&Event::Failed {
                    operand: failure.operand.clone(),
                    message: failure.message.clone(),
                });
                result.failures.push(failure);
                result.elapsed = started.elapsed();
                return result;
            }
        }
    }

    for operand in &invocation.operands {
        match process_operand(invocation, backend, prompter, observer, operand) {
            Ok(OperandOutcome::Trashed {
                items,
                bytes,
                totals_complete,
                cache_update,
            }) => {
                result.trashed += 1;
                result.items = result.items.saturating_add(items);
                result.bytes = result.bytes.saturating_add(bytes);
                result.totals_complete &= totals_complete;
                cache_updates.extend(cache_update);
            }
            Ok(OperandOutcome::Removed {
                items,
                bytes,
                totals_complete,
            }) => {
                result.removed += 1;
                result.items = result.items.saturating_add(items);
                result.bytes = result.bytes.saturating_add(bytes);
                result.totals_complete &= totals_complete;
            }
            Ok(OperandOutcome::Skipped) => result.skipped += 1,
            Err(OperandError::Reported(failure)) => result.failures.push(failure),
            Err(OperandError::NeedsReport(message)) => {
                observer.observe(&Event::Failed {
                    operand: operand.clone(),
                    message: message.clone(),
                });
                result.failures.push(Failure {
                    operand: operand.clone(),
                    message,
                });
            }
        }
    }
    if !cache_updates.is_empty() {
        if let Ok(backend) = backend.get() {
            for message in backend.apply_directory_size_updates(cache_updates) {
                observer.observe(&Event::Warning { message });
            }
        }
    }
    result.elapsed = started.elapsed();
    result
}

enum OperandOutcome {
    Trashed {
        items: u64,
        bytes: u64,
        totals_complete: bool,
        cache_update: Option<DirectorySizeUpdate>,
    },
    Removed {
        items: u64,
        bytes: u64,
        totals_complete: bool,
    },
    Skipped,
}

enum OperandError {
    NeedsReport(String),
    Reported(Failure),
}

impl From<String> for OperandError {
    fn from(message: String) -> Self {
        Self::NeedsReport(message)
    }
}

fn process_operand(
    invocation: &Invocation,
    backend: &mut BackendSource<'_>,
    prompter: &mut impl Prompter,
    observer: &mut impl Observer,
    operand: &OsString,
) -> Result<OperandOutcome, OperandError> {
    let operand_path = Path::new(operand);
    let metadata = match fs::symlink_metadata(operand_path) {
        Ok(metadata) => metadata,
        Err(error) if invocation.force && is_nonexistent(&error) => {
            return Ok(OperandOutcome::Skipped);
        }
        Err(error) => return Err(error.to_string().into()),
    };
    reject_dot_components(operand_path)?;
    reject_trailing_slash_symlink(operand_path, invocation.recursive)?;
    let identity = SourceIdentity::from_metadata(&metadata);
    let path = resolve_without_following_leaf(operand_path).map_err(|error| error.to_string())?;
    let is_directory = metadata.file_type().is_dir();

    if is_directory && !invocation.recursive && !invocation.dir {
        return Err("Is a directory".to_string().into());
    }
    check_root_policy(invocation, &path, &metadata)?;
    observer.observe(&Event::Validated {
        operand: operand.clone(),
        path: path.clone(),
    });

    let prompt = match invocation.prompt {
        PromptMode::Always => true,
        PromptMode::Default => {
            prompter.stdin_is_terminal() && !metadata.file_type().is_symlink() && !writable(&path)
        }
        PromptMode::Never | PromptMode::Once => false,
    };
    if prompt {
        let question = if is_directory && invocation.dir && !invocation.recursive {
            format!("rmu: remove directory {}? ", quote(operand))
        } else {
            format!("rmu: remove {}? ", quote(operand))
        };
        match prompter.confirm(&question) {
            Ok(true) => {}
            Ok(false) => return Ok(OperandOutcome::Skipped),
            Err(error) => return Err(format!("cannot read response: {error}").into()),
        }
    }

    if is_directory && invocation.dir && !invocation.recursive {
        if let Err(error) = remove_empty_directory_verified(&path, identity) {
            let message = error.to_string();
            observer.observe(&Event::RemoveEmptyDirectoryFailed {
                operand: operand.clone(),
                message: message.clone(),
            });
            return Err(OperandError::Reported(Failure {
                operand: operand.clone(),
                message,
            }));
        }
        observer.observe(&Event::RemovedEmptyDirectory {
            operand: operand.clone(),
            path,
        });
        return Ok(OperandOutcome::Removed {
            items: 1,
            bytes: metadata.blocks().saturating_mul(512),
            totals_complete: true,
        });
    }

    let backend = backend.get()?;
    check_one_file_system(invocation, &path, is_directory)?;

    let scan_summary = if is_directory {
        scan_directory(operand, &path, observer)
    } else {
        None
    };
    let directory_size = scan_summary
        .as_ref()
        .filter(|summary| summary.complete())
        .map(|summary| summary.bytes);
    check_one_file_system(invocation, &path, is_directory)?;

    observer.observe(&Event::Moving {
        operand: operand.clone(),
        path: path.clone(),
    });
    let receipt = backend
        .trash_verified_deferred_cache(&path, identity, directory_size)
        .map_err(|error| error.to_string())?;
    let cache_update = receipt.cache_update.clone();
    observer.observe(&Event::Trashed {
        operand: operand.clone(),
        receipt,
    });
    let (items, bytes, totals_complete) = scan_summary.map_or_else(
        || (1, metadata.blocks().saturating_mul(512), !is_directory),
        |summary| (summary.entries, summary.bytes, summary.complete()),
    );
    Ok(OperandOutcome::Trashed {
        items,
        bytes,
        totals_complete,
        cache_update,
    })
}

fn is_nonexistent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn check_one_file_system(
    invocation: &Invocation,
    path: &Path,
    is_directory: bool,
) -> Result<(), String> {
    if is_directory
        && invocation.one_file_system
        && TrashBackend::contains_nested_mount(path).map_err(|error| error.to_string())?
    {
        return Err(
            "--one-file-system found a nested mount; refusing the whole operand because Trash cannot represent a partial directory move"
                .into(),
        );
    }
    Ok(())
}

fn scan_directory(
    operand: &OsString,
    path: &Path,
    observer: &mut impl Observer,
) -> Option<ScanSummary> {
    let report_progress = observer.wants_scan();
    let planned = if report_progress {
        observer.observe(&Event::ScanPlanning {
            operand: operand.clone(),
            path: path.to_path_buf(),
        });
        match scan_tree(path, |summary| {
            observer.observe(&Event::ScanProgress {
                operand: operand.clone(),
                summary: summary.clone(),
            });
        }) {
            Ok(summary) if summary.complete() => Some(summary),
            Ok(summary) => {
                observer.observe(&Event::ScanFailed {
                    operand: operand.clone(),
                    message: format!(
                        "count is incomplete ({} unreadable); totals are lower bounds and ETA is unavailable",
                        summary.unreadable
                    ),
                });
                None
            }
            Err(error) => {
                observer.observe(&Event::ScanFailed {
                    operand: operand.clone(),
                    message: format!("cannot count tree for ETA: {error}"),
                });
                None
            }
        }
    } else {
        None
    };
    if report_progress {
        observer.observe(&Event::ScanStarted {
            operand: operand.clone(),
            path: path.to_path_buf(),
            total: planned,
        });
    }
    let scan = scan_tree(path, |summary| {
        if report_progress {
            observer.observe(&Event::ScanProgress {
                operand: operand.clone(),
                summary: summary.clone(),
            });
        }
    });
    match scan {
        Ok(summary) => {
            if report_progress {
                observer.observe(&Event::ScanFinished {
                    operand: operand.clone(),
                    summary: summary.clone(),
                });
            }
            Some(summary)
        }
        Err(error) => {
            if report_progress {
                observer.observe(&Event::ScanFailed {
                    operand: operand.clone(),
                    message: error.to_string(),
                });
            }
            None
        }
    }
}

fn reject_dot_components(path: &Path) -> Result<(), String> {
    let bytes = path.as_os_str().as_bytes();
    let without_trailing_slashes = bytes.strip_suffix(b"/").map_or(bytes, |mut trimmed| {
        while let Some(next) = trimmed.strip_suffix(b"/") {
            trimmed = next;
        }
        trimmed
    });
    let final_component = without_trailing_slashes
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or_default();
    if matches!(final_component, b"." | b"..") {
        Err("refusing to remove '.' or '..' directory".into())
    } else {
        Ok(())
    }
}

fn reject_trailing_slash_symlink(path: &Path, recursive: bool) -> Result<(), String> {
    let bytes = path.as_os_str().as_bytes();
    if !bytes.ends_with(b"/") {
        return Ok(());
    }
    let trimmed = bytes.strip_suffix(b"/").map_or(bytes, |mut value| {
        while let Some(next) = value.strip_suffix(b"/") {
            value = next;
        }
        value
    });
    let trimmed = Path::new(std::ffi::OsStr::from_bytes(trimmed));
    let Ok(metadata) = fs::symlink_metadata(trimmed) else {
        return Ok(());
    };
    if !metadata.file_type().is_symlink() {
        return Ok(());
    }
    if !recursive && fs::metadata(trimmed).is_ok_and(|target| target.is_dir()) {
        Err("Is a directory".into())
    } else {
        Err("Not a directory".into())
    }
}

fn resolve_without_following_leaf(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env_current_dir()?.join(path)
    };
    if absolute.file_name().is_none() {
        return absolute.canonicalize();
    }
    let leaf = absolute
        .file_name()
        .ok_or_else(|| io::Error::other("path has no file name"))?;
    let parent = absolute
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    Ok(parent.canonicalize()?.join(leaf))
}

fn env_current_dir() -> io::Result<PathBuf> {
    std::env::current_dir()
}

fn check_root_policy(
    invocation: &Invocation,
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<(), String> {
    if path == Path::new("/") {
        return if invocation.recursive && invocation.preserve_root != PreserveRoot::Disabled {
            Err("it is dangerous to operate recursively on '/'".into())
        } else {
            Err("the filesystem root cannot be moved into Trash".into())
        };
    }
    if invocation.preserve_root == PreserveRoot::All {
        let parent = path
            .parent()
            .ok_or_else(|| "path has no parent".to_string())?;
        let parent_device = fs::metadata(parent)
            .map_err(|error| error.to_string())?
            .dev();
        if metadata.dev() != parent_device {
            return Err("operand is on a different filesystem from its parent".into());
        }
    }
    Ok(())
}

fn writable(path: &Path) -> bool {
    effective_access(path, AccessMode::Write).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_dot_and_dot_dot_after_trailing_slashes() {
        assert!(reject_dot_components(Path::new(".")).is_err());
        assert!(reject_dot_components(Path::new("foo/..")).is_err());
        assert!(reject_dot_components(Path::new("foo/.")).is_err());
    }

    #[test]
    fn c_locale_confirmation_accepts_yes_and_rejects_no() {
        assert!(response_is_affirmative(b"y"));
        assert!(response_is_affirmative(b"yes"));
        assert!(!response_is_affirmative(b"n"));
        assert!(!response_is_affirmative(b"invalid"));
        assert!(!response_is_affirmative(b"\xff"));
        assert!(!response_is_affirmative(b"  y\n"));
        assert!(response_is_affirmative(b"y\0n\n"));
    }

    #[test]
    fn sub_unit_rates_keep_a_nonzero_eta() {
        let rate = rate_per_second(1, Duration::from_secs(4));
        assert_eq!(format_unit_rate(rate), "0.2");
        assert_eq!(estimate_remaining(3, rate), Some(Duration::from_secs(12)));
    }

    #[test]
    fn zero_observed_rate_never_claims_zero_eta_for_remaining_work() {
        assert_eq!(estimate_remaining(1, 0), None);
        assert_eq!(estimate_remaining(0, 0), Some(Duration::ZERO));
        assert_eq!(maximum_known_duration(None, Some(Duration::ZERO)), None);
    }

    #[test]
    fn an_incomplete_execution_scan_never_uses_a_previous_exact_total() {
        let total = ScanSummary {
            entries: 10,
            bytes: 4096,
            ..ScanSummary::default()
        };
        let partial = ScanSummary {
            entries: 3,
            bytes: 1024,
            unreadable: 1,
            ..ScanSummary::default()
        };
        let status = scan_progress_status(&partial, Some(&total), Duration::from_secs(1));
        assert!(status.contains("at least 3 items"), "{status}");
        assert!(status.contains("ETA unavailable"), "{status}");
        assert!(!status.contains("3/10"), "{status}");
    }
}
