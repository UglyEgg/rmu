use std::io::{self, Write};
use std::process::ExitCode;

use crate::args::{Invocation, ParseOutcome};
use crate::engine::{ConsoleObserver, StdioPrompter, execute_from_environment, initialize_locale};
use crate::mode::{OperationMode, execute_gnu_rm, select_mode};

pub(crate) fn entrypoint() -> ExitCode {
    initialize_locale();
    let posixly_correct = std::env::var_os("POSIXLY_CORRECT").is_some();
    let selected = match select_mode(
        std::env::args_os().skip(1),
        std::env::var_os("RMU_MODE").as_deref(),
        posixly_correct,
    ) {
        Ok(selected) => selected,
        Err(error) => {
            write_stderr(format_args!("rmu: {error}\n"));
            write_stderr(format_args!("Try 'rmu --help' for more information.\n"));
            return ExitCode::from(1);
        }
    };
    if selected.mode == OperationMode::Delegate {
        if let Err(error) = execute_gnu_rm(&selected.arguments) {
            write_stderr(format_args!("rmu: {error}\n"));
        }
        return ExitCode::from(1);
    }
    let progress_environment = std::env::var_os("RMU_PROGRESS");
    let parsed = Invocation::parse_with_environment(
        selected.arguments,
        progress_environment.as_deref(),
        posixly_correct,
    );
    let invocation = match parsed {
        Ok(ParseOutcome::Run(invocation)) => invocation,
        Ok(ParseOutcome::Help) => {
            return write_stdout(crate::args::HELP.as_bytes());
        }
        Ok(ParseOutcome::Version) => {
            return write_stdout(format!("rmu {}\n", env!("CARGO_PKG_VERSION")).as_bytes());
        }
        Err(error) => {
            write_stderr(format_args!("rmu: {error}\n"));
            write_stderr(format_args!("Try 'rmu --help' for more information.\n"));
            return ExitCode::from(1);
        }
    };

    let mut prompter = StdioPrompter;
    let mut observer = ConsoleObserver::new(invocation.verbose, invocation.progress);
    let result = execute_from_environment(&invocation, &mut prompter, &mut observer);
    let output_result = observer.finish(&result);
    if let Err(error) = &output_result {
        write_stderr(format_args!("rmu: write error: {error}\n"));
    }

    if result.success() && output_result.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn write_stdout(contents: &[u8]) -> ExitCode {
    let result = io::stdout()
        .lock()
        .write_all(contents)
        .and_then(|()| io::stdout().flush());
    if let Err(error) = result {
        write_stderr(format_args!("rmu: write error: {error}\n"));
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn write_stderr(arguments: std::fmt::Arguments<'_>) {
    let mut stderr = io::stderr().lock();
    let _ = stderr.write_fmt(arguments).and_then(|()| stderr.flush());
}
