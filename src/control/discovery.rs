use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::metadata::{read_info, stable_id};
use super::{DiscoveryReport, EntryIdentity, EntryState, INFO_SUFFIX, TrashEntry, TrashRoot};
use crate::ctl_args::SortField;
use crate::display::quote;
use crate::freedesktop::{
    PinnedDirectory, SourceIdentity, info_name, mounted_paths, trusted_shared_trash_metadata,
};
use crate::linux::{AccessMode, effective_access};

pub(super) enum FilesystemSelector {
    Home,
    Volume(PathBuf),
}

pub(super) fn filesystem_selector_for_mount(
    home_mount: &Path,
    selected_mount: PathBuf,
) -> FilesystemSelector {
    if home_mount == selected_mount {
        FilesystemSelector::Home
    } else {
        FilesystemSelector::Volume(selected_mount)
    }
}

impl FilesystemSelector {
    pub(super) fn matches(&self, root: &TrashRoot) -> bool {
        match self {
            Self::Home => root.is_home,
            Self::Volume(mount) => root.volume_mount_points.contains(mount),
        }
    }
}

pub(super) fn data_home_from_environment() -> Result<PathBuf, String> {
    if let Some(value) = env::var_os("XDG_DATA_HOME") {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    let home = env::var_os("HOME").ok_or_else(|| "HOME is not set".to_string())?;
    let home = PathBuf::from(home);
    if !home.is_absolute() {
        return Err("HOME is not an absolute path".into());
    }
    Ok(home.join(".local/share"))
}

pub(super) fn mount_for(path: &Path, mounts: &[PathBuf]) -> PathBuf {
    mounts
        .iter()
        .filter(|mount| path.starts_with(mount))
        .max_by_key(|mount| mount.as_os_str().as_bytes().len())
        .cloned()
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub(super) fn refuse_mounted_payload(path: &Path) -> Result<(), String> {
    let mounts = mounted_paths().map_err(|error| {
        format!("cannot verify mount boundaries before permanent deletion: {error}")
    })?;
    if mounts
        .iter()
        .any(|mount| mount == path || mount.starts_with(path))
    {
        Err(format!(
            "refusing permanent deletion because {} contains a mounted filesystem boundary",
            quote(path.as_os_str())
        ))
    } else {
        Ok(())
    }
}

pub(super) enum SharedTrashState {
    Missing,
    Unknown(String),
    Usable,
    Unsafe(String),
}

pub(super) enum MountInspection {
    Absent,
    OutsideUserScope,
    Present,
    Unknown(String),
}

pub(super) fn inspect_mount(
    path: &Path,
    inspection: io::Result<fs::Metadata>,
    uid: u32,
) -> MountInspection {
    match inspection {
        Ok(metadata) if metadata.file_type().is_dir() => {
            classify_mount_access(metadata.uid(), uid, has_search_access(path))
        }
        Ok(_) => MountInspection::Absent,
        Err(error) if error.kind() == io::ErrorKind::NotFound => MountInspection::Absent,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            let user_owned_parent = path
                .parent()
                .and_then(|parent| fs::metadata(parent).ok())
                .is_some_and(|metadata| metadata.uid() == uid);
            if user_owned_parent {
                MountInspection::Unknown(error.to_string())
            } else {
                MountInspection::OutsideUserScope
            }
        }
        Err(error) => MountInspection::Unknown(error.to_string()),
    }
}

pub(super) fn classify_mount_access(
    owner: u32,
    uid: u32,
    access: io::Result<bool>,
) -> MountInspection {
    match access {
        Ok(true) => MountInspection::Present,
        Ok(false) if owner == uid => {
            MountInspection::Unknown("search access is denied on a user-owned mount".into())
        }
        Ok(false) => MountInspection::OutsideUserScope,
        Err(error) => MountInspection::Unknown(error.to_string()),
    }
}

pub(super) fn has_search_access(path: &Path) -> io::Result<bool> {
    effective_access(path, AccessMode::Search)
}

pub(super) fn shared_trash_state(path: &Path, uid: u32) -> SharedTrashState {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => SharedTrashState::Missing,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            SharedTrashState::Unknown(error.to_string())
        }
        Err(error) => SharedTrashState::Unknown(error.to_string()),
        Ok(metadata) if trusted_shared_trash_metadata(&metadata, uid) => SharedTrashState::Usable,
        Ok(metadata) if !metadata.file_type().is_dir() => {
            SharedTrashState::Unsafe("not a real directory".into())
        }
        Ok(metadata) if metadata.permissions().mode() & 0o1000 == 0 => {
            SharedTrashState::Unsafe("sticky bit is not set".into())
        }
        Ok(_) => SharedTrashState::Unsafe("not owned by root or the current user".into()),
    }
}

pub(super) fn trash_candidate_exists(
    path: &Path,
    inspection: io::Result<()>,
    discovery: &mut DiscoveryReport,
) -> bool {
    match inspection {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            discovery.unknown(format!(
                "cannot inspect Trash candidate {}: {error}",
                quote(path.as_os_str())
            ));
            false
        }
    }
}

pub(super) fn open_optional_child(
    root: &PinnedDirectory,
    name: &OsStr,
    path: &Path,
    uid: u32,
    warnings: &mut Vec<String>,
) -> Option<PinnedDirectory> {
    match root.open_private_child(name, path, uid) {
        Ok(directory) => Some(directory),
        Err(_error)
            if fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            warnings.push(format!(
                "required Trash directory {} is missing",
                quote(path.as_os_str())
            ));
            None
        }
        Err(error) => {
            warnings.push(format!(
                "cannot inspect {}: {error}",
                quote(path.as_os_str())
            ));
            None
        }
    }
}

pub(super) fn pinned_directory_path(directory: &PinnedDirectory) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}", directory.raw_fd()))
}

pub(super) fn collect_payload_names(
    directory: &PinnedDirectory,
    logical_path: &Path,
    names: &mut BTreeSet<OsString>,
    warnings: &mut Vec<String>,
) -> bool {
    let entries = match fs::read_dir(pinned_directory_path(directory)) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!(
                "cannot enumerate {}: {error}",
                quote(logical_path.as_os_str())
            ));
            return false;
        }
    };
    let mut complete = true;
    for entry in entries {
        match entry {
            Ok(entry) => {
                let name = entry.file_name();
                names.insert(name);
            }
            Err(error) => {
                complete = false;
                warnings.push(format!("cannot read payload entry: {error}"));
            }
        }
    }
    complete
}

pub(super) fn collect_info_names(
    directory: &PinnedDirectory,
    logical_path: &Path,
    names: &mut BTreeSet<OsString>,
    warnings: &mut Vec<String>,
) -> bool {
    let entries = match fs::read_dir(pinned_directory_path(directory)) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!(
                "cannot enumerate {}: {error}",
                quote(logical_path.as_os_str())
            ));
            return false;
        }
    };
    let mut complete = true;
    for entry in entries {
        match entry {
            Ok(entry) => {
                let name = entry.file_name();
                let displayed_path = logical_path.join(&name);
                let bytes = name.into_vec();
                if let Some(base) = bytes.strip_suffix(INFO_SUFFIX) {
                    names.insert(OsString::from_vec(base.to_vec()));
                } else if is_internal_quarantine_name(OsStr::from_bytes(&bytes)) {
                    warnings.push(format!(
                        "recoverable quarantine metadata found at {}",
                        quote(displayed_path.as_os_str())
                    ));
                } else {
                    complete = false;
                    warnings.push(format!(
                        "unexpected file in Trash metadata directory: {}",
                        quote(displayed_path.as_os_str())
                    ));
                }
            }
            Err(error) => {
                complete = false;
                warnings.push(format!("cannot read metadata entry: {error}"));
            }
        }
    }
    complete
}

pub(super) fn collect_root_quarantines(
    directory: &PinnedDirectory,
    logical_path: &Path,
    warnings: &mut Vec<String>,
) -> bool {
    let entries = match fs::read_dir(pinned_directory_path(directory)) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!(
                "cannot enumerate Trash root {}: {error}",
                quote(logical_path.as_os_str())
            ));
            return false;
        }
    };
    let mut complete = true;
    for entry in entries {
        match entry {
            Ok(entry) if is_internal_root_temporary(&entry.file_name()) => {
                let displayed_path = logical_path.join(entry.file_name());
                warnings.push(format!(
                    "recoverable temporary cache file found at {}",
                    quote(displayed_path.as_os_str())
                ));
            }
            Ok(entry)
                if matches!(
                    entry.file_name().as_bytes(),
                    b"files" | b"info" | b"directorysizes"
                ) => {}
            Ok(entry) => {
                complete = false;
                let displayed_path = logical_path.join(entry.file_name());
                warnings.push(format!(
                    "unexpected entry in Trash root: {}",
                    quote(displayed_path.as_os_str())
                ));
            }
            Err(error) => {
                complete = false;
                warnings.push(format!("cannot read Trash root entry: {error}"));
            }
        }
    }
    complete
}

#[allow(clippy::too_many_lines)]
pub(super) fn inspect_entry(
    root_index: usize,
    root: &TrashRoot,
    name: &OsStr,
    files_directory: Option<&PinnedDirectory>,
    info_directory: Option<&PinnedDirectory>,
    cache: &HashMap<Vec<u8>, (u64, i64)>,
) -> (TrashEntry, bool) {
    let payload = root.path.join("files").join(name);
    let info_name = info_name(name);
    let info = root.path.join("info").join(&info_name);
    let mut trusted = true;
    let mut problems = Vec::new();
    let payload_metadata = files_directory.and_then(|directory| match directory.metadata(name) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            trusted = false;
            problems.push(format!("cannot inspect payload: {error}"));
            None
        }
    });
    let info_metadata = info_directory.and_then(|directory| match directory.metadata(&info_name) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            trusted = false;
            problems.push(format!("cannot inspect metadata: {error}"));
            None
        }
    });
    let payload_identity = payload_metadata
        .as_ref()
        .map(|metadata| EntryIdentity::from_metadata(metadata, info_metadata.is_none()));
    let info_identity = info_metadata
        .as_ref()
        .map(|metadata| EntryIdentity::from_metadata(metadata, true));
    let parsed = match (info_directory, info_metadata.as_ref()) {
        (Some(directory), Some(metadata)) if metadata.file_type().is_file() => read_info(
            directory,
            &info_name,
            SourceIdentity::from_metadata(metadata),
            root,
        )
        .map_err(|error| {
            if error.operational {
                trusted = false;
            }
            problems.push(error.message);
        })
        .ok(),
        (_, Some(_)) => {
            problems.push("metadata is not a regular file".into());
            None
        }
        _ => None,
    };
    if parsed
        .as_ref()
        .is_some_and(|value| value.ambiguous_relative)
    {
        problems
            .push("relative Path is ambiguous across mount aliases; restore requires --to".into());
    }
    if payload_metadata.is_none() {
        problems.push("payload is missing".into());
    }
    if info_metadata.is_none() {
        problems.push("metadata is missing".into());
    }
    let state = match (
        payload_metadata.is_some(),
        info_metadata.is_some(),
        parsed.is_some(),
    ) {
        (true, true, true) => EntryState::Healthy,
        (true, false, _) => EntryState::PayloadOnly,
        (false, true, true) => EntryState::InfoOnly,
        _ => EntryState::Malformed,
    };
    // DeletionDate is the user-facing deletion time used for sorting and age
    // filters. The metadata mtime is kept separately for directorysizes.
    let deleted_epoch = parsed.as_ref().and_then(|value| value.deleted_epoch);
    let size = payload_metadata.as_ref().and_then(|metadata| {
        if metadata.file_type().is_dir() {
            cache
                .get(name.as_bytes())
                .filter(|(_, cache_mtime)| {
                    info_metadata
                        .as_ref()
                        .is_some_and(|info| *cache_mtime == info.mtime())
                })
                .map(|(size, _)| *size)
        } else {
            Some(metadata.blocks().saturating_mul(512))
        }
    });
    let entry = TrashEntry {
        id: stable_id(
            root,
            name,
            payload_metadata.as_ref(),
            info_metadata.as_ref(),
        ),
        name: name.to_os_string(),
        payload,
        info,
        original_path: parsed.as_ref().and_then(|value| value.original.clone()),
        deletion_date: parsed.as_ref().map(|value| value.deletion_date.clone()),
        deleted_epoch,
        size,
        state,
        problems,
        info_mtime: info_metadata.as_ref().map(fs::Metadata::mtime),
        root_index,
        payload_identity,
        info_identity,
        restore_name: parsed.as_ref().map(|value| value.restore_name.clone()),
    };
    (entry, trusted)
}

pub(super) fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

pub(super) fn age_matches(
    entry: &TrashEntry,
    now: u64,
    older_than: Option<u64>,
    newer_than: Option<u64>,
) -> bool {
    let Some(deleted) = entry.deleted_epoch else {
        return older_than.is_none() && newer_than.is_none();
    };
    let age = now.saturating_sub(deleted);
    older_than.is_none_or(|limit| age >= limit) && newer_than.is_none_or(|limit| age <= limit)
}

pub(super) fn compare_entries(left: &TrashEntry, right: &TrashEntry, sort: SortField) -> Ordering {
    let ordering = match sort {
        SortField::Name => left.name.cmp(&right.name),
        SortField::Date => left.deleted_epoch.cmp(&right.deleted_epoch),
        SortField::Size => left.size.cmp(&right.size),
        SortField::Path => left.original_path.cmp(&right.original_path),
    };
    ordering.then_with(|| left.id.cmp(&right.id))
}

pub(super) fn canonical_existing_ancestor(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot inspect current directory: {error}"))?
            .join(path)
    };
    let mut candidate = absolute.as_path();
    loop {
        match candidate.canonicalize() {
            Ok(path) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = candidate.parent().ok_or_else(|| {
                    format!(
                        "cannot find an existing ancestor of {}",
                        quote(absolute.as_os_str())
                    )
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "cannot resolve {}: {error}",
                    quote(candidate.as_os_str())
                ));
            }
        }
    }
}

pub(super) fn resolve_prospective_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .map_err(|error| format!("cannot inspect current directory: {error}"))?
            .join(path)
    };
    let mut candidate = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match candidate.canonicalize() {
            Ok(mut resolved) => {
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let name = candidate.file_name().ok_or_else(|| {
                    format!(
                        "cannot find an existing ancestor of {}",
                        quote(absolute.as_os_str())
                    )
                })?;
                missing.push(name.to_os_string());
                candidate = candidate.parent().ok_or_else(|| {
                    format!(
                        "cannot find an existing ancestor of {}",
                        quote(absolute.as_os_str())
                    )
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "cannot resolve {}: {error}",
                    quote(candidate.as_os_str())
                ));
            }
        }
    }
}

pub(super) fn is_internal_quarantine_name(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    let Some(suffix) = bytes
        .strip_prefix(b".rmu-quarantine-")
        .or_else(|| bytes.strip_prefix(b".rtrash-quarantine-"))
    else {
        return false;
    };
    let Some(separator) = suffix.iter().position(|byte| *byte == b'-') else {
        return false;
    };
    !suffix[..separator].is_empty()
        && !suffix[separator + 1..].is_empty()
        && suffix[..separator].iter().all(u8::is_ascii_digit)
        && suffix[separator + 1..].iter().all(u8::is_ascii_digit)
}

pub(super) fn is_internal_root_temporary(name: &OsStr) -> bool {
    is_internal_quarantine_name(name)
        || [
            b".directorysizes.rmu-".as_slice(),
            b".directorysizes.rtrash-".as_slice(),
        ]
        .iter()
        .find_map(|prefix| name.as_bytes().strip_prefix(*prefix))
        .and_then(|suffix| suffix.strip_suffix(b".tmp"))
        .and_then(|suffix| {
            let separator = suffix.iter().position(|byte| *byte == b'-')?;
            Some((&suffix[..separator], &suffix[separator + 1..]))
        })
        .is_some_and(|(pid, sequence)| {
            !pid.is_empty()
                && !sequence.is_empty()
                && pid.iter().all(u8::is_ascii_digit)
                && sequence.iter().all(u8::is_ascii_digit)
        })
}
