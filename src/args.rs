use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::OsStrExt;

use crate::display::escape_bytes;

pub const HELP: &str = r"Usage: rmu [OPTION]... [FILE]...
Move each FILE into the FreeDesktop Trash used by Linux desktop environments.

GNU rm-compatible options:
  -f, --force                 ignore nonexistent files and missing operands,
                              and never prompt
  -i                          prompt before every removal
  -I                          prompt once before removing more than three files,
                              or when removing recursively
      --interactive[=WHEN]    prompt according to WHEN: never, once, or always
  -d, --dir                   remove empty directories permanently
      --one-file-system       refuse a directory containing nested mounts
      --no-preserve-root      disable GNU rm's root policy (the filesystem
                              root still cannot be moved into Trash)
      --preserve-root[=all]   do not remove '/' (default); with 'all', reject
                              operands on a different filesystem from parent
  -r, -R, --recursive         remove directories and their contents
  -v, --verbose               explain what is being done
      --help                  display this help and exit
      --version               output version information and exit

RMU modes and enhancement:
      --trash                 explicitly select the default recoverable mode
      --delegate              delegate all other arguments to GNU coreutils rm
      --rmu-progress[=WHEN]   show totals, rate, ETA, move status, and summary;
                                WHEN is auto, always, or never

Environment:
  RMU_MODE                   default operation mode: trash or delegate;
                              an explicit mode option takes precedence
  RMU_GNU_RM                 GNU rm executable used by delegate mode and tests
  RMU_PROGRESS               default progress mode: auto, always, or never;
                              the command-line option takes precedence
  POSIXLY_CORRECT            stop option parsing at the first operand, as GNU
                              rm does when this variable is present

The default is Trash mode. No Trash failure ever falls back to permanent
deletion; delegation to GNU rm must be explicitly selected.
";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptMode {
    Default,
    Never,
    Once,
    Always,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreserveRoot {
    Root,
    All,
    Disabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressMode {
    Never,
    Auto,
    Always,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Invocation {
    pub operands: Vec<OsString>,
    pub recursive: bool,
    pub dir: bool,
    pub force: bool,
    pub prompt: PromptMode,
    pub one_file_system: bool,
    pub preserve_root: PreserveRoot,
    pub verbose: bool,
    pub progress: ProgressMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseOutcome {
    Run(Invocation),
    Help,
    Version,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Invocation {
    #[cfg(test)]
    pub fn parse<I>(arguments: I) -> Result<ParseOutcome, ParseError>
    where
        I: IntoIterator<Item = OsString>,
    {
        Self::parse_with_default_progress(arguments, ProgressMode::Never)
    }

    #[cfg(test)]
    pub fn parse_with_default_progress<I>(
        arguments: I,
        default_progress: ProgressMode,
    ) -> Result<ParseOutcome, ParseError>
    where
        I: IntoIterator<Item = OsString>,
    {
        Self::parse_with_order(arguments, default_progress, false)
    }

    fn parse_with_order<I>(
        arguments: I,
        default_progress: ProgressMode,
        stop_options_at_first_operand: bool,
    ) -> Result<ParseOutcome, ParseError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut invocation = Self {
            operands: Vec::new(),
            recursive: false,
            dir: false,
            force: false,
            prompt: PromptMode::Default,
            one_file_system: false,
            preserve_root: PreserveRoot::Root,
            verbose: false,
            progress: default_progress,
        };
        let mut options = true;

        for argument in arguments {
            let bytes = argument.as_bytes();
            if options && bytes == b"--" {
                options = false;
                continue;
            }
            if options && bytes.starts_with(b"--") && bytes.len() > 2 {
                match parse_long(bytes, &mut invocation)? {
                    Some(outcome) => return Ok(outcome),
                    None => continue,
                }
            }
            if options && bytes.starts_with(b"-") && bytes.len() > 1 {
                parse_short(bytes, &mut invocation)?;
                continue;
            }
            invocation.operands.push(argument);
            if stop_options_at_first_operand {
                options = false;
            }
        }

        if invocation.operands.is_empty() && !invocation.force {
            return Err(ParseError("missing operand".into()));
        }
        Ok(ParseOutcome::Run(invocation))
    }

    #[cfg(test)]
    pub fn parse_with_environment_progress<I>(
        arguments: I,
        environment_value: Option<&OsStr>,
    ) -> Result<ParseOutcome, ParseError>
    where
        I: IntoIterator<Item = OsString>,
    {
        Self::parse_with_environment(arguments, environment_value, false)
    }

    pub fn parse_with_environment<I>(
        arguments: I,
        progress_environment: Option<&OsStr>,
        posixly_correct: bool,
    ) -> Result<ParseOutcome, ParseError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let arguments: Vec<OsString> = arguments.into_iter().collect();
        let command_line_progress = has_command_line_progress(&arguments, posixly_correct);
        let parsed = Self::parse_with_order(arguments, ProgressMode::Never, posixly_correct)?;
        let ParseOutcome::Run(mut invocation) = parsed else {
            return Ok(parsed);
        };
        if !command_line_progress {
            invocation.progress = progress_mode_from_environment(progress_environment)?;
        }
        Ok(ParseOutcome::Run(invocation))
    }
}

pub fn progress_mode_from_environment(value: Option<&OsStr>) -> Result<ProgressMode, ParseError> {
    match value.map(OsStrExt::as_bytes) {
        Some(b"auto") => Ok(ProgressMode::Auto),
        Some(b"always") => Ok(ProgressMode::Always),
        None | Some(b"never") => Ok(ProgressMode::Never),
        Some(other) => Err(ParseError(format!(
            "invalid value '{}' for RMU_PROGRESS; expected auto, always, or never",
            display_bytes(other)
        ))),
    }
}

fn has_command_line_progress(arguments: &[OsString], stop_options_at_first_operand: bool) -> bool {
    for argument in arguments {
        let bytes = argument.as_bytes();
        if bytes == b"--" {
            return false;
        }
        if bytes == b"--rmu-progress" || bytes.starts_with(b"--rmu-progress=") {
            return true;
        }
        if stop_options_at_first_operand && !(bytes.starts_with(b"-") && bytes.len() > 1) {
            return false;
        }
    }
    false
}

fn parse_long(
    bytes: &[u8],
    invocation: &mut Invocation,
) -> Result<Option<ParseOutcome>, ParseError> {
    let long = &bytes[2..];
    let (name, value) = match long.iter().position(|byte| *byte == b'=') {
        Some(index) => (&long[..index], Some(&long[index + 1..])),
        None => (long, None),
    };
    if name == b"rmu-progress" {
        invocation.progress = match value {
            None | Some(b"auto") => ProgressMode::Auto,
            Some(b"always") => ProgressMode::Always,
            Some(b"never") => ProgressMode::Never,
            Some(other) => {
                return Err(ParseError(format!(
                    "invalid argument '{}' for '--rmu-progress'",
                    display_bytes(other)
                )));
            }
        };
        return Ok(None);
    }
    let name = resolve_long_name(name, bytes)?;
    match name {
        b"force" if value.is_none() => apply_force(invocation),
        b"interactive" => apply_interactive(invocation, value)?,
        b"dir" if value.is_none() => invocation.dir = true,
        b"recursive" if value.is_none() => invocation.recursive = true,
        b"one-file-system" if value.is_none() => invocation.one_file_system = true,
        b"no-preserve-root" if value.is_none() => invocation.preserve_root = PreserveRoot::Disabled,
        b"preserve-root" => {
            invocation.preserve_root = match value {
                None => PreserveRoot::Root,
                Some(b"all") => PreserveRoot::All,
                Some(other) => {
                    return Err(ParseError(format!(
                        "invalid argument '{}' for '--preserve-root'",
                        display_bytes(other)
                    )));
                }
            };
        }
        b"verbose" if value.is_none() => invocation.verbose = true,
        b"help" if value.is_none() => return Ok(Some(ParseOutcome::Help)),
        b"version" if value.is_none() => return Ok(Some(ParseOutcome::Version)),
        _ => {
            return Err(ParseError(format!(
                "unrecognized option '{}'",
                display_bytes(bytes)
            )));
        }
    }
    Ok(None)
}

fn resolve_long_name(name: &[u8], original: &[u8]) -> Result<&'static [u8], ParseError> {
    #[derive(Clone, Copy)]
    enum Abbreviation {
        UniquePrefix,
        ExactOnly,
    }

    struct LongOption {
        name: &'static [u8],
        abbreviation: Abbreviation,
    }

    const LONG_OPTIONS: [LongOption; 10] = [
        LongOption {
            name: b"force",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"interactive",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"dir",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"recursive",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"one-file-system",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"no-preserve-root",
            abbreviation: Abbreviation::ExactOnly,
        },
        LongOption {
            name: b"preserve-root",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"verbose",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"help",
            abbreviation: Abbreviation::UniquePrefix,
        },
        LongOption {
            name: b"version",
            abbreviation: Abbreviation::UniquePrefix,
        },
    ];
    if let Some(exact) = LONG_OPTIONS.iter().find(|option| option.name == name) {
        return Ok(exact.name);
    }
    let mut matches = LONG_OPTIONS
        .iter()
        .filter(|option| matches!(option.abbreviation, Abbreviation::UniquePrefix))
        .map(|option| option.name)
        .filter(|option| option.starts_with(name));
    let Some(first) = matches.next() else {
        return Err(ParseError(format!(
            "unrecognized option '{}'",
            display_bytes(original)
        )));
    };
    if matches.next().is_some() {
        return Err(ParseError(format!(
            "option '{}' is ambiguous",
            display_bytes(original)
        )));
    }
    Ok(first)
}

fn parse_short(bytes: &[u8], invocation: &mut Invocation) -> Result<(), ParseError> {
    for option in &bytes[1..] {
        match option {
            b'f' => apply_force(invocation),
            b'i' => apply_explicit_prompt(invocation, PromptMode::Always),
            b'I' => apply_explicit_prompt(invocation, PromptMode::Once),
            b'd' => invocation.dir = true,
            b'r' | b'R' => invocation.recursive = true,
            b'v' => invocation.verbose = true,
            unknown => {
                return Err(ParseError(format!(
                    "invalid option -- '{}'",
                    char::from(*unknown).escape_default()
                )));
            }
        }
    }
    Ok(())
}

fn apply_force(invocation: &mut Invocation) {
    invocation.force = true;
    invocation.prompt = PromptMode::Never;
}

fn apply_explicit_prompt(invocation: &mut Invocation, prompt: PromptMode) {
    invocation.force = false;
    invocation.prompt = prompt;
}

fn apply_interactive(invocation: &mut Invocation, value: Option<&[u8]>) -> Result<(), ParseError> {
    match value {
        None
        | Some(b"always" | b"a" | b"al" | b"alw" | b"alwa" | b"alway" | b"yes" | b"y" | b"ye") => {
            apply_explicit_prompt(invocation, PromptMode::Always);
        }
        Some(b"once" | b"o" | b"on" | b"onc") => {
            apply_explicit_prompt(invocation, PromptMode::Once);
        }
        Some(b"never" | b"n" | b"ne" | b"nev" | b"neve" | b"no" | b"non" | b"none") => {
            invocation.prompt = PromptMode::Never;
        }
        Some(other) => {
            return Err(ParseError(format!(
                "invalid argument '{}' for '--interactive'",
                display_bytes(other)
            )));
        }
    }
    Ok(())
}

fn display_bytes(bytes: &[u8]) -> String {
    escape_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStringExt;

    fn run(arguments: &[&str]) -> Invocation {
        match Invocation::parse(arguments.iter().map(OsString::from)).unwrap() {
            ParseOutcome::Run(invocation) => invocation,
            other => panic!("expected invocation, got {other:?}"),
        }
    }

    #[test]
    fn accepts_grouped_gnu_options() {
        let invocation = run(&["-rfv", "folder"]);
        assert!(invocation.recursive);
        assert!(invocation.force);
        assert!(invocation.verbose);
        assert_eq!(invocation.prompt, PromptMode::Never);
    }

    #[test]
    fn option_order_controls_force_and_prompt() {
        assert_eq!(run(&["-fi", "x"]).prompt, PromptMode::Always);
        assert!(!run(&["-fi", "x"]).force);
        assert_eq!(run(&["-if", "x"]).prompt, PromptMode::Never);
        assert!(run(&["-if", "x"]).force);
    }

    #[test]
    fn accepts_all_gnu_interactive_value_aliases() {
        assert_eq!(run(&["--interactive=yes", "x"]).prompt, PromptMode::Always);
        assert_eq!(run(&["--interactive=y", "x"]).prompt, PromptMode::Always);
        assert_eq!(run(&["--interactive=no", "x"]).prompt, PromptMode::Never);
        assert_eq!(run(&["--interactive=none", "x"]).prompt, PromptMode::Never);
    }

    #[test]
    fn double_dash_allows_hyphenated_operand() {
        let invocation = run(&["--", "-rf"]);
        assert_eq!(invocation.operands, [OsString::from("-rf")]);
        assert!(!invocation.recursive);
    }

    #[test]
    fn force_allows_no_operands() {
        assert!(matches!(
            Invocation::parse([OsString::from("-f")]),
            Ok(ParseOutcome::Run(_))
        ));
    }

    #[test]
    fn accepts_unambiguous_gnu_long_option_abbreviations() {
        let invocation = run(&["--rec", "--inter=o", "folder"]);
        assert!(invocation.recursive);
        assert_eq!(invocation.prompt, PromptMode::Once);
        assert!(Invocation::parse([OsString::from("--ver")]).is_err());
    }

    #[test]
    fn no_preserve_root_requires_its_exact_gnu_spelling() {
        let name = "no-preserve-root";
        for length in 1..name.len() {
            let option = format!("--{}", &name[..length]);
            assert!(Invocation::parse([OsString::from(option)]).is_err());
        }
        let invocation = run(&["--no-preserve-root", "folder"]);
        assert_eq!(invocation.preserve_root, PreserveRoot::Disabled);
    }

    #[test]
    fn rejects_empty_optional_values_like_gnu_rm() {
        assert!(Invocation::parse([OsString::from("--interactive=")]).is_err());
        assert!(Invocation::parse([OsString::from("--preserve-root=")]).is_err());
    }

    #[test]
    fn progress_is_namespaced_and_off_by_default() {
        assert_eq!(run(&["file"]).progress, ProgressMode::Never);
        assert_eq!(
            run(&["--rmu-progress", "file"]).progress,
            ProgressMode::Auto
        );
        assert_eq!(
            run(&["--rmu-progress=always", "file"]).progress,
            ProgressMode::Always
        );
        assert!(Invocation::parse([OsString::from("--rmu-progress=")]).is_err());
    }

    #[test]
    fn posix_order_stops_option_parsing_at_the_first_operand() {
        let parsed = Invocation::parse_with_environment(
            [
                OsString::from("file"),
                OsString::from("-v"),
                OsString::from("--rmu-progress=always"),
            ],
            None,
            true,
        )
        .unwrap();
        let ParseOutcome::Run(invocation) = parsed else {
            panic!("expected runnable invocation");
        };
        assert_eq!(invocation.operands, ["file", "-v", "--rmu-progress=always"]);
        assert!(!invocation.verbose);
        assert_eq!(invocation.progress, ProgressMode::Never);
    }

    #[test]
    fn environment_progress_is_strict_and_cli_takes_precedence() {
        assert_eq!(
            progress_mode_from_environment(None),
            Ok(ProgressMode::Never)
        );
        assert_eq!(
            progress_mode_from_environment(Some(OsStr::new("always"))),
            Ok(ProgressMode::Always)
        );
        assert!(progress_mode_from_environment(Some(OsStr::new(""))).is_err());
        assert!(progress_mode_from_environment(Some(OsStr::new("yes"))).is_err());

        let parsed = Invocation::parse_with_environment_progress(
            [
                OsString::from("--rmu-progress=never"),
                OsString::from("file"),
            ],
            Some(OsStr::new("invalid")),
        )
        .unwrap();
        let ParseOutcome::Run(invocation) = parsed else {
            panic!("expected a runnable invocation");
        };
        assert_eq!(invocation.progress, ProgressMode::Never);

        assert!(matches!(
            Invocation::parse_with_environment_progress(
                [OsString::from("--help")],
                Some(OsStr::new("invalid"))
            ),
            Ok(ParseOutcome::Help)
        ));
        assert!(
            Invocation::parse_with_environment_progress(
                [OsString::from("file")],
                Some(OsStr::new("invalid"))
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_values_are_terminal_safe() {
        let error = progress_mode_from_environment(Some(OsStr::from_bytes(b"bad\n\x1b\xff")))
            .unwrap_err()
            .to_string();
        assert!(error.contains("bad\\n\\x1B\\xFF"));
        assert!(!error.contains('\n'));

        let error = Invocation::parse([OsString::from_vec(b"--interactive=bad\r\x1b".to_vec())])
            .unwrap_err()
            .to_string();
        assert!(error.contains("bad\\r\\x1B"));
        assert!(!error.contains('\r'));
    }
}
