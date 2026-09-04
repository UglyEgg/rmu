use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use super::discovery::refuse_mounted_payload;
use super::transaction::{
    capture_entry_snapshot, choose_destination, describe_quarantine_state,
    publish_restored_payload, quarantine_recovery_status, reject_pinned_destination_in_roots,
    require_healthy, revalidate_source, rollback_captured_entry, rollback_published_restore,
};
use super::{ActionResult, CapturedEntry, EntryIdentity, TrashEntry, TrashManager};
use crate::ctl_args::ConflictPolicy;
use crate::display::quote;
use crate::freedesktop::{PinnedDirectory, remove_tree_if_matches};

#[cfg(test)]
use super::{FORCE_METADATA_CLEANUP_FAILURE, MOVE_RESTORE_DESTINATION_AFTER_PUBLICATION};
#[cfg(test)]
use std::io;

impl TrashManager {
    #[allow(clippy::too_many_lines)]
    pub(super) fn restore_entry(
        &self,
        entry: &TrashEntry,
        destination_directory: Option<&Path>,
        conflict: ConflictPolicy,
        parents: bool,
        dry_run: bool,
        reserved_destinations: Option<&HashSet<PathBuf>>,
    ) -> Result<ActionResult, String> {
        require_healthy(entry, "restore")?;
        let destination = if let Some(directory) = destination_directory {
            let name = entry
                .restore_name
                .as_ref()
                .ok_or_else(|| format!("{} has no restorable file name", entry.id))?;
            directory.join(name)
        } else {
            entry.original_path.clone().ok_or_else(|| {
                format!(
                    "{} has an ambiguous original path across mount aliases; use restore --to",
                    entry.id
                )
            })?
        };
        let destination = choose_destination(&destination, conflict, reserved_destinations)?;
        self.reject_destination_in_trash(&destination)?;
        if dry_run {
            return Ok(ActionResult {
                id: entry.id.clone(),
                path: Some(destination),
                warnings: Vec::new(),
            });
        }
        let parent = destination
            .parent()
            .ok_or_else(|| "restore destination has no parent directory".to_string())?;
        if parents {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "cannot create restore parent {}: {error}",
                    quote(parent.as_os_str())
                )
            })?;
        }
        let destination_parent =
            PinnedDirectory::open(parent).map_err(|error| error.to_string())?;
        let destination_name = destination
            .file_name()
            .ok_or_else(|| "restore destination has no file name".to_string())?;
        reject_pinned_destination_in_roots(
            &destination_parent,
            destination_name,
            &destination,
            &self.roots,
        )?;

        let (_root_directory, files_directory, info_directory) = self.open_entry_root(entry)?;
        let payload_identity = entry
            .payload_identity
            .expect("healthy entry has payload identity");
        let info_identity = entry
            .info_identity
            .expect("healthy entry has info identity");
        let captured = capture_entry_snapshot(entry, &files_directory, &info_directory)?;
        let (payload_capture, _) = captured
            .payload
            .as_ref()
            .expect("healthy entry snapshot includes payload");
        let (info_capture, _) = captured
            .info
            .as_ref()
            .expect("healthy entry snapshot includes metadata");
        let prepublication = destination_parent
            .revalidate_path(parent)
            .map_err(|error| format!("restore destination parent changed: {error}"))
            .and_then(|()| {
                reject_pinned_destination_in_roots(
                    &destination_parent,
                    destination_name,
                    &destination,
                    &self.roots,
                )
            });
        if let Err(error) = prepublication {
            let recovery =
                rollback_captured_entry(entry, &captured, &files_directory, &info_directory);
            return Err(format!(
                "{error}; payload recovery state: {}; metadata recovery state: {}",
                recovery.payload, recovery.metadata
            ));
        }
        if let Err(error) = publish_restored_payload(
            &files_directory,
            payload_capture,
            &destination_parent,
            destination_name,
        ) {
            let recovery =
                rollback_captured_entry(entry, &captured, &files_directory, &info_directory);
            return Err(format!(
                "cannot restore {} to {}: {error}; payload recovery state: {}; metadata recovery state: {}",
                entry.id,
                quote(destination.as_os_str()),
                recovery.payload,
                recovery.metadata
            ));
        }

        #[cfg(test)]
        MOVE_RESTORE_DESTINATION_AFTER_PUBLICATION.with(|move_parent| {
            if move_parent.replace(false) {
                let parked = parent
                    .parent()
                    .expect("restore parent has a parent")
                    .join(".rmu-test-moved-restore-parent");
                fs::rename(parent, parked).unwrap();
                fs::create_dir(parent).unwrap();
            }
        });

        if let Err(error) = revalidate_source(
            &destination_parent,
            destination_name,
            payload_identity.source,
            "restored payload",
        ) {
            return Err(format!(
                "{error}; the unexpected destination entry was left untouched and the selected payload location is unknown; metadata recovery state: {}",
                describe_quarantine_state(&info_directory, info_capture, info_identity)
            ));
        }

        let namespace = destination_parent
            .revalidate_path(parent)
            .map_err(|error| format!("restore destination parent changed: {error}"))
            .and_then(|()| {
                reject_pinned_destination_in_roots(
                    &destination_parent,
                    destination_name,
                    &destination,
                    &self.roots,
                )
            });
        if let Err(error) = namespace {
            let recovery = rollback_published_restore(
                entry,
                &captured,
                &files_directory,
                &info_directory,
                &destination_parent,
                destination_name,
            );
            return Err(format!("{error}; {recovery}"));
        }

        let mut warnings = Vec::new();
        if let Err(error) =
            info_directory.remove_file_if_matches(info_capture, info_identity.source)
        {
            warnings.push(format!(
                "restored payload, but could not remove its metadata: {error}; metadata recovery: {}",
                describe_quarantine_state(&info_directory, info_capture, info_identity)
            ));
        }
        Ok(ActionResult {
            id: entry.id.clone(),
            path: Some(destination),
            warnings,
        })
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn purge_entry(
        &self,
        entry: &TrashEntry,
        dry_run: bool,
    ) -> Result<ActionResult, String> {
        if dry_run {
            return Ok(ActionResult {
                id: entry.id.clone(),
                path: None,
                warnings: Vec::new(),
            });
        }
        if entry
            .payload_identity
            .is_some_and(EntryIdentity::is_directory)
        {
            refuse_mounted_payload(&entry.payload)?;
        }
        let entry_root = &self.roots[entry.root_index];
        let root_directory = self.open_root(entry_root)?;
        let files_directory = root_directory
            .open_private_child(
                OsStr::new("files"),
                &entry_root.path.join("files"),
                self.uid,
            )
            .map_err(|error| error.to_string())?;
        let info_directory = root_directory
            .open_private_child(OsStr::new("info"), &entry_root.path.join("info"), self.uid)
            .map_err(|error| error.to_string())?;
        let CapturedEntry {
            payload: payload_capture,
            info: info_capture,
        } = capture_entry_snapshot(entry, &files_directory, &info_directory)?;

        if let Some((capture_name, identity)) = &payload_capture {
            let files = &files_directory;
            let deletion = if identity.is_directory() {
                if let Err(error) =
                    revalidate_source(files, capture_name, identity.source, "captured payload")
                {
                    let payload_recovery =
                        quarantine_recovery_status(files, payload_capture.as_ref());
                    let metadata_recovery =
                        quarantine_recovery_status(&info_directory, info_capture.as_ref());
                    return Err(format!(
                        "{error}; payload recovery state: {payload_recovery}; metadata recovery state: {metadata_recovery}"
                    ));
                }
                if let Err(error) =
                    refuse_mounted_payload(&entry_root.path.join("files").join(capture_name))
                {
                    let payload_recovery =
                        quarantine_recovery_status(files, payload_capture.as_ref());
                    let metadata_recovery =
                        quarantine_recovery_status(&info_directory, info_capture.as_ref());
                    return Err(format!(
                        "{error}; payload recovery: {payload_recovery}; metadata recovery: {metadata_recovery}"
                    ));
                }
                remove_tree_if_matches(files, capture_name, identity.source)
            } else {
                files.remove_file_if_matches(capture_name, identity.source)
            };
            if let Err(error) = deletion {
                let payload_recovery = quarantine_recovery_status(files, payload_capture.as_ref());
                let metadata_recovery =
                    quarantine_recovery_status(&info_directory, info_capture.as_ref());
                return Err(format!(
                    "cannot permanently remove {}: {error}; payload recovery state: {payload_recovery}; metadata recovery state: {metadata_recovery}",
                    entry.id
                ));
            }
        }

        if let Some((capture_name, identity)) = &info_capture {
            let info = &info_directory;
            #[cfg(test)]
            let metadata_cleanup = FORCE_METADATA_CLEANUP_FAILURE.with(|force| {
                if force.replace(false) {
                    Err(io::Error::other("injected metadata cleanup failure"))
                } else {
                    info.remove_file_if_matches(capture_name, identity.source)
                }
            });
            #[cfg(not(test))]
            let metadata_cleanup = info.remove_file_if_matches(capture_name, identity.source);
            if let Err(error) = metadata_cleanup {
                return Err(format!(
                    "payload was permanently removed, but metadata cleanup failed: {error}; metadata recovery: {}",
                    describe_quarantine_state(info, capture_name, *identity)
                ));
            }
        }
        Ok(ActionResult {
            id: entry.id.clone(),
            path: None,
            warnings: Vec::new(),
        })
    }
}
