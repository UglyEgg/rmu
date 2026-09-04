use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, PathBuf};

use super::{MAX_INFO_BYTES, TrashRoot};
use crate::freedesktop::{PinnedDirectory, SourceIdentity, open_file_at};
use crate::linux::{O_CLOEXEC, O_NOFOLLOW, O_NONBLOCK, local_epoch};

pub(super) struct ParsedInfo {
    pub(super) original: Option<PathBuf>,
    pub(super) restore_name: OsString,
    pub(super) ambiguous_relative: bool,
    pub(super) deletion_date: String,
    pub(super) deleted_epoch: Option<u64>,
}

pub(super) struct InfoReadError {
    pub(super) message: String,
    pub(super) operational: bool,
}

impl InfoReadError {
    fn io(message: String) -> Self {
        Self {
            message,
            operational: true,
        }
    }

    fn malformed(message: String) -> Self {
        Self {
            message,
            operational: false,
        }
    }
}

pub(super) fn read_info(
    directory: &PinnedDirectory,
    name: &OsStr,
    expected: SourceIdentity,
    root: &TrashRoot,
) -> Result<ParsedInfo, InfoReadError> {
    let mut file = open_file_at(directory, name, O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK, 0)
        .map_err(|error| InfoReadError::io(format!("cannot open metadata: {error}")))?;
    let metadata = file
        .metadata()
        .map_err(|error| InfoReadError::io(format!("cannot inspect opened metadata: {error}")))?;
    if !metadata.file_type().is_file() || !expected.matches(&metadata) {
        return Err(InfoReadError::io(
            "metadata changed while it was opened".into(),
        ));
    }
    let mut contents = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_INFO_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| InfoReadError::io(format!("cannot read metadata: {error}")))?;
    if u64::try_from(contents.len()).unwrap_or(u64::MAX) > MAX_INFO_BYTES {
        return Err(InfoReadError::malformed(
            "metadata exceeds the 1 MiB safety limit".into(),
        ));
    }
    parse_info(&contents, root)
        .map_err(|error| InfoReadError::malformed(format!("malformed metadata: {error}")))
}

pub(super) fn parse_info(contents: &[u8], root: &TrashRoot) -> Result<ParsedInfo, String> {
    let mut lines = contents.split(|byte| *byte == b'\n');
    let header = trim_cr(lines.next().unwrap_or_default());
    if header != b"[Trash Info]" {
        return Err("missing [Trash Info] header".into());
    }
    let mut path = None;
    let mut date = None;
    for line in lines {
        let line = trim_cr(line);
        if let Some(value) = line.strip_prefix(b"Path=") {
            if path.is_none() {
                path = Some(percent_decode(value)?);
            }
        } else if let Some(value) = line.strip_prefix(b"DeletionDate=") {
            if date.is_none() {
                date = Some(
                    std::str::from_utf8(value)
                        .map_err(|_| "DeletionDate is not UTF-8")?
                        .to_owned(),
                );
            }
        }
    }
    let path = PathBuf::from(OsString::from_vec(
        path.ok_or_else(|| "missing Path field".to_string())?,
    ));
    if path.as_os_str().is_empty() || path.file_name().is_none() {
        return Err("Path has no restorable file name".into());
    }
    let restore_name = path
        .file_name()
        .expect("the restorable file name was validated")
        .to_os_string();
    let (original, ambiguous_relative) = if path.is_absolute() {
        if !root.is_home {
            return Err("absolute Path is not valid in a per-volume Trash".into());
        }
        (Some(path), false)
    } else {
        if path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err("relative Path escapes its filesystem".into());
        }
        if root.info_bases.len() == 1 {
            (Some(root.info_bases[0].join(path)), false)
        } else {
            (None, true)
        }
    };
    let deletion_date = date.ok_or_else(|| "missing DeletionDate field".to_string())?;
    let deleted_epoch = Some(
        parse_deletion_date_local(&deletion_date)
            .ok_or_else(|| "DeletionDate is not YYYY-MM-DDThh:mm:ss".to_string())?,
    );
    Ok(ParsedInfo {
        original,
        restore_name,
        ambiguous_relative,
        deletion_date,
        deleted_epoch,
    })
}

pub(super) fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

pub(super) fn percent_decode(value: &[u8]) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] == b'%' {
            let high = value.get(index + 1).and_then(|value| hex(*value));
            let low = value.get(index + 2).and_then(|value| hex(*value));
            let decoded = high
                .zip(low)
                .map(|(high, low)| (high << 4) | low)
                .ok_or_else(|| "invalid percent escape in Path".to_string())?;
            if decoded == 0 {
                return Err("Path contains NUL".into());
            }
            output.push(decoded);
            index += 3;
        } else {
            if value[index] == 0 {
                return Err("Path contains NUL".into());
            }
            output.push(value[index]);
            index += 1;
        }
    }
    Ok(output)
}

const fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

pub(super) fn parse_deletion_date_local(value: &str) -> Option<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let year = decimal(&bytes[0..4])?;
    let month = decimal(&bytes[5..7])?;
    let day = decimal(&bytes[8..10])?;
    let hour = decimal(&bytes[11..13])?;
    let minute = decimal(&bytes[14..16])?;
    let second = decimal(&bytes[17..19])?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
        || day > days_in_month(year, month)?
    {
        return None;
    }
    local_epoch(year, month, day, hour, minute, second)
}

fn decimal(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0_u32, |value, byte| {
        byte.is_ascii_digit()
            .then_some(value * 10 + u32::from(*byte - b'0'))
    })
}

fn days_in_month(year: u32, month: u32) -> Option<u32> {
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => Some(31),
        4 | 6 | 9 | 11 => Some(30),
        2 if leap => Some(29),
        2 => Some(28),
        _ => None,
    }
}

pub(super) fn stable_id(
    root: &TrashRoot,
    name: &OsStr,
    payload: Option<&fs::Metadata>,
    info: Option<&fs::Metadata>,
) -> String {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&root.device.to_le_bytes());
    bytes.extend_from_slice(&root.inode.to_le_bytes());
    bytes.push(u8::from(root.is_home));
    bytes.push(u8::from(!root.volume_mount_points.is_empty()));
    bytes.push(0);
    bytes.extend_from_slice(name.as_bytes());
    extend_incarnation(&mut bytes, b'P', payload);
    extend_incarnation(&mut bytes, b'I', info);
    let first = fnv1a(&bytes, 0xcbf2_9ce4_8422_2325);
    let second = fnv1a(&bytes, 0x8422_2325_cbf2_9ce4);
    format!("{first:016x}{second:016x}")
}

fn extend_incarnation(output: &mut Vec<u8>, marker: u8, metadata: Option<&fs::Metadata>) {
    output.push(marker);
    let Some(metadata) = metadata else {
        output.push(0);
        return;
    };
    output.push(1);
    output.extend_from_slice(&metadata.dev().to_le_bytes());
    output.extend_from_slice(&metadata.ino().to_le_bytes());
    output.extend_from_slice(&(metadata.mode() & 0o170_000).to_le_bytes());
    // Renaming an entry changes ctime, including during RMU's own reversible
    // capture transaction. Size and mtime distinguish ordinary incarnations
    // without changing merely because an entry was moved and rolled back.
    output.extend_from_slice(&metadata.size().to_le_bytes());
    output.extend_from_slice(&metadata.mtime().to_le_bytes());
    output.extend_from_slice(&metadata.mtime_nsec().to_le_bytes());
}

fn fnv1a(bytes: &[u8], seed: u64) -> u64 {
    bytes.iter().fold(seed, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
    })
}
