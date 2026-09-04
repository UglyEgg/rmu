use std::ffi::OsStr;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::cache::replace_cache;
use super::discovery::{
    is_internal_quarantine_name, is_internal_root_temporary, pinned_directory_path,
};
use super::transaction::{
    capture_entry_snapshot, open_recovery_directory, plan_recovery_destinations,
    recover_captured_component, recover_loose_item, reject_pinned_destination_in_roots, revalidate,
};
use super::{
    ActionResult, AuthorizedInventory, DoctorReport, EntryIdentity, EntryState, Inventory,
    TrashEntry, TrashManager, TrashRoot, inventory_problems,
};
use crate::display::quote;
use crate::freedesktop::{info_name, percent_encode};
use crate::scanner::scan_tree;

impl TrashManager {
    pub fn diagnose(&self, inventory: &Inventory) -> Result<DoctorReport, String> {
        if inventory.manager_id != self.manager_id {
            return Err("refusing to diagnose an inventory from another Trash manager".into());
        }
        let observed = inventory_problems(inventory);
        Ok(DoctorReport {
            unresolved: observed.clone(),
            observed,
            actions: Vec::new(),
        })
    }

    fn recover_entry(
        &self,
        entry: &TrashEntry,
        recovery_directory: &Path,
        dry_run: bool,
    ) -> Vec<Result<ActionResult, String>> {
        let destinations = match plan_recovery_destinations(entry, recovery_directory) {
            Ok(destinations) => destinations,
            Err(error) => return vec![Err(error)],
        };
        if dry_run {
            return [destinations.payload, destinations.info]
                .into_iter()
                .flatten()
                .map(|destination| {
                    Ok(ActionResult {
                        id: entry.id.clone(),
                        path: Some(destination),
                        warnings: Vec::new(),
                    })
                })
                .collect();
        }
        let recovery = match open_recovery_directory(recovery_directory) {
            Ok(directory) => directory,
            Err(error) => return vec![Err(error)],
        };
        for destination in [&destinations.payload, &destinations.info]
            .into_iter()
            .flatten()
        {
            let Some(destination_name) = destination.file_name() else {
                return vec![Err("recovery destination has no file name".into())];
            };
            if let Err(error) = reject_pinned_destination_in_roots(
                &recovery,
                destination_name,
                destination,
                &self.roots,
            ) {
                return vec![Err(error)];
            }
        }
        let entry_root = &self.roots[entry.root_index];
        let root = match self.open_root(entry_root) {
            Ok(root) => root,
            Err(error) => return vec![Err(error)],
        };
        let files = match root.open_private_child(
            OsStr::new("files"),
            &entry_root.path.join("files"),
            self.uid,
        ) {
            Ok(directory) => directory,
            Err(error) => return vec![Err(error.to_string())],
        };
        let info = match root.open_private_child(
            OsStr::new("info"),
            &entry_root.path.join("info"),
            self.uid,
        ) {
            Ok(directory) => directory,
            Err(error) => return vec![Err(error.to_string())],
        };
        let captured = match capture_entry_snapshot(entry, &files, &info) {
            Ok(captured) => captured,
            Err(error) => return vec![Err(error)],
        };
        let mut results = Vec::new();
        if let (Some((capture, identity)), Some(destination)) =
            (captured.payload, destinations.payload)
        {
            results.push(recover_captured_component(
                &files,
                &capture,
                identity,
                &recovery,
                &destination,
                &entry.id,
                "payload",
            ));
        }
        if let (Some((capture, identity)), Some(destination)) = (captured.info, destinations.info) {
            results.push(recover_captured_component(
                &info,
                &capture,
                identity,
                &recovery,
                &destination,
                &entry.id,
                "metadata",
            ));
        }
        results
    }

    #[allow(clippy::too_many_lines)]
    fn recover_loose_entries(
        &self,
        root: &TrashRoot,
        recovery_directory: &Path,
        dry_run: bool,
    ) -> Vec<Result<ActionResult, String>> {
        let root_directory = match self.open_root(root) {
            Ok(directory) => directory,
            Err(error) => return vec![Err(error)],
        };
        let files_path = root.path.join("files");
        let files_directory =
            match root_directory.open_private_child(OsStr::new("files"), &files_path, self.uid) {
                Ok(directory) => directory,
                Err(error) => {
                    return vec![Err(format!(
                        "cannot inspect {}: {error}",
                        quote(files_path.as_os_str())
                    ))];
                }
            };
        let info_path = root.path.join("info");
        let info_directory =
            match root_directory.open_private_child(OsStr::new("info"), &info_path, self.uid) {
                Ok(directory) => directory,
                Err(error) => {
                    return vec![Err(format!(
                        "cannot inspect {}: {error}",
                        quote(info_path.as_os_str())
                    ))];
                }
            };
        let mut results = Vec::new();
        for (child, label, directory) in [
            ("files", "payload", &files_directory),
            ("info", "metadata", &info_directory),
        ] {
            let path = root.path.join(child);
            let entries = match fs::read_dir(pinned_directory_path(directory)) {
                Ok(entries) => entries,
                Err(error) => {
                    results.push(Err(format!(
                        "cannot enumerate {}: {error}",
                        quote(path.as_os_str())
                    )));
                    continue;
                }
            };
            let mut items = Vec::new();
            for item in entries {
                match item {
                    Ok(item) => items.push(item),
                    Err(error) => {
                        results.push(Err(format!(
                            "cannot enumerate {} completely: {error}",
                            quote(path.as_os_str())
                        )));
                    }
                }
            }
            items.sort_by_key(fs::DirEntry::file_name);
            for item in items {
                let name = item.file_name();
                let is_loose = if child == "files" {
                    is_internal_quarantine_name(&name)
                        && info_directory
                            .metadata(&info_name(&name))
                            .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
                } else {
                    is_internal_quarantine_name(&name)
                };
                if !is_loose {
                    continue;
                }
                let metadata = match directory.metadata(&name) {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        results.push(Err(format!(
                            "cannot inspect loose {label} {}: {error}",
                            quote(name.as_os_str())
                        )));
                        continue;
                    }
                };
                let absent_counterpart =
                    (child == "files").then(|| (&info_directory, info_name(&name)));
                results.push(recover_loose_item(
                    directory,
                    root,
                    &name,
                    &metadata,
                    label,
                    recovery_directory,
                    dry_run,
                    absent_counterpart,
                    &self.roots,
                ));
            }
        }
        match fs::read_dir(pinned_directory_path(&root_directory)) {
            Ok(entries) => {
                let mut items = Vec::new();
                for item in entries {
                    match item {
                        Ok(item) => items.push(item),
                        Err(error) => {
                            results.push(Err(format!(
                                "cannot enumerate {} completely: {error}",
                                quote(root.path.as_os_str())
                            )));
                        }
                    }
                }
                items.sort_by_key(fs::DirEntry::file_name);
                for item in items {
                    let name = item.file_name();
                    if !is_internal_root_temporary(&name) {
                        continue;
                    }
                    match root_directory.metadata(&name) {
                        Ok(metadata) => results.push(recover_loose_item(
                            &root_directory,
                            root,
                            &name,
                            &metadata,
                            "cache",
                            recovery_directory,
                            dry_run,
                            None,
                            &self.roots,
                        )),
                        Err(error) => results.push(Err(format!(
                            "cannot inspect loose cache {}: {error}",
                            quote(name.as_os_str())
                        ))),
                    }
                }
            }
            Err(error) => results.push(Err(format!(
                "cannot enumerate {}: {error}",
                quote(root.path.as_os_str())
            ))),
        }
        results
    }
}

impl AuthorizedInventory<'_> {
    pub(super) fn doctor_inventory(
        &self,
        repair_cache: bool,
        recovery_directory: Option<&Path>,
        dry_run: bool,
    ) -> DoctorReport {
        let observed = inventory_problems(self.inventory);
        let mut actions = Vec::new();
        let mut action_failures = Vec::new();
        if let Some(directory) = recovery_directory {
            match self.manager.reject_recovery_overlap(directory) {
                Ok(()) => {
                    for entry in &self.inventory.entries {
                        if entry.state == EntryState::Healthy {
                            continue;
                        }
                        for result in self.manager.recover_entry(entry, directory, dry_run) {
                            match result {
                                Ok(action) => actions.push(action),
                                Err(error) => action_failures.push(error),
                            }
                        }
                    }
                    for root in &self.manager.roots {
                        for result in self.manager.recover_loose_entries(root, directory, dry_run) {
                            match result {
                                Ok(action) => actions.push(action),
                                Err(error) => action_failures.push(error),
                            }
                        }
                    }
                }
                Err(error) => action_failures.push(error),
            }
        }
        if repair_cache {
            if dry_run || recovery_directory.is_none() {
                self.repair_all_caches(dry_run, &mut actions, &mut action_failures);
            } else {
                let refreshed = self.manager.inventory();
                match self
                    .manager
                    .authorize_mutations(&refreshed, "repair caches after recovery")
                {
                    Ok(authorized) => {
                        authorized.repair_all_caches(false, &mut actions, &mut action_failures);
                    }
                    Err(error) => action_failures.push(error),
                }
            }
        }
        let mut unresolved = if dry_run {
            observed.clone()
        } else {
            inventory_problems(&self.manager.inventory())
        };
        unresolved.extend(
            action_failures
                .into_iter()
                .enumerate()
                .map(|(index, message)| super::DoctorProblem {
                    key: super::ProblemKey::ActionFailure(index),
                    message,
                }),
        );
        DoctorReport {
            observed,
            unresolved,
            actions,
        }
    }

    fn repair_all_caches(
        &self,
        dry_run: bool,
        actions: &mut Vec<ActionResult>,
        failures: &mut Vec<String>,
    ) {
        for (root_index, root) in self.manager.roots.iter().enumerate() {
            match self.repair_cache(root_index, root, dry_run) {
                Ok(action) => actions.push(action),
                Err(error) => failures.push(error),
            }
        }
    }

    fn repair_cache(
        &self,
        root_index: usize,
        root: &TrashRoot,
        dry_run: bool,
    ) -> Result<ActionResult, String> {
        let root_entries = || {
            self.inventory
                .entries
                .iter()
                .filter(|entry| entry.root_index == root_index)
                .filter(|entry| entry.state == EntryState::Healthy)
                .filter(|entry| {
                    entry
                        .payload_identity
                        .is_some_and(EntryIdentity::is_directory)
                })
        };
        if dry_run {
            return Ok(ActionResult {
                id: "directorysizes".into(),
                path: Some(root.path.join("directorysizes")),
                warnings: vec![format!(
                    "would rebuild {} directory entries",
                    root_entries().count()
                )],
            });
        }
        let root_directory = self.manager.open_root(root)?;
        let files_directory = root_directory
            .open_private_child(
                OsStr::new("files"),
                &root.path.join("files"),
                self.manager.uid,
            )
            .map_err(|error| error.to_string())?;
        let mut contents = Vec::new();
        let mut failures = Vec::new();
        for entry in root_entries() {
            let Some(identity) = entry.payload_identity else {
                continue;
            };
            if let Err(error) = revalidate(&files_directory, &entry.name, identity, "payload") {
                failures.push(format!("skipped {}: {error}", entry.id));
                continue;
            }
            let pinned_files = PathBuf::from(format!("/proc/self/fd/{}", files_directory.raw_fd()));
            match scan_tree(&pinned_files.join(&entry.name), |_| {}) {
                Ok(summary) if summary.complete() => {
                    writeln!(
                        &mut contents,
                        "{} {} {}",
                        summary.bytes,
                        entry.info_mtime.unwrap_or(0),
                        percent_encode(entry.name.as_bytes())
                    )
                    .expect("writing to a byte vector cannot fail");
                    if let Err(error) = crate::directory_sizes::ensure_serialized(&contents) {
                        failures.push(format!("skipped {}: {error}", entry.id));
                        break;
                    }
                }
                Ok(summary) => failures.push(format!(
                    "skipped {} because {} entries were unreadable",
                    entry.id, summary.unreadable
                )),
                Err(error) => failures.push(format!("skipped {}: {error}", entry.id)),
            }
        }
        if !failures.is_empty() {
            return Err(format!(
                "refusing to publish a partial directorysizes cache: {}",
                failures.join("; ")
            ));
        }
        replace_cache(&root_directory, &contents)
            .map_err(|error| format!("cannot rebuild {}: {error}", quote(root.path.as_os_str())))?;
        Ok(ActionResult {
            id: "directorysizes".into(),
            path: Some(root.path.join("directorysizes")),
            warnings: Vec::new(),
        })
    }
}
