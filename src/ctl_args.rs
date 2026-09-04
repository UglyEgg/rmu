use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::args::ProgressMode;
use crate::display::escape_bytes;

pub const HELP: &str = r"Usage: rmuctl [GLOBAL-OPTION] COMMAND [OPTION]...
Inspect and manage the FreeDesktop Trash. No daemon or index is used.

Commands:
  list       list Trash entries and their stable IDs
  info       show complete information for one or more IDs
  restore    restore entries to their original path or --to directory
  purge      permanently remove selected entries
  empty      permanently remove all matching entries
  doctor     diagnose and safely recover damaged Trash state
  completions
             print a shell completion definition to standard output

Global options:
      --json                 emit JSON or JSON Lines
      --null                 terminate list records with NUL
  -q, --quiet                suppress successful action output
      --no-color             disable color (accepted; output is currently plain)
      --help                 display this help and exit
      --version              output version information and exit

List options:
      --filesystem PATH      select entries associated with PATH's filesystem
      --older-than DURATION  select entries older than 30d, 12h, 90m, or 10s
      --newer-than DURATION  select entries newer than DURATION
      --sort FIELD           name, date, size, or path
      --reverse              reverse the selected ordering

Restore options:
      --to DIRECTORY         restore below DIRECTORY instead of original path
      --conflict POLICY      fail or rename (default: fail)
      --parents              create missing destination parents
      --dry-run              report without changing anything

Purge/empty options:
      --dry-run              report without changing anything
      --yes                  do not prompt for permanent deletion
      --progress[=WHEN]      auto, always, or never
      --older-than DURATION  empty only entries older than DURATION
      --filesystem PATH      empty only the selected filesystem Trash

Doctor options:
      --repair-cache         rebuild FreeDesktop directorysizes caches
      --recover-orphans DIR  move orphaned payloads/metadata into DIR
      --dry-run              report repairs without changing anything

Completion arguments:
  completions SHELL COMMAND print one definition; SHELL is bash, zsh, or fish,
                             and COMMAND is rmu or rmuctl

Environment:
  RMU_PROGRESS             default purge/empty progress: auto, always, or never

Safety-changing defaults such as --yes, purge, repair, or overwrite are never
read from the environment.
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SortField {
    Name,
    Date,
    Size,
    Path,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConflictPolicy {
    Fail,
    Rename,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GlobalOptions {
    pub json: bool,
    pub null: bool,
    pub quiet: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ListOptions {
    pub filesystem: Option<PathBuf>,
    pub older_than: Option<u64>,
    pub newer_than: Option<u64>,
    pub sort: SortField,
    pub reverse: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreOptions {
    pub ids: Vec<String>,
    pub destination: Option<PathBuf>,
    pub conflict: ConflictPolicy,
    pub parents: bool,
    pub dry_run: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurgeOptions {
    pub ids: Vec<String>,
    pub dry_run: bool,
    pub yes: bool,
    pub progress: ProgressMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmptyOptions {
    pub filesystem: Option<PathBuf>,
    pub older_than: Option<u64>,
    pub dry_run: bool,
    pub yes: bool,
    pub progress: ProgressMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoctorOptions {
    pub repair_cache: bool,
    pub recover_orphans: Option<PathBuf>,
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionTarget {
    Rmu,
    Rmuctl,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionOptions {
    pub shell: CompletionShell,
    pub target: CompletionTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    List(ListOptions),
    Info(Vec<String>),
    Restore(RestoreOptions),
    Purge(PurgeOptions),
    Empty(EmptyOptions),
    Doctor(DoctorOptions),
    Completions(CompletionOptions),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub global: GlobalOptions,
    pub command: Command,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Run(Invocation),
    Help,
    Version,
}

impl Invocation {
    pub fn parse(
        arguments: impl IntoIterator<Item = OsString>,
        progress_environment: Option<&OsStr>,
    ) -> Result<ParseOutcome, String> {
        let mut arguments: Vec<OsString> = arguments.into_iter().collect();
        if arguments.is_empty() {
            return Err("missing command".into());
        }
        if arguments
            .iter()
            .any(|argument| argument.as_bytes() == b"--help")
        {
            return Ok(ParseOutcome::Help);
        }
        if arguments
            .iter()
            .any(|argument| argument.as_bytes() == b"--version")
        {
            return Ok(ParseOutcome::Version);
        }
        let mut global = GlobalOptions::default();
        let command_index = consume_leading_globals(&arguments, &mut global);
        if command_index >= arguments.len() {
            return Err("missing command".into());
        }
        let command = arguments.remove(command_index);
        arguments.drain(..command_index);
        let command_bytes = command.as_bytes();
        if command_bytes == b"--help" {
            return Ok(ParseOutcome::Help);
        }
        if command_bytes == b"--version" {
            return Ok(ParseOutcome::Version);
        }
        let command = match command_bytes {
            b"list" => Command::List(parse_list(&arguments, &mut global)?),
            b"info" => Command::Info(parse_ids(&arguments, &mut global, "info")?),
            b"restore" => Command::Restore(parse_restore(&arguments, &mut global)?),
            b"purge" => Command::Purge(parse_purge(&arguments, &mut global, progress_environment)?),
            b"empty" => Command::Empty(parse_empty(&arguments, &mut global, progress_environment)?),
            b"doctor" => Command::Doctor(parse_doctor(&arguments, &mut global)?),
            b"completions" => Command::Completions(parse_completions(&arguments)?),
            _ => {
                return Err(format!(
                    "unknown command '{}'",
                    escape_bytes(command.as_bytes())
                ));
            }
        };
        if global.null && !matches!(command, Command::List(_)) {
            return Err("--null is only valid with list".into());
        }
        if matches!(command, Command::Completions(_)) && global != GlobalOptions::default() {
            return Err("global output options are not valid with completions".into());
        }
        Ok(ParseOutcome::Run(Self { global, command }))
    }
}

fn consume_leading_globals(arguments: &[OsString], global: &mut GlobalOptions) -> usize {
    for (index, argument) in arguments.iter().enumerate() {
        if !consume_global(argument, global) {
            return index;
        }
    }
    arguments.len()
}

fn consume_global(argument: &OsStr, global: &mut GlobalOptions) -> bool {
    match argument.as_bytes() {
        b"--json" => global.json = true,
        b"--null" => global.null = true,
        b"-q" | b"--quiet" => global.quiet = true,
        b"--no-color" => {}
        bytes if bytes.starts_with(b"-") => return false,
        _ => return false,
    }
    true
}

fn consume_common(argument: &OsStr, global: &mut GlobalOptions) -> bool {
    consume_global(argument, global)
}

#[allow(clippy::too_many_lines)]
fn parse_list(arguments: &[OsString], global: &mut GlobalOptions) -> Result<ListOptions, String> {
    let mut options = ListOptions {
        filesystem: None,
        older_than: None,
        newer_than: None,
        sort: SortField::Date,
        reverse: false,
    };
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if consume_common(argument, global) {
            index += 1;
            continue;
        }
        match argument.as_bytes() {
            b"--reverse" => options.reverse = true,
            b"--filesystem" => {
                index += 1;
                options.filesystem = Some(PathBuf::from(required_value(
                    arguments,
                    index,
                    "--filesystem",
                )?));
            }
            b"--older-than" => {
                index += 1;
                options.older_than = Some(parse_duration_os(required_value(
                    arguments,
                    index,
                    "--older-than",
                )?)?);
            }
            b"--newer-than" => {
                index += 1;
                options.newer_than = Some(parse_duration_os(required_value(
                    arguments,
                    index,
                    "--newer-than",
                )?)?);
            }
            b"--sort" => {
                index += 1;
                options.sort = parse_sort(required_value(arguments, index, "--sort")?)?;
            }
            bytes if bytes.starts_with(b"--filesystem=") => {
                options.filesystem = Some(PathBuf::from(OsStr::from_bytes(
                    &bytes[b"--filesystem=".len()..],
                )));
            }
            bytes if bytes.starts_with(b"--older-than=") => {
                options.older_than = Some(parse_duration(&bytes[b"--older-than=".len()..])?);
            }
            bytes if bytes.starts_with(b"--newer-than=") => {
                options.newer_than = Some(parse_duration(&bytes[b"--newer-than=".len()..])?);
            }
            bytes if bytes.starts_with(b"--sort=") => {
                options.sort = parse_sort(OsStr::from_bytes(&bytes[b"--sort=".len()..]))?;
            }
            _ => return Err(unexpected(argument, "list")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_ids(
    arguments: &[OsString],
    global: &mut GlobalOptions,
    command: &str,
) -> Result<Vec<String>, String> {
    let mut ids = Vec::new();
    let mut operands = false;
    for argument in arguments {
        if !operands && argument.as_bytes() == b"--" {
            operands = true;
        } else if !operands && consume_common(argument, global) {
        } else if !operands && argument.as_bytes().starts_with(b"-") {
            return Err(unexpected(argument, command));
        } else {
            ids.push(id_string(argument)?);
        }
    }
    if ids.is_empty() {
        Err(format!("{command} requires at least one entry ID"))
    } else {
        Ok(ids)
    }
}

fn parse_restore(
    arguments: &[OsString],
    global: &mut GlobalOptions,
) -> Result<RestoreOptions, String> {
    let mut options = RestoreOptions {
        ids: Vec::new(),
        destination: None,
        conflict: ConflictPolicy::Fail,
        parents: false,
        dry_run: false,
    };
    let mut operands = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if !operands && argument.as_bytes() == b"--" {
            operands = true;
        } else if !operands && consume_common(argument, global) {
        } else if !operands {
            match argument.as_bytes() {
                b"--to" => {
                    index += 1;
                    options.destination =
                        Some(PathBuf::from(required_value(arguments, index, "--to")?));
                }
                b"--conflict" => {
                    index += 1;
                    options.conflict =
                        parse_conflict(required_value(arguments, index, "--conflict")?)?;
                }
                b"--parents" => options.parents = true,
                b"--dry-run" => options.dry_run = true,
                bytes if bytes.starts_with(b"--to=") => {
                    options.destination =
                        Some(PathBuf::from(OsStr::from_bytes(&bytes[b"--to=".len()..])));
                }
                bytes if bytes.starts_with(b"--conflict=") => {
                    options.conflict =
                        parse_conflict(OsStr::from_bytes(&bytes[b"--conflict=".len()..]))?;
                }
                bytes if bytes.starts_with(b"-") => {
                    return Err(unexpected(argument, "restore"));
                }
                _ => options.ids.push(id_string(argument)?),
            }
        } else {
            options.ids.push(id_string(argument)?);
        }
        index += 1;
    }
    if options.ids.is_empty() {
        Err("restore requires at least one entry ID".into())
    } else {
        Ok(options)
    }
}

fn parse_purge(
    arguments: &[OsString],
    global: &mut GlobalOptions,
    progress_environment: Option<&OsStr>,
) -> Result<PurgeOptions, String> {
    let mut options = PurgeOptions {
        ids: Vec::new(),
        dry_run: false,
        yes: false,
        progress: ProgressMode::Never,
    };
    let mut progress_set = false;
    let mut operands = false;
    for argument in arguments {
        if !operands && argument.as_bytes() == b"--" {
            operands = true;
        } else if !operands && consume_common(argument, global) {
        } else if !operands {
            match argument.as_bytes() {
                b"--dry-run" => options.dry_run = true,
                b"--yes" => options.yes = true,
                b"--progress" => {
                    options.progress = ProgressMode::Auto;
                    progress_set = true;
                }
                bytes if bytes.starts_with(b"--progress=") => {
                    options.progress = parse_progress(&bytes[b"--progress=".len()..])?;
                    progress_set = true;
                }
                bytes if bytes.starts_with(b"-") => {
                    return Err(unexpected(argument, "purge"));
                }
                _ => options.ids.push(id_string(argument)?),
            }
        } else {
            options.ids.push(id_string(argument)?);
        }
    }
    if !progress_set {
        options.progress = parse_progress_environment(progress_environment)?;
    }
    if options.ids.is_empty() {
        Err("purge requires at least one entry ID".into())
    } else {
        Ok(options)
    }
}

#[allow(clippy::too_many_lines)]
fn parse_empty(
    arguments: &[OsString],
    global: &mut GlobalOptions,
    progress_environment: Option<&OsStr>,
) -> Result<EmptyOptions, String> {
    let mut options = EmptyOptions {
        filesystem: None,
        older_than: None,
        dry_run: false,
        yes: false,
        progress: ProgressMode::Never,
    };
    let mut progress_set = false;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if consume_common(argument, global) {
            index += 1;
            continue;
        }
        match argument.as_bytes() {
            b"--dry-run" => options.dry_run = true,
            b"--yes" => options.yes = true,
            b"--progress" => {
                options.progress = ProgressMode::Auto;
                progress_set = true;
            }
            b"--filesystem" => {
                index += 1;
                options.filesystem = Some(PathBuf::from(required_value(
                    arguments,
                    index,
                    "--filesystem",
                )?));
            }
            b"--older-than" => {
                index += 1;
                options.older_than = Some(parse_duration_os(required_value(
                    arguments,
                    index,
                    "--older-than",
                )?)?);
            }
            bytes if bytes.starts_with(b"--progress=") => {
                options.progress = parse_progress(&bytes[b"--progress=".len()..])?;
                progress_set = true;
            }
            bytes if bytes.starts_with(b"--filesystem=") => {
                options.filesystem = Some(PathBuf::from(OsStr::from_bytes(
                    &bytes[b"--filesystem=".len()..],
                )));
            }
            bytes if bytes.starts_with(b"--older-than=") => {
                options.older_than = Some(parse_duration(&bytes[b"--older-than=".len()..])?);
            }
            _ => return Err(unexpected(argument, "empty")),
        }
        index += 1;
    }
    if !progress_set {
        options.progress = parse_progress_environment(progress_environment)?;
    }
    Ok(options)
}

fn parse_doctor(
    arguments: &[OsString],
    global: &mut GlobalOptions,
) -> Result<DoctorOptions, String> {
    let mut options = DoctorOptions {
        repair_cache: false,
        recover_orphans: None,
        dry_run: false,
    };
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if consume_common(argument, global) {
            index += 1;
            continue;
        }
        match argument.as_bytes() {
            b"--repair-cache" => options.repair_cache = true,
            b"--dry-run" => options.dry_run = true,
            b"--recover-orphans" => {
                index += 1;
                options.recover_orphans = Some(PathBuf::from(required_value(
                    arguments,
                    index,
                    "--recover-orphans",
                )?));
            }
            bytes if bytes.starts_with(b"--recover-orphans=") => {
                options.recover_orphans = Some(PathBuf::from(OsStr::from_bytes(
                    &bytes[b"--recover-orphans=".len()..],
                )));
            }
            _ => return Err(unexpected(argument, "doctor")),
        }
        index += 1;
    }
    Ok(options)
}

fn parse_completions(arguments: &[OsString]) -> Result<CompletionOptions, String> {
    if arguments.len() != 2 {
        return Err("completions requires SHELL and COMMAND (bash|zsh|fish and rmu|rmuctl)".into());
    }
    let shell = match arguments[0].as_bytes() {
        b"bash" => CompletionShell::Bash,
        b"zsh" => CompletionShell::Zsh,
        b"fish" => CompletionShell::Fish,
        value => {
            return Err(format!(
                "unknown completion shell '{}'; expected bash, zsh, or fish",
                escape_bytes(value)
            ));
        }
    };
    let target = match arguments[1].as_bytes() {
        b"rmu" => CompletionTarget::Rmu,
        b"rmuctl" => CompletionTarget::Rmuctl,
        value => {
            return Err(format!(
                "unknown completion command '{}'; expected rmu or rmuctl",
                escape_bytes(value)
            ));
        }
    };
    Ok(CompletionOptions { shell, target })
}

fn required_value<'a>(
    arguments: &'a [OsString],
    index: usize,
    option: &str,
) -> Result<&'a OsStr, String> {
    arguments
        .get(index)
        .map(OsString::as_os_str)
        .ok_or_else(|| format!("option {option} requires a value"))
}

fn id_string(value: &OsStr) -> Result<String, String> {
    let bytes = value.as_bytes();
    if bytes.len() == 32
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(value
            .to_str()
            .expect("an ASCII hexadecimal ID is valid UTF-8")
            .to_owned())
    } else {
        Err(format!(
            "invalid entry ID '{}'; expected 32 lowercase hexadecimal characters",
            escape_bytes(bytes)
        ))
    }
}

fn parse_sort(value: &OsStr) -> Result<SortField, String> {
    match value.as_bytes() {
        b"name" => Ok(SortField::Name),
        b"date" => Ok(SortField::Date),
        b"size" => Ok(SortField::Size),
        b"path" => Ok(SortField::Path),
        _ => Err(format!(
            "invalid sort field '{}'",
            escape_bytes(value.as_bytes())
        )),
    }
}

fn parse_conflict(value: &OsStr) -> Result<ConflictPolicy, String> {
    match value.as_bytes() {
        b"fail" => Ok(ConflictPolicy::Fail),
        b"rename" => Ok(ConflictPolicy::Rename),
        _ => Err(format!(
            "invalid conflict policy '{}'",
            escape_bytes(value.as_bytes())
        )),
    }
}

fn parse_duration_os(value: &OsStr) -> Result<u64, String> {
    parse_duration(value.as_bytes())
}

pub fn parse_duration(value: &[u8]) -> Result<u64, String> {
    if value.len() < 2 {
        return Err("duration must end in w, d, h, m, or s".into());
    }
    let (&suffix, number) = value
        .split_last()
        .ok_or_else(|| "duration is empty".to_string())?;
    let multiplier = match suffix {
        b'w' => 7 * 24 * 60 * 60,
        b'd' => 24 * 60 * 60,
        b'h' => 60 * 60,
        b'm' => 60,
        b's' => 1,
        _ => return Err("duration must end in w, d, h, m, or s".into()),
    };
    let number = std::str::from_utf8(number)
        .ok()
        .and_then(|number| number.parse::<u64>().ok())
        .ok_or_else(|| "duration must start with a nonnegative integer".to_string())?;
    number
        .checked_mul(multiplier)
        .ok_or_else(|| "duration is too large".into())
}

fn parse_progress_environment(value: Option<&OsStr>) -> Result<ProgressMode, String> {
    value.map_or(Ok(ProgressMode::Never), |value| {
        parse_progress(value.as_bytes()).map_err(|error| format!("RMU_PROGRESS: {error}"))
    })
}

fn parse_progress(value: &[u8]) -> Result<ProgressMode, String> {
    match value {
        b"never" => Ok(ProgressMode::Never),
        b"auto" => Ok(ProgressMode::Auto),
        b"always" => Ok(ProgressMode::Always),
        _ => Err(format!(
            "invalid progress mode '{}'; expected never, auto, or always",
            escape_bytes(value)
        )),
    }
}

fn unexpected(argument: &OsStr, command: &str) -> String {
    format!(
        "unexpected {command} argument '{}'",
        escape_bytes(argument.as_bytes())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations_without_dependencies() {
        assert_eq!(parse_duration(b"30d").unwrap(), 2_592_000);
        assert_eq!(parse_duration(b"12h").unwrap(), 43_200);
        assert!(parse_duration(b"tomorrow").is_err());
    }

    #[test]
    fn safety_flags_never_come_from_the_environment() {
        let parsed = Invocation::parse(
            [OsString::from("empty"), OsString::from("--dry-run")],
            Some(OsStr::new("always")),
        )
        .unwrap();
        let ParseOutcome::Run(invocation) = parsed else {
            panic!("expected runnable invocation");
        };
        let Command::Empty(options) = invocation.command else {
            panic!("expected empty command");
        };
        assert!(options.dry_run);
        assert!(!options.yes);
        assert_eq!(options.progress, ProgressMode::Always);
    }

    #[test]
    fn progress_environment_only_applies_to_progress_commands() {
        let parsed = Invocation::parse(
            [OsString::from("list")],
            Some(OsStr::new("not-a-progress-mode")),
        );
        assert!(parsed.is_ok());
        let parsed = Invocation::parse(
            [OsString::from("empty"), OsString::from("--dry-run")],
            Some(OsStr::new("not-a-progress-mode")),
        );
        assert!(parsed.is_err());
    }

    #[test]
    fn explicit_progress_overrides_even_an_invalid_environment_value() {
        let parsed = Invocation::parse(
            [
                OsString::from("purge"),
                OsString::from("--progress=always"),
                OsString::from("0123456789abcdef0123456789abcdef"),
            ],
            Some(OsStr::new("not-a-progress-mode")),
        )
        .unwrap();
        let ParseOutcome::Run(invocation) = parsed else {
            panic!("expected runnable invocation");
        };
        let Command::Purge(options) = invocation.command else {
            panic!("expected purge command");
        };
        assert_eq!(options.progress, ProgressMode::Always);
    }

    #[test]
    fn entry_ids_are_strict_lowercase_hex_and_terminal_safe() {
        assert!(id_string(OsStr::new("0123456789abcdef0123456789abcdef")).is_ok());
        assert!(id_string(OsStr::new("0123456789ABCDEF0123456789ABCDEF")).is_err());
        let error = id_string(OsStr::from_bytes(b"bad\n\x1b[31m")).unwrap_err();
        assert!(!error.contains('\n'));
        assert!(!error.contains('\u{1b}'));
        assert!(error.contains("\\n"));
        assert!(error.contains("\\x1B"));
    }

    #[test]
    fn completions_requires_a_known_shell_and_target() {
        let parsed = Invocation::parse(
            [
                OsString::from("completions"),
                OsString::from("zsh"),
                OsString::from("rmu"),
            ],
            None,
        )
        .unwrap();
        let ParseOutcome::Run(invocation) = parsed else {
            panic!("expected runnable invocation");
        };
        let Command::Completions(options) = invocation.command else {
            panic!("expected completions command");
        };
        assert_eq!(options.shell, CompletionShell::Zsh);
        assert_eq!(options.target, CompletionTarget::Rmu);

        let parsed = Invocation::parse(
            [
                OsString::from("completions"),
                OsString::from("powershell"),
                OsString::from("rmu"),
            ],
            None,
        );
        assert!(parsed.unwrap_err().contains("expected bash, zsh, or fish"));
    }

    #[test]
    fn completions_rejects_global_output_modes() {
        let parsed = Invocation::parse(
            [
                OsString::from("--json"),
                OsString::from("completions"),
                OsString::from("fish"),
                OsString::from("rmuctl"),
            ],
            None,
        );
        assert_eq!(
            parsed.unwrap_err(),
            "global output options are not valid with completions"
        );
    }

    #[test]
    fn help_and_version_are_process_wide_completion_selectors() {
        assert!(matches!(
            Invocation::parse(
                [
                    OsString::from("completions"),
                    OsString::from("zsh"),
                    OsString::from("rmu"),
                    OsString::from("--help"),
                ],
                None,
            ),
            Ok(ParseOutcome::Help)
        ));
        assert!(matches!(
            Invocation::parse(
                [
                    OsString::from("completions"),
                    OsString::from("fish"),
                    OsString::from("rmuctl"),
                    OsString::from("--"),
                    OsString::from("--version"),
                ],
                None,
            ),
            Ok(ParseOutcome::Version)
        ));
    }
}
