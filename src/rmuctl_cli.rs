use std::collections::HashSet;
use std::ffi::OsStr;
use std::io::{self, IsTerminal, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;

use crate::args::ProgressMode;
use crate::control::{ActionResult, AuthorizedInventory, DoctorReport, TrashEntry, TrashManager};
use crate::ctl_args::{
    Command, CompletionOptions, CompletionShell, CompletionTarget, GlobalOptions, Invocation,
    ParseOutcome,
};
use crate::display::{escape_bytes, quote};
use crate::engine::{Prompter, StdioPrompter, initialize_locale};

pub(crate) fn entrypoint() -> ExitCode {
    initialize_locale();
    let progress_environment = std::env::var_os("RMU_PROGRESS");
    let parsed = Invocation::parse(std::env::args_os().skip(1), progress_environment.as_deref());
    let invocation = match parsed {
        Ok(ParseOutcome::Run(invocation)) => invocation,
        Ok(ParseOutcome::Help) => return write_stdout(crate::ctl_args::HELP.as_bytes()),
        Ok(ParseOutcome::Version) => {
            return write_stdout(format!("rmuctl {}\n", env!("CARGO_PKG_VERSION")).as_bytes());
        }
        Err(error) => return usage_error(&error),
    };
    match run(invocation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            drop(write_stderr(format_args!("rmuctl: {error}\n")));
            ExitCode::from(1)
        }
    }
}

#[allow(clippy::too_many_lines)]
fn run(invocation: Invocation) -> Result<(), String> {
    if let Command::Completions(options) = &invocation.command {
        return write_completions(*options);
    }
    let manager = TrashManager::discover()?;
    let inventory = manager.inventory();
    for warning in inventory.warnings() {
        write_stderr(format_args!("rmuctl: warning: {warning}\n")).map_err(write_error)?;
    }
    match invocation.command {
        Command::List(options) => {
            let entries = manager.filter_list(&inventory, &options)?;
            render_list(&entries, &invocation.global)?;
            if inventory.is_complete() {
                Ok(())
            } else {
                Err("listing is partial because Trash inventory was incomplete".into())
            }
        }
        Command::Info(ids) => {
            let entries = manager.select(&inventory, &ids)?;
            let render_result = render_info(&entries, &invocation.global);
            combine_results(
                render_result,
                complete_inventory_result(&inventory, "information is partial"),
            )
        }
        Command::Restore(options) => {
            let authorized = manager.authorize_mutations(&inventory, "restore")?;
            let entries = manager.select(&inventory, &options.ids)?;
            let mut results = Vec::new();
            let mut failures = Vec::new();
            let mut cache_entries = Vec::new();
            let mut reserved_destinations = HashSet::new();
            for entry in entries {
                match authorized.restore(
                    entry,
                    options.destination.as_deref(),
                    options.conflict,
                    options.parents,
                    options.dry_run,
                    options.dry_run.then_some(&reserved_destinations),
                ) {
                    Ok(result) => {
                        if let Some(path) = &result.path {
                            reserved_destinations.insert(path.clone());
                        }
                        if !options.dry_run {
                            cache_entries.push(entry);
                        }
                        results.push(result);
                    }
                    Err(error) => failures.push(format!("{}: {error}", entry.id)),
                }
            }
            let mut cache_warnings = authorized.update_caches(&cache_entries);
            for result in &mut results {
                if let Some(warning) = cache_warnings.remove(&result.id) {
                    result.warnings.push(warning);
                }
            }
            let render_result = render_actions(
                if options.dry_run {
                    "would restore"
                } else {
                    "restored"
                },
                &results,
                &invocation.global,
            );
            let failure_result = finish_failures(&failures);
            combine_results(render_result, failure_result)
        }
        Command::Purge(options) => {
            let authorized = manager.authorize_mutations(&inventory, "purge")?;
            let entries = manager.select(&inventory, &options.ids)?;
            confirm_permanent("purge", entries.len(), options.yes, options.dry_run)?;
            run_purge(
                &authorized,
                &entries,
                options.dry_run,
                options.progress,
                &invocation.global,
            )
        }
        Command::Empty(options) => {
            let authorized = manager.authorize_mutations(&inventory, "empty")?;
            let entries = manager.filter_empty(
                &inventory,
                options.filesystem.as_deref(),
                options.older_than,
            )?;
            if entries.is_empty() {
                return Ok(());
            }
            confirm_permanent("empty", entries.len(), options.yes, options.dry_run)?;
            run_purge(
                &authorized,
                &entries,
                options.dry_run,
                options.progress,
                &invocation.global,
            )
        }
        Command::Doctor(options) => {
            let mutating = options.repair_cache || options.recover_orphans.is_some();
            let report = if mutating {
                manager
                    .authorize_mutations(&inventory, "run doctor repairs")?
                    .doctor(
                        options.repair_cache,
                        options.recover_orphans.as_deref(),
                        options.dry_run,
                    )
            } else {
                manager.diagnose(&inventory)?
            };
            render_doctor(&report, &invocation.global, options.dry_run)?;
            if report.unresolved.is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "doctor found {} unresolved problem(s)",
                    report.unresolved.len()
                ))
            }
        }
        Command::Completions(_) => {
            unreachable!("completion commands return before Trash discovery")
        }
    }
}

fn write_completions(options: CompletionOptions) -> Result<(), String> {
    let script = match (options.shell, options.target) {
        (CompletionShell::Bash, CompletionTarget::Rmu) => {
            include_bytes!("../completions/rmu.bash").as_slice()
        }
        (CompletionShell::Bash, CompletionTarget::Rmuctl) => {
            include_bytes!("../completions/rmuctl.bash").as_slice()
        }
        (CompletionShell::Zsh, CompletionTarget::Rmu) => {
            include_bytes!("../completions/_rmu").as_slice()
        }
        (CompletionShell::Zsh, CompletionTarget::Rmuctl) => {
            include_bytes!("../completions/_rmuctl").as_slice()
        }
        (CompletionShell::Fish, CompletionTarget::Rmu) => {
            include_bytes!("../completions/rmu.fish").as_slice()
        }
        (CompletionShell::Fish, CompletionTarget::Rmuctl) => {
            include_bytes!("../completions/rmuctl.fish").as_slice()
        }
    };
    let mut stdout = io::stdout().lock();
    stdout.write_all(script).map_err(write_error)?;
    stdout.flush().map_err(write_error)
}

fn complete_inventory_result(
    inventory: &crate::control::Inventory,
    incomplete_message: &str,
) -> Result<(), String> {
    if inventory.is_complete() {
        Ok(())
    } else {
        Err(format!(
            "{incomplete_message} because Trash inventory was incomplete"
        ))
    }
}

fn run_purge(
    authorized: &AuthorizedInventory<'_>,
    entries: &[&TrashEntry],
    dry_run: bool,
    progress: ProgressMode,
    global: &GlobalOptions,
) -> Result<(), String> {
    let progress_terminal = io::stderr().is_terminal();
    let show_progress = match progress {
        ProgressMode::Never => false,
        ProgressMode::Auto => progress_terminal,
        ProgressMode::Always => true,
    };
    let mut results = Vec::new();
    let mut failures = Vec::new();
    let mut cache_entries = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        match authorized.purge(entry, dry_run) {
            Ok(result) => {
                if !dry_run {
                    cache_entries.push(*entry);
                }
                results.push(result);
            }
            Err(error) => failures.push(format!("{}: {error}", entry.id)),
        }
        if show_progress {
            if progress_terminal {
                write_stderr(format_args!(
                    "\rrmuctl: {}/{} entries processed",
                    index + 1,
                    entries.len()
                ))
                .map_err(write_error)?;
            } else {
                write_stderr(format_args!(
                    "rmuctl: {}/{} entries processed\n",
                    index + 1,
                    entries.len()
                ))
                .map_err(write_error)?;
            }
        }
    }
    let mut cache_warnings = authorized.update_caches(&cache_entries);
    for result in &mut results {
        if let Some(warning) = cache_warnings.remove(&result.id) {
            result.warnings.push(warning);
        }
    }
    if show_progress && progress_terminal {
        write_stderr(format_args!("\n")).map_err(write_error)?;
    }
    let render_result = render_actions(
        if dry_run { "would purge" } else { "purged" },
        &results,
        global,
    );
    let failure_result = finish_failures(&failures);
    combine_results(render_result, failure_result)
}

fn combine_results(first: Result<(), String>, second: Result<(), String>) -> Result<(), String> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(format!("{first}; {second}")),
    }
}

fn confirm_permanent(
    operation: &str,
    count: usize,
    yes: bool,
    dry_run: bool,
) -> Result<(), String> {
    if yes || dry_run || count == 0 {
        return Ok(());
    }
    let mut prompter = StdioPrompter;
    if !prompter.stdin_is_terminal() {
        return Err(format!(
            "refusing to {operation} {count} entries without confirmation; use --yes in a non-interactive session"
        ));
    }
    let prompt = format!(
        "rmuctl: permanently {operation} {count} Trash entr{}? ",
        if count == 1 { "y" } else { "ies" }
    );
    if prompter
        .confirm(&prompt)
        .map_err(|error| format!("cannot read confirmation: {error}"))?
    {
        Ok(())
    } else {
        Err("operation was not confirmed".into())
    }
}

fn render_list(entries: &[&TrashEntry], global: &GlobalOptions) -> Result<(), String> {
    let terminator = if global.null { b'\0' } else { b'\n' };
    let mut stdout = io::stdout().lock();
    for entry in entries {
        if global.json {
            write_entry_json(&mut stdout, entry, false)?;
        } else {
            write!(
                stdout,
                "{}\t{}\t{}\t{}\t{}",
                entry.id,
                entry.deletion_date.as_deref().unwrap_or("-"),
                entry
                    .size
                    .map_or_else(|| "-".into(), |size| size.to_string()),
                entry.state.as_str(),
                entry.original_path.as_deref().map_or_else(
                    || "-".into(),
                    |path| escape_bytes(path.as_os_str().as_bytes())
                )
            )
            .map_err(write_error)?;
        }
        stdout.write_all(&[terminator]).map_err(write_error)?;
    }
    stdout.flush().map_err(write_error)
}

fn render_info(entries: &[&TrashEntry], global: &GlobalOptions) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    for (index, entry) in entries.iter().enumerate() {
        if global.json {
            write_entry_json(&mut stdout, entry, true)?;
            stdout.write_all(b"\n").map_err(write_error)?;
        } else {
            if index > 0 {
                stdout.write_all(b"\n").map_err(write_error)?;
            }
            writeln!(stdout, "ID: {}", entry.id).map_err(write_error)?;
            writeln!(stdout, "State: {}", entry.state.as_str()).map_err(write_error)?;
            writeln!(
                stdout,
                "Original: {}",
                entry
                    .original_path
                    .as_deref()
                    .map_or_else(|| "-".into(), |path| quote(path.as_os_str()))
            )
            .map_err(write_error)?;
            writeln!(
                stdout,
                "Deleted: {}",
                entry.deletion_date.as_deref().unwrap_or("-")
            )
            .map_err(write_error)?;
            writeln!(
                stdout,
                "Size: {}",
                entry
                    .size
                    .map_or_else(|| "-".into(), |size| size.to_string())
            )
            .map_err(write_error)?;
            writeln!(stdout, "Payload: {}", quote(entry.payload.as_os_str()))
                .map_err(write_error)?;
            writeln!(stdout, "Metadata: {}", quote(entry.info.as_os_str())).map_err(write_error)?;
            for problem in &entry.problems {
                writeln!(stdout, "Problem: {problem}").map_err(write_error)?;
            }
        }
    }
    stdout.flush().map_err(write_error)
}

fn write_entry_json(
    output: &mut impl Write,
    entry: &TrashEntry,
    complete: bool,
) -> Result<(), String> {
    write!(
        output,
        "{{\"id\":\"{}\",\"state\":\"{}\",\"name\":\"{}\",\"name_bytes_hex\":\"{}\",\"original_path\":{},\"original_path_bytes_hex\":{},\"deletion_date\":{},\"size\":{}",
        json_escape(entry.id.as_bytes()),
        entry.state.as_str(),
        json_escape(entry.name.as_bytes()),
        hex_bytes(entry.name.as_bytes()),
        json_os(entry.original_path.as_deref().map(Path::as_os_str)),
        json_hex(entry.original_path.as_deref().map(Path::as_os_str)),
        json_string(entry.deletion_date.as_deref()),
        entry.size.map_or_else(|| "null".into(), |size| size.to_string())
    )
    .map_err(write_error)?;
    if complete {
        write!(
            output,
            ",\"payload\":\"{}\",\"payload_bytes_hex\":\"{}\",\"metadata\":\"{}\",\"metadata_bytes_hex\":\"{}\",\"problems\":[",
            json_escape(entry.payload.as_os_str().as_bytes()),
            hex_bytes(entry.payload.as_os_str().as_bytes()),
            json_escape(entry.info.as_os_str().as_bytes()),
            hex_bytes(entry.info.as_os_str().as_bytes())
        )
        .map_err(write_error)?;
        for (index, problem) in entry.problems.iter().enumerate() {
            if index > 0 {
                output.write_all(b",").map_err(write_error)?;
            }
            write!(output, "\"{}\"", json_escape(problem.as_bytes())).map_err(write_error)?;
        }
        output.write_all(b"]").map_err(write_error)?;
    }
    output.write_all(b"}").map_err(write_error)
}

fn render_actions(
    verb: &str,
    actions: &[ActionResult],
    global: &GlobalOptions,
) -> Result<(), String> {
    for action in actions {
        for warning in &action.warnings {
            write_stderr(format_args!("rmuctl: warning: {}: {warning}\n", action.id))
                .map_err(write_error)?;
        }
    }
    if global.quiet {
        return Ok(());
    }
    let mut stdout = io::stdout().lock();
    for action in actions {
        if global.json {
            writeln!(
                stdout,
                "{{\"id\":\"{}\",\"action\":\"{}\",\"path\":{},\"path_bytes_hex\":{}}}",
                json_escape(action.id.as_bytes()),
                json_escape(verb.as_bytes()),
                json_os(action.path.as_deref().map(Path::as_os_str)),
                json_hex(action.path.as_deref().map(Path::as_os_str))
            )
            .map_err(write_error)?;
        } else if let Some(path) = &action.path {
            writeln!(
                stdout,
                "{verb} {} to {}",
                action.id,
                quote(path.as_os_str())
            )
            .map_err(write_error)?;
        } else {
            writeln!(stdout, "{verb} {}", action.id).map_err(write_error)?;
        }
    }
    stdout.flush().map_err(write_error)
}

fn render_doctor(
    report: &DoctorReport,
    global: &GlobalOptions,
    dry_run: bool,
) -> Result<(), String> {
    let mut stdout = io::stdout().lock();
    let unresolved: HashSet<_> = report
        .unresolved
        .iter()
        .map(|problem| &problem.key)
        .collect();
    let observed: HashSet<_> = report.observed.iter().map(|problem| &problem.key).collect();
    if global.json {
        for problem in &report.observed {
            let status = if unresolved.contains(&problem.key) {
                "unresolved"
            } else {
                "resolved"
            };
            writeln!(
                stdout,
                "{{\"kind\":\"problem\",\"status\":\"{status}\",\"message\":\"{}\"}}",
                json_escape(problem.message.as_bytes())
            )
            .map_err(write_error)?;
        }
        for problem in report
            .unresolved
            .iter()
            .filter(|problem| !observed.contains(&problem.key))
        {
            writeln!(
                stdout,
                "{{\"kind\":\"problem\",\"status\":\"unresolved\",\"message\":\"{}\"}}",
                json_escape(problem.message.as_bytes())
            )
            .map_err(write_error)?;
        }
    } else {
        for problem in &report.observed {
            let label = if unresolved.contains(&problem.key) {
                "problem"
            } else {
                "resolved"
            };
            writeln!(stdout, "{label}: {problem}").map_err(write_error)?;
        }
        for problem in report
            .unresolved
            .iter()
            .filter(|problem| !observed.contains(&problem.key))
        {
            writeln!(stdout, "unresolved: {problem}").map_err(write_error)?;
        }
        if report.observed.is_empty() && report.unresolved.is_empty() && report.actions.is_empty() {
            writeln!(stdout, "Trash state is healthy.").map_err(write_error)?;
        }
    }
    stdout.flush().map_err(write_error)?;
    render_actions(doctor_action_verb(dry_run), &report.actions, global)
}

fn doctor_action_verb(dry_run: bool) -> &'static str {
    if dry_run { "would repair" } else { "repaired" }
}

fn finish_failures(failures: &[String]) -> Result<(), String> {
    if failures.is_empty() {
        Ok(())
    } else {
        for failure in failures {
            write_stderr(format_args!("rmuctl: {failure}\n")).map_err(write_error)?;
        }
        Err(format!("{} operation(s) failed", failures.len()))
    }
}

fn json_os(value: Option<&OsStr>) -> String {
    value.map_or_else(
        || "null".into(),
        |value| format!("\"{}\"", json_escape(value.as_bytes())),
    )
}

fn json_string(value: Option<&str>) -> String {
    value.map_or_else(
        || "null".into(),
        |value| format!("\"{}\"", json_escape(value.as_bytes())),
    )
}

fn json_hex(value: Option<&OsStr>) -> String {
    value.map_or_else(
        || "null".into(),
        |value| format!("\"{}\"", hex_bytes(value.as_bytes())),
    )
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn json_escape(bytes: &[u8]) -> String {
    let mut output = String::new();
    for &byte in bytes {
        match byte {
            b'"' => output.push_str("\\\""),
            b'\\' => output.push_str("\\\\"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0x20..=0x7e => output.push(char::from(byte)),
            // Unix paths are byte strings. This is a visible display escape;
            // use the corresponding *_bytes_hex field for exact bytes.
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                output.push('\\');
                output.push('\\');
                output.push('x');
                output.push(char::from(HEX[usize::from(byte >> 4)]));
                output.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    output
}

#[allow(clippy::needless_pass_by_value)]
fn write_error(error: io::Error) -> String {
    format!("write error: {error}")
}

fn usage_error(error: &str) -> ExitCode {
    drop(write_stderr(format_args!("rmuctl: {error}\n")));
    drop(write_stderr(format_args!(
        "Try 'rmuctl --help' for more information.\n"
    )));
    ExitCode::from(1)
}

fn write_stdout(contents: &[u8]) -> ExitCode {
    let result = io::stdout()
        .lock()
        .write_all(contents)
        .and_then(|()| io::stdout().flush());
    if let Err(error) = result {
        drop(write_stderr(format_args!("rmuctl: write error: {error}\n")));
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn write_stderr(arguments: std::fmt::Arguments<'_>) -> io::Result<()> {
    let mut stderr = io::stderr().lock();
    stderr.write_fmt(arguments).and_then(|()| stderr.flush())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_byte_hex_distinguishes_raw_bytes_from_visible_escape_text() {
        assert_eq!(hex_bytes(b"\xff"), "ff");
        assert_eq!(hex_bytes(br"\xFF"), "5c784646");
        assert_ne!(hex_bytes(b"\xff"), hex_bytes(br"\xFF"));
    }

    #[test]
    fn doctor_dry_run_uses_a_future_tense_action() {
        assert_eq!(doctor_action_verb(true), "would repair");
        assert_eq!(doctor_action_verb(false), "repaired");
    }
}
