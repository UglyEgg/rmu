use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use super::metadata::stable_id;
use super::{
    ActionResult, CaptureError, CapturedEntry, EntryIdentity, EntryRecovery, EntryState,
    RecoveryDestinations, RecoveryState, TrashEntry, TrashRoot,
};
use crate::ctl_args::ConflictPolicy;
use crate::display::quote;
use crate::freedesktop::{
    PinnedDirectory, SourceIdentity, info_name, quarantine_recovery_state, rename_no_replace_at,
};

#[cfg(test)]
use super::{FAIL_CAPTURE_VERIFICATION_ON, FORCE_RESTORE_PUBLICATION_FAILURE};

pub(super) fn require_healthy(entry: &TrashEntry, operation: &str) -> Result<(), String> {
    if entry.state == EntryState::Healthy {
        Ok(())
    } else {
        Err(format!(
            "cannot {operation} {} because its state is {}",
            entry.id,
            entry.state.as_str()
        ))
    }
}

pub(super) fn choose_destination(
    path: &Path,
    conflict: ConflictPolicy,
    reserved: Option<&HashSet<PathBuf>>,
) -> Result<PathBuf, String> {
    let is_reserved = |candidate: &Path| reserved.is_some_and(|paths| paths.contains(candidate));
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && !is_reserved(path) => {
            Ok(path.to_path_buf())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if conflict == ConflictPolicy::Fail {
                return Err(format!(
                    "destination {} is already reserved by this operation",
                    quote(path.as_os_str())
                ));
            }
            choose_renamed_destination(path, reserved)
        }
        Err(error) => Err(format!(
            "cannot inspect {}: {error}",
            quote(path.as_os_str())
        )),
        Ok(_) if conflict == ConflictPolicy::Fail => Err(format!(
            "destination {} already exists",
            quote(path.as_os_str())
        )),
        Ok(_) => choose_renamed_destination(path, reserved),
    }
}

pub(super) fn choose_renamed_destination(
    path: &Path,
    reserved: Option<&HashSet<PathBuf>>,
) -> Result<PathBuf, String> {
    let name = path
        .file_name()
        .ok_or_else(|| "destination has no file name".to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "destination has no parent".to_string())?;
    let maximum = component_name_limit(parent);
    for suffix in 1..=10_000_u32 {
        let suffix = format!(".restored.{suffix}").into_bytes();
        let base_length = maximum.saturating_sub(suffix.len());
        let mut bytes = name.as_bytes()[..name.as_bytes().len().min(base_length)].to_vec();
        bytes.extend_from_slice(&suffix);
        let candidate = parent.join(OsString::from_vec(bytes));
        if reserved.is_some_and(|paths| paths.contains(&candidate)) {
            continue;
        }
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Err(error) => {
                return Err(format!(
                    "cannot inspect {}: {error}",
                    quote(candidate.as_os_str())
                ));
            }
            Ok(_) => {}
        }
    }
    Err("could not choose an unused restore name".into())
}

pub(super) fn revalidate(
    directory: &PinnedDirectory,
    name: &OsStr,
    expected: EntryIdentity,
    kind: &str,
) -> Result<(), String> {
    let metadata = directory
        .metadata(name)
        .map_err(|error| format!("cannot revalidate {kind}: {error}"))?;
    if expected.matches(&metadata) {
        Ok(())
    } else {
        Err(format!("refusing action because {kind} identity changed"))
    }
}

pub(super) fn capture(
    directory: &PinnedDirectory,
    name: &OsStr,
    expected: EntryIdentity,
) -> Result<OsString, CaptureError> {
    for _ in 0..1024 {
        let capture = directory.quarantine_name();
        match rename_no_replace_at(directory, name, directory, &capture) {
            Ok(()) => {
                #[cfg(test)]
                let fail_verification = FAIL_CAPTURE_VERIFICATION_ON.with(|remaining| {
                    let value = remaining.get();
                    if value == 0 {
                        false
                    } else if value == 1 {
                        remaining.set(0);
                        true
                    } else {
                        remaining.set(value - 1);
                        false
                    }
                });
                #[cfg(not(test))]
                let fail_verification = false;
                if fail_verification {
                    return Err(CaptureError::after_rename(
                        "cannot verify captured entry: injected verification failure".into(),
                        capture,
                        expected,
                    ));
                }
                let metadata = match directory.metadata(&capture) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        return Err(CaptureError::after_rename(
                            format!("cannot verify captured entry: {error}"),
                            capture,
                            expected,
                        ));
                    }
                };
                if expected.source.matches(&metadata) {
                    return Ok(capture);
                }
                return Err(CaptureError::after_rename(
                    "captured entry identity changed".into(),
                    capture,
                    expected,
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(CaptureError::before_rename(format!(
                    "cannot capture entry safely: {error}"
                )));
            }
        }
    }
    Err(CaptureError::before_rename(
        "could not reserve a quarantine name".into(),
    ))
}

pub(super) fn capture_entry_snapshot(
    entry: &TrashEntry,
    files: &PinnedDirectory,
    info: &PinnedDirectory,
) -> Result<CapturedEntry, String> {
    let info_name = info_name(&entry.name);
    let payload = if let Some(identity) = entry.payload_identity {
        revalidate(files, &entry.name, identity, "payload")?;
        match capture(files, &entry.name, identity) {
            Ok(name) => Some((name, identity)),
            Err(error) => {
                let captured = CapturedEntry {
                    payload: error.captured,
                    info: None,
                };
                let recovery = rollback_captured_entry(entry, &captured, files, info);
                return Err(format!(
                    "{}; payload recovery state: {}; metadata recovery state: {}",
                    error.message, recovery.payload, recovery.metadata
                ));
            }
        }
    } else {
        None
    };
    let metadata = if let Some(identity) = entry.info_identity {
        if let Err(error) = revalidate(info, &info_name, identity, "metadata") {
            let captured = CapturedEntry {
                payload,
                info: None,
            };
            let recovery = rollback_captured_entry(entry, &captured, files, info);
            return Err(format!(
                "{error}; payload recovery state: {}; metadata recovery state: {}",
                recovery.payload, recovery.metadata
            ));
        }
        match capture(info, &info_name, identity) {
            Ok(name) => Some((name, identity)),
            Err(error) => {
                let captured = CapturedEntry {
                    payload,
                    info: error.captured,
                };
                let recovery = rollback_captured_entry(entry, &captured, files, info);
                return Err(format!(
                    "{}; payload recovery state: {}; metadata recovery state: {}",
                    error.message, recovery.payload, recovery.metadata
                ));
            }
        }
    } else {
        None
    };

    let captured = CapturedEntry {
        payload,
        info: metadata,
    };

    let topology = require_absent(files, &entry.name, "payload")
        .and_then(|()| require_absent(info, &info_name, "metadata"));
    if let Err(error) = topology {
        let recovery = rollback_captured_entry(entry, &captured, files, info);
        return Err(format!(
            "refusing action because the Trash entry changed during capture: {error}; payload recovery state: {}; metadata recovery state: {}",
            recovery.payload, recovery.metadata
        ));
    }

    Ok(captured)
}

pub(super) fn rollback_captured_entry(
    entry: &TrashEntry,
    captured: &CapturedEntry,
    files: &PinnedDirectory,
    info: &PinnedDirectory,
) -> EntryRecovery {
    EntryRecovery {
        payload: restore_captured(files, &entry.name, captured.payload.as_ref()),
        metadata: restore_captured(info, &info_name(&entry.name), captured.info.as_ref()),
    }
}

pub(super) fn publish_restored_payload(
    source: &PinnedDirectory,
    source_name: &OsStr,
    destination: &PinnedDirectory,
    destination_name: &OsStr,
) -> io::Result<()> {
    #[cfg(test)]
    if FORCE_RESTORE_PUBLICATION_FAILURE.with(|force| force.replace(false)) {
        return Err(io::Error::from_raw_os_error(18));
    }
    rename_no_replace_at(source, source_name, destination, destination_name)
}

pub(super) fn rollback_published_restore(
    entry: &TrashEntry,
    captured: &CapturedEntry,
    files: &PinnedDirectory,
    info: &PinnedDirectory,
    destination: &PinnedDirectory,
    destination_name: &OsStr,
) -> String {
    let (capture_name, identity) = captured
        .payload
        .as_ref()
        .expect("healthy restore has a captured payload");
    let metadata_state = || {
        captured.info.as_ref().map_or_else(
            || RecoveryState::NotNeeded.to_string(),
            |(name, identity)| describe_quarantine_state(info, name, *identity),
        )
    };
    if let Err(error) = revalidate_source(
        destination,
        destination_name,
        identity.source,
        "published restore payload",
    ) {
        return format!(
            "{error}; payload recovery state: unknown; metadata recovery state: {}",
            metadata_state()
        );
    }
    if let Err(error) = rename_no_replace_at(destination, destination_name, files, capture_name) {
        return format!(
            "cannot recapture the published restore payload: {error}; payload recovery state: unknown; metadata recovery state: {}",
            metadata_state()
        );
    }
    let recovery = rollback_captured_entry(entry, captured, files, info);
    format!(
        "payload recovery state: {}; metadata recovery state: {}",
        recovery.payload, recovery.metadata
    )
}

pub(super) fn require_absent(
    directory: &PinnedDirectory,
    name: &OsStr,
    kind: &str,
) -> Result<(), String> {
    match directory.metadata(name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot verify that {kind} remains absent: {error}")),
        Ok(_) => Err(format!("a {kind} appeared after inventory")),
    }
}

pub(super) fn restore_captured(
    directory: &PinnedDirectory,
    original_name: &OsStr,
    captured: Option<&(OsString, EntryIdentity)>,
) -> RecoveryState {
    let Some((capture_name, identity)) = captured else {
        return RecoveryState::NotNeeded;
    };
    let secured = match capture(directory, capture_name, *identity) {
        Ok(name) => name,
        Err(error) => {
            return RecoveryState::Unknown(format!(
                "could not secure the captured entry for restoration: {}",
                error.describe(directory)
            ));
        }
    };
    if let Err(error) = rename_no_replace_at(directory, &secured, directory, original_name) {
        return RecoveryState::Quarantined(format!(
            "could not restore {}: {error}; recovery state: {}",
            quote(original_name),
            describe_quarantine_state(directory, &secured, *identity)
        ));
    }
    match revalidate_source(
        directory,
        original_name,
        identity.source,
        "restored component",
    ) {
        Ok(()) => RecoveryState::Restored(original_name.to_os_string()),
        Err(error) => RecoveryState::Unknown(format!(
            "{error}; the selected component location after restoration is unknown"
        )),
    }
}

pub(super) fn quarantine_recovery_status(
    directory: &PinnedDirectory,
    capture: Option<&(OsString, EntryIdentity)>,
) -> RecoveryState {
    if let Some((capture, identity)) = capture {
        RecoveryState::Quarantined(describe_quarantine_state(directory, capture, *identity))
    } else {
        RecoveryState::NotNeeded
    }
}

pub(super) fn describe_quarantine_state(
    directory: &PinnedDirectory,
    capture: &OsStr,
    identity: EntryIdentity,
) -> String {
    quarantine_recovery_state(directory, capture, Some(identity.source))
}

pub(super) fn revalidate_source(
    directory: &PinnedDirectory,
    name: &OsStr,
    expected: SourceIdentity,
    kind: &str,
) -> Result<(), String> {
    let metadata = directory
        .metadata(name)
        .map_err(|error| format!("cannot revalidate {kind}: {error}"))?;
    if expected.matches(&metadata) {
        Ok(())
    } else {
        Err(format!("refusing action because {kind} identity changed"))
    }
}

pub(super) fn create_private_directories(path: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700).create(path)
}

pub(super) fn recovery_name(directory: &Path, kind: &str, entry: &TrashEntry) -> PathBuf {
    let mut bytes = format!("{kind}-{}-", entry.id).into_bytes();
    let maximum = component_name_limit(directory);
    let remaining = maximum.saturating_sub(bytes.len());
    bytes.extend_from_slice(&entry.name.as_bytes()[..entry.name.as_bytes().len().min(remaining)]);
    directory.join(OsString::from_vec(bytes))
}

pub(super) fn plan_recovery_destinations(
    entry: &TrashEntry,
    directory: &Path,
) -> Result<RecoveryDestinations, String> {
    let payload = entry
        .payload_identity
        .map(|_| {
            choose_destination(
                &recovery_name(directory, "payload", entry),
                ConflictPolicy::Rename,
                None,
            )
        })
        .transpose()?;
    let info = entry
        .info_identity
        .map(|_| {
            choose_destination(
                &recovery_name(directory, "metadata", entry),
                ConflictPolicy::Rename,
                None,
            )
        })
        .transpose()?;
    Ok(RecoveryDestinations { payload, info })
}

pub(super) fn open_recovery_directory(path: &Path) -> Result<PinnedDirectory, String> {
    create_private_directories(path).map_err(|error| {
        format!(
            "cannot create recovery directory {}: {error}",
            quote(path.as_os_str())
        )
    })?;
    PinnedDirectory::open(path).map_err(|error| error.to_string())
}

pub(super) fn recover_captured_component(
    source: &PinnedDirectory,
    capture: &OsStr,
    identity: EntryIdentity,
    recovery: &PinnedDirectory,
    destination: &Path,
    id: &str,
    kind: &str,
) -> Result<ActionResult, String> {
    let destination_name = destination
        .file_name()
        .expect("validated recovery destination has a file name");
    if let Err(error) = rename_no_replace_at(source, capture, recovery, destination_name) {
        let state = describe_quarantine_state(source, capture, identity);
        return Err(format!(
            "cannot recover {kind} for {id}: {error}; recovery state: {state}"
        ));
    }
    if let Err(error) = revalidate_source(
        recovery,
        destination_name,
        identity.source,
        "recovered component",
    ) {
        return Err(format!(
            "{error}; the unexpected recovery destination was left untouched and the selected component location is unknown"
        ));
    }
    Ok(ActionResult {
        id: id.into(),
        path: Some(destination.to_path_buf()),
        warnings: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn recover_loose_item(
    source: &PinnedDirectory,
    root: &TrashRoot,
    name: &OsStr,
    metadata: &fs::Metadata,
    label: &str,
    recovery_directory: &Path,
    dry_run: bool,
    absent_counterpart: Option<(&PinnedDirectory, OsString)>,
    managed_roots: &[TrashRoot],
) -> Result<ActionResult, String> {
    let identity = EntryIdentity::from_metadata(metadata, true);
    let id = if absent_counterpart.is_some() {
        stable_id(root, name, Some(metadata), None)
    } else {
        stable_id(root, name, None, Some(metadata))
    };
    let proposed = recovery_directory.join(format!("{label}-{id}"));
    let destination = choose_destination(&proposed, ConflictPolicy::Rename, None)?;
    if dry_run {
        return Ok(ActionResult {
            id,
            path: Some(destination),
            warnings: Vec::new(),
        });
    }
    create_private_directories(recovery_directory).map_err(|error| {
        format!(
            "cannot create recovery directory {}: {error}",
            quote(recovery_directory.as_os_str())
        )
    })?;
    let recovery = PinnedDirectory::open(recovery_directory).map_err(|error| error.to_string())?;
    let destination_name = destination
        .file_name()
        .expect("generated recovery destination has a file name");
    reject_pinned_destination_in_roots(&recovery, destination_name, &destination, managed_roots)?;
    let captured = if let Some((counterpart, counterpart_name)) = absent_counterpart {
        revalidate(source, name, identity, label)?;
        let captured = capture(source, name, identity).map_err(|error| error.describe(source))?;
        if let Err(error) = require_absent(counterpart, &counterpart_name, "metadata") {
            let recovery = restore_captured(source, name, Some(&(captured.clone(), identity)));
            return Err(format!(
                "refusing loose payload recovery because its counterpart changed: {error}; payload recovery state: {recovery}"
            ));
        }
        captured
    } else {
        revalidate(source, name, identity, label)?;
        capture(source, name, identity).map_err(|error| error.describe(source))?
    };
    if let Err(error) = rename_no_replace_at(source, &captured, &recovery, destination_name) {
        let recovery = describe_quarantine_state(source, &captured, identity);
        return Err(format!(
            "cannot recover loose {label}: {error}; recovery state: {recovery}"
        ));
    }
    if let Err(error) = revalidate_source(
        &recovery,
        destination_name,
        identity.source,
        "recovered component",
    ) {
        return Err(format!(
            "{error}; the unexpected recovery destination was left untouched and the selected component location is unknown"
        ));
    }
    Ok(ActionResult {
        id,
        path: Some(destination),
        warnings: Vec::new(),
    })
}

pub(super) fn reject_pinned_destination_in_roots(
    directory: &PinnedDirectory,
    name: &OsStr,
    requested: &Path,
    roots: &[TrashRoot],
) -> Result<(), String> {
    let parent = PathBuf::from(format!("/proc/self/fd/{}", directory.raw_fd()))
        .canonicalize()
        .map_err(|error| {
            format!(
                "cannot resolve pinned destination parent {}: {error}",
                quote(requested.as_os_str())
            )
        })?;
    let pinned_destination = parent.join(name);
    if let Some(root) = roots.iter().find(|root| {
        root.aliases
            .iter()
            .any(|alias| pinned_destination.starts_with(alias))
    }) {
        Err(format!(
            "refusing destination {} because its pinned parent resolves inside Trash root {}",
            quote(requested.as_os_str()),
            quote(root.path.as_os_str())
        ))
    } else {
        Ok(())
    }
}

pub(super) fn component_name_limit(path: &Path) -> usize {
    let mut candidate = path;
    loop {
        if let Ok(directory) = PinnedDirectory::open(candidate) {
            return directory.name_max();
        }
        let Some(parent) = candidate.parent() else {
            return 255;
        };
        candidate = parent;
    }
}
