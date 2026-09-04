//! Stateless inspection and maintenance for FreeDesktop.org Trash directories.

mod actions;
mod cache;
mod discovery;
mod doctor;
mod metadata;
mod transaction;

#[cfg(test)]
use cache::remove_cache_name;
use cache::{read_directory_sizes, remove_cache_names};
use discovery::{
    FilesystemSelector, MountInspection, SharedTrashState, age_matches,
    canonical_existing_ancestor, collect_info_names, collect_payload_names,
    collect_root_quarantines, compare_entries, data_home_from_environment,
    filesystem_selector_for_mount, inspect_entry, inspect_mount, mount_for, now_epoch,
    open_optional_child, resolve_prospective_path, shared_trash_state, trash_candidate_exists,
};
#[cfg(test)]
use discovery::{classify_mount_access, is_internal_quarantine_name, is_internal_root_temporary};
#[cfg(test)]
use metadata::{parse_deletion_date_local, parse_info, percent_decode, stable_id};
use transaction::describe_quarantine_state;
#[cfg(test)]
use transaction::reject_pinned_destination_in_roots;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

#[cfg(test)]
use std::cell::Cell;

use crate::ctl_args::{ConflictPolicy, ListOptions};
use crate::display::quote;
use crate::freedesktop::{
    PinnedDirectory, SourceIdentity, effective_uid, mount_paths, mounted_paths,
};

const MAX_INFO_BYTES: u64 = 1024 * 1024;
const INFO_SUFFIX: &[u8] = b".trashinfo";
static NEXT_MANAGER_ID: AtomicU64 = AtomicU64::new(1);
#[cfg(test)]
thread_local! {
    static FORCE_METADATA_CLEANUP_FAILURE: Cell<bool> = const { Cell::new(false) };
    static FORCE_RESTORE_PUBLICATION_FAILURE: Cell<bool> = const { Cell::new(false) };
    static MOVE_RESTORE_DESTINATION_AFTER_PUBLICATION: Cell<bool> = const { Cell::new(false) };
    static FORCE_NEXT_INVENTORY_INCOMPLETE: Cell<bool> = const { Cell::new(false) };
    static FAIL_CAPTURE_VERIFICATION_ON: Cell<usize> = const { Cell::new(0) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntryState {
    Healthy,
    PayloadOnly,
    InfoOnly,
    Malformed,
}

impl EntryState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::PayloadOnly => "payload-only",
            Self::InfoOnly => "info-only",
            Self::Malformed => "malformed",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TrashEntry {
    pub id: String,
    pub name: OsString,
    pub payload: PathBuf,
    pub info: PathBuf,
    pub original_path: Option<PathBuf>,
    pub deletion_date: Option<String>,
    pub deleted_epoch: Option<u64>,
    pub size: Option<u64>,
    pub state: EntryState,
    pub problems: Vec<String>,
    info_mtime: Option<i64>,
    root_index: usize,
    payload_identity: Option<EntryIdentity>,
    info_identity: Option<EntryIdentity>,
    restore_name: Option<OsString>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EntryIdentity {
    source: SourceIdentity,
    ctime: Option<(i64, i64)>,
}

impl EntryIdentity {
    fn from_metadata(metadata: &fs::Metadata, include_ctime: bool) -> Self {
        Self {
            source: SourceIdentity::from_metadata(metadata),
            ctime: include_ctime.then(|| (metadata.ctime(), metadata.ctime_nsec())),
        }
    }

    fn matches(self, metadata: &fs::Metadata) -> bool {
        self.source.matches(metadata)
            && self
                .ctime
                .is_none_or(|ctime| ctime == (metadata.ctime(), metadata.ctime_nsec()))
    }

    fn is_directory(self) -> bool {
        self.source.is_directory()
    }
}

#[derive(Clone, Debug)]
struct TrashRoot {
    path: PathBuf,
    aliases: Vec<PathBuf>,
    volume_mount_points: Vec<PathBuf>,
    info_bases: Vec<PathBuf>,
    is_home: bool,
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Inventory {
    entries: Vec<TrashEntry>,
    id_index: HashMap<String, EntryIndex>,
    warnings: Vec<InventoryWarning>,
    complete: bool,
    manager_id: u64,
}

#[derive(Clone, Copy, Debug)]
enum EntryIndex {
    Unique(usize),
    Ambiguous,
}

impl Inventory {
    #[must_use]
    pub fn warnings(&self) -> impl ExactSizeIterator<Item = &str> {
        self.warnings.iter().map(|warning| warning.message.as_str())
    }

    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }
}

#[derive(Debug)]
pub struct AuthorizedInventory<'a> {
    manager: &'a TrashManager,
    inventory: &'a Inventory,
}

impl AuthorizedInventory<'_> {
    pub fn restore(
        &self,
        entry: &TrashEntry,
        destination_directory: Option<&Path>,
        conflict: ConflictPolicy,
        parents: bool,
        dry_run: bool,
        reserved_destinations: Option<&HashSet<PathBuf>>,
    ) -> Result<ActionResult, String> {
        self.require_entry(entry, "restore")?;
        self.manager.restore_entry(
            entry,
            destination_directory,
            conflict,
            parents,
            dry_run,
            reserved_destinations,
        )
    }

    pub fn purge(&self, entry: &TrashEntry, dry_run: bool) -> Result<ActionResult, String> {
        self.require_entry(entry, "purge")?;
        self.manager.purge_entry(entry, dry_run)
    }

    pub fn update_caches(&self, entries: &[&TrashEntry]) -> HashMap<String, String> {
        let mut groups: HashMap<usize, (HashSet<Vec<u8>>, Vec<String>)> = HashMap::new();
        let mut warnings = HashMap::new();
        for entry in entries {
            if let Err(error) = self.require_entry(entry, "update the cache for") {
                warnings.insert(entry.id.clone(), error);
                continue;
            }
            if !entry
                .payload_identity
                .is_some_and(EntryIdentity::is_directory)
            {
                continue;
            }
            let group = groups.entry(entry.root_index).or_default();
            group.0.insert(entry.name.as_bytes().to_vec());
            group.1.push(entry.id.clone());
        }
        for (root_index, (names, ids)) in groups {
            let root = &self.manager.roots[root_index];
            let result = self.manager.open_root(root).and_then(|directory| {
                remove_cache_names(&directory, &names).map_err(|error| error.to_string())
            });
            if let Err(error) = result {
                for id in ids {
                    warnings.insert(id, format!("could not update directorysizes: {error}"));
                }
            }
        }
        warnings
    }

    #[must_use]
    pub fn doctor(
        &self,
        repair_cache: bool,
        recovery_directory: Option<&Path>,
        dry_run: bool,
    ) -> DoctorReport {
        self.doctor_inventory(repair_cache, recovery_directory, dry_run)
    }

    fn require_entry(&self, entry: &TrashEntry, operation: &str) -> Result<(), String> {
        let authorized = self
            .inventory
            .id_index
            .get(&entry.id)
            .and_then(|index| match index {
                EntryIndex::Unique(index) => self.inventory.entries.get(*index),
                EntryIndex::Ambiguous => None,
            })
            .is_some_and(|candidate| std::ptr::eq(candidate, entry));
        if authorized {
            Ok(())
        } else {
            Err(format!(
                "refusing to {operation} an entry outside the authorized inventory"
            ))
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActionResult {
    pub id: String,
    pub path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DoctorReport {
    pub observed: Vec<DoctorProblem>,
    pub unresolved: Vec<DoctorProblem>,
    pub actions: Vec<ActionResult>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ProblemKey {
    DiscoveryWarning(usize),
    RootWarning {
        device: u64,
        inode: u64,
        index: usize,
    },
    #[cfg(test)]
    InjectedIncompleteInventory,
    Entry {
        id: String,
        index: usize,
    },
    ActionFailure(usize),
}

#[derive(Clone, Debug)]
struct InventoryWarning {
    key: ProblemKey,
    message: String,
}

#[derive(Clone, Debug)]
pub struct DoctorProblem {
    pub message: String,
    pub(crate) key: ProblemKey,
}

impl fmt::Display for DoctorProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

struct CapturedEntry {
    payload: Option<(OsString, EntryIdentity)>,
    info: Option<(OsString, EntryIdentity)>,
}

struct CaptureError {
    message: String,
    captured: Option<(OsString, EntryIdentity)>,
}

impl CaptureError {
    fn before_rename(message: String) -> Self {
        Self {
            message,
            captured: None,
        }
    }

    fn after_rename(message: String, name: OsString, identity: EntryIdentity) -> Self {
        Self {
            message,
            captured: Some((name, identity)),
        }
    }

    fn describe(&self, directory: &PinnedDirectory) -> String {
        match &self.captured {
            Some((name, identity)) => format!(
                "{}; recovery state: {}",
                self.message,
                describe_quarantine_state(directory, name, *identity)
            ),
            None => self.message.clone(),
        }
    }
}

struct EntryRecovery {
    payload: RecoveryState,
    metadata: RecoveryState,
}

enum RecoveryState {
    NotNeeded,
    Restored(OsString),
    Quarantined(String),
    Unknown(String),
}

impl fmt::Display for RecoveryState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotNeeded => formatter.write_str("not needed"),
            Self::Restored(name) => {
                write!(formatter, "verified restored as {}", quote(name))
            }
            Self::Quarantined(state) | Self::Unknown(state) => formatter.write_str(state),
        }
    }
}

struct RecoveryDestinations {
    payload: Option<PathBuf>,
    info: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct TrashManager {
    manager_id: u64,
    uid: u32,
    home_mount: PathBuf,
    roots: Vec<TrashRoot>,
    discovery_warnings: Vec<String>,
    discovery_complete: bool,
}

struct DiscoveryReport {
    warnings: Vec<String>,
    complete: bool,
}

impl DiscoveryReport {
    fn new() -> Self {
        Self {
            warnings: Vec::new(),
            complete: true,
        }
    }

    fn unknown(&mut self, warning: String) {
        self.complete = false;
        self.warnings.push(warning);
    }

    fn warn(&mut self, warning: String) {
        self.warnings.push(warning);
    }
}

fn inventory_problems(inventory: &Inventory) -> Vec<DoctorProblem> {
    let mut problems = inventory
        .warnings
        .iter()
        .map(|warning| DoctorProblem {
            key: warning.key.clone(),
            message: warning.message.clone(),
        })
        .collect::<Vec<_>>();
    for entry in &inventory.entries {
        problems.extend(
            entry
                .problems
                .iter()
                .enumerate()
                .map(|(index, problem)| DoctorProblem {
                    key: ProblemKey::Entry {
                        id: entry.id.clone(),
                        index,
                    },
                    message: format!("{}: {problem}", entry.id),
                }),
        );
    }
    problems
}

fn add_root_warnings(inventory: &mut Inventory, root: &TrashRoot, warnings: Vec<String>) {
    inventory
        .warnings
        .extend(
            warnings
                .into_iter()
                .enumerate()
                .map(|(index, message)| InventoryWarning {
                    key: ProblemKey::RootWarning {
                        device: root.device,
                        inode: root.inode,
                        index,
                    },
                    message,
                }),
        );
}

impl TrashManager {
    #[allow(clippy::too_many_lines)]
    pub fn discover() -> Result<Self, String> {
        let uid = effective_uid();
        let mut candidates = Vec::new();
        let mut discovery = DiscoveryReport::new();
        let data_home = data_home_from_environment()?;
        let mount_paths =
            mount_paths().map_err(|error| format!("cannot inspect mounts: {error}"))?;
        let mounts = mount_paths.all;
        let trash_mounts = mount_paths.trash_candidates;
        let resolved_data_home = canonical_existing_ancestor(&data_home)?;
        let home_mount = mount_for(&resolved_data_home, &mounts);
        candidates.push((
            data_home.join("Trash"),
            home_mount.clone(),
            resolved_data_home.clone(),
            true,
        ));
        for mount in &trash_mounts {
            match inspect_mount(mount, fs::metadata(mount), uid) {
                MountInspection::Present => {}
                MountInspection::Absent | MountInspection::OutsideUserScope => continue,
                MountInspection::Unknown(reason) => {
                    discovery.unknown(format!(
                        "cannot inspect mount point {}: {reason}",
                        quote(mount.as_os_str())
                    ));
                    continue;
                }
            }
            let shared = mount.join(".Trash");
            match shared_trash_state(&shared, uid) {
                SharedTrashState::Usable => {
                    candidates.push((
                        shared.join(uid.to_string()),
                        mount.clone(),
                        mount.clone(),
                        false,
                    ));
                }
                SharedTrashState::Missing => {}
                SharedTrashState::Unknown(reason) => discovery.unknown(format!(
                    "cannot inspect shared Trash {}: {reason}",
                    quote(shared.as_os_str())
                )),
                SharedTrashState::Unsafe(reason) => discovery.warn(format!(
                    "ignoring unsafe shared Trash {}: {reason}",
                    quote(shared.as_os_str())
                )),
            }
            candidates.push((
                mount.join(format!(".Trash-{uid}")),
                mount.clone(),
                mount.clone(),
                false,
            ));
        }

        let mut roots = Vec::new();
        let mut seen = HashMap::new();
        for (candidate, mount_point, info_base, is_home) in candidates {
            if !trash_candidate_exists(
                &candidate,
                fs::symlink_metadata(&candidate).map(|_| ()),
                &mut discovery,
            ) {
                continue;
            }
            let pinned = match PinnedDirectory::open_private(&candidate, uid) {
                Ok(directory) => directory,
                Err(error) => {
                    discovery.unknown(format!(
                        "ignoring unsafe Trash root {}: {error}",
                        quote(candidate.as_os_str())
                    ));
                    continue;
                }
            };
            let pinned_path = PathBuf::from(format!("/proc/self/fd/{}", pinned.raw_fd()));
            let canonical = match pinned_path.canonicalize() {
                Ok(path) => path,
                Err(error) => {
                    discovery.unknown(format!(
                        "cannot resolve Trash root {}: {error}",
                        quote(candidate.as_os_str())
                    ));
                    continue;
                }
            };
            let root_metadata = match fs::metadata(&pinned_path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    discovery.unknown(format!(
                        "cannot identify Trash root {}: {error}",
                        quote(candidate.as_os_str())
                    ));
                    continue;
                }
            };
            let identity = (root_metadata.dev(), root_metadata.ino());
            if let Some(index) = seen.get(&identity).copied() {
                let existing: &mut TrashRoot = &mut roots[index];
                if !existing.aliases.contains(&canonical) {
                    existing.aliases.push(canonical);
                    existing.aliases.sort();
                }
                if is_home {
                    existing.is_home = true;
                } else if !existing.volume_mount_points.contains(&mount_point) {
                    existing.volume_mount_points.push(mount_point);
                    existing.volume_mount_points.sort();
                }
                if !existing.info_bases.contains(&info_base) {
                    existing.info_bases.push(info_base);
                    existing.info_bases.sort();
                }
                continue;
            }
            seen.insert(identity, roots.len());
            roots.push(TrashRoot {
                path: canonical.clone(),
                aliases: vec![canonical],
                volume_mount_points: if is_home {
                    Vec::new()
                } else {
                    vec![mount_point]
                },
                info_bases: vec![info_base],
                is_home,
                device: root_metadata.dev(),
                inode: root_metadata.ino(),
            });
        }
        roots.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(Self {
            manager_id: NEXT_MANAGER_ID.fetch_add(1, AtomicOrdering::Relaxed),
            uid,
            home_mount,
            roots,
            discovery_warnings: discovery.warnings,
            discovery_complete: discovery.complete,
        })
    }

    #[must_use]
    pub fn inventory(&self) -> Inventory {
        let mut inventory = Inventory {
            entries: Vec::new(),
            id_index: HashMap::new(),
            warnings: self
                .discovery_warnings
                .iter()
                .enumerate()
                .map(|(index, message)| InventoryWarning {
                    key: ProblemKey::DiscoveryWarning(index),
                    message: message.clone(),
                })
                .collect(),
            complete: self.discovery_complete,
            manager_id: self.manager_id,
        };
        for (root_index, root) in self.roots.iter().enumerate() {
            self.inventory_root(root_index, root, &mut inventory);
        }
        for (index, entry) in inventory.entries.iter().enumerate() {
            inventory
                .id_index
                .entry(entry.id.clone())
                .and_modify(|entry| *entry = EntryIndex::Ambiguous)
                .or_insert(EntryIndex::Unique(index));
        }
        for entry in &mut inventory.entries {
            if matches!(
                inventory.id_index.get(&entry.id),
                Some(EntryIndex::Ambiguous)
            ) {
                entry
                    .problems
                    .push("stable ID collision; actions using this ID are refused".into());
                entry.state = EntryState::Malformed;
            }
        }
        #[cfg(test)]
        FORCE_NEXT_INVENTORY_INCOMPLETE.with(|force| {
            if force.replace(false) {
                inventory.complete = false;
                inventory.warnings.push(InventoryWarning {
                    key: ProblemKey::InjectedIncompleteInventory,
                    message: "injected incomplete refreshed inventory".into(),
                });
            }
        });
        inventory
    }

    fn inventory_root(&self, root_index: usize, root: &TrashRoot, inventory: &mut Inventory) {
        let mut warnings = Vec::new();
        let root_directory = match self.open_root(root) {
            Ok(directory) => directory,
            Err(error) => {
                warnings.push(format!(
                    "cannot inspect Trash root {}: {error}",
                    quote(root.path.as_os_str())
                ));
                add_root_warnings(inventory, root, warnings);
                inventory.complete = false;
                return;
            }
        };
        let files_path = root.path.join("files");
        let info_path = root.path.join("info");
        let files_directory = open_optional_child(
            &root_directory,
            OsStr::new("files"),
            &files_path,
            self.uid,
            &mut warnings,
        );
        let info_directory = open_optional_child(
            &root_directory,
            OsStr::new("info"),
            &info_path,
            self.uid,
            &mut warnings,
        );
        let mut root_complete = files_directory.is_some() && info_directory.is_some();

        let mut names = BTreeSet::new();
        if let Some(directory) = files_directory.as_ref() {
            root_complete &=
                collect_payload_names(directory, &files_path, &mut names, &mut warnings);
        }
        if let Some(directory) = info_directory.as_ref() {
            root_complete &= collect_info_names(directory, &info_path, &mut names, &mut warnings);
        }
        root_complete &= collect_root_quarantines(&root_directory, &root.path, &mut warnings);
        if !root_complete {
            inventory.complete = false;
        }
        let cache = match read_directory_sizes(&root_directory) {
            Ok(cache) => cache,
            Err(error) => {
                warnings.push(format!(
                    "cannot read {}: {error}",
                    quote(root.path.join("directorysizes").as_os_str())
                ));
                HashMap::new()
            }
        };
        for name in names {
            let (entry, trusted) = inspect_entry(
                root_index,
                root,
                &name,
                files_directory.as_ref(),
                info_directory.as_ref(),
                &cache,
            );
            inventory.entries.push(entry);
            if !trusted {
                inventory.complete = false;
            }
        }
        add_root_warnings(inventory, root, warnings);
    }

    pub fn select<'a>(
        &self,
        inventory: &'a Inventory,
        ids: &[String],
    ) -> Result<Vec<&'a TrashEntry>, String> {
        if inventory.manager_id != self.manager_id {
            return Err("refusing to select from an inventory issued by another manager".into());
        }
        let mut selected = Vec::with_capacity(ids.len());
        let mut unique = HashSet::new();
        for id in ids {
            if !unique.insert(id) {
                return Err(format!("entry ID {id} was specified more than once"));
            }
            let entry = match inventory.id_index.get(id) {
                Some(EntryIndex::Unique(index)) => &inventory.entries[*index],
                Some(EntryIndex::Ambiguous) => {
                    return Err(format!("entry ID {id} is ambiguous; no action was taken"));
                }
                None => return Err(format!("no Trash entry has ID {id}")),
            };
            selected.push(entry);
        }
        Ok(selected)
    }

    pub fn authorize_mutations<'a>(
        &'a self,
        inventory: &'a Inventory,
        operation: &str,
    ) -> Result<AuthorizedInventory<'a>, String> {
        if inventory.manager_id != self.manager_id {
            return Err(format!(
                "refusing to {operation} using an inventory from another Trash manager"
            ));
        }
        if !inventory.complete {
            return Err(format!(
                "refusing to {operation} because Trash inventory was incomplete"
            ));
        }
        Ok(AuthorizedInventory {
            manager: self,
            inventory,
        })
    }

    pub fn filter_list<'a>(
        &self,
        inventory: &'a Inventory,
        options: &ListOptions,
    ) -> Result<Vec<&'a TrashEntry>, String> {
        let filesystem = options
            .filesystem
            .as_deref()
            .map(|path| self.filesystem_selector(path))
            .transpose()?;
        let now = now_epoch();
        let mut entries: Vec<_> = inventory
            .entries
            .iter()
            .filter(|entry| {
                filesystem
                    .as_ref()
                    .is_none_or(|selector| selector.matches(&self.roots[entry.root_index]))
            })
            .filter(|entry| age_matches(entry, now, options.older_than, options.newer_than))
            .collect();
        entries.sort_by(|left, right| compare_entries(left, right, options.sort));
        if options.reverse {
            entries.reverse();
        }
        Ok(entries)
    }

    pub fn filter_empty<'a>(
        &self,
        inventory: &'a Inventory,
        filesystem: Option<&Path>,
        older_than: Option<u64>,
    ) -> Result<Vec<&'a TrashEntry>, String> {
        let filesystem = filesystem
            .map(|path| self.filesystem_selector(path))
            .transpose()?;
        let now = now_epoch();
        Ok(inventory
            .entries
            .iter()
            .filter(|entry| {
                filesystem
                    .as_ref()
                    .is_none_or(|selector| selector.matches(&self.roots[entry.root_index]))
            })
            .filter(|entry| age_matches(entry, now, older_than, None))
            .collect())
    }

    fn open_entry_root(
        &self,
        entry: &TrashEntry,
    ) -> Result<(PinnedDirectory, PinnedDirectory, PinnedDirectory), String> {
        let entry_root = &self.roots[entry.root_index];
        let root = self.open_root(entry_root)?;
        let files = root
            .open_private_child(
                OsStr::new("files"),
                &entry_root.path.join("files"),
                self.uid,
            )
            .map_err(|error| error.to_string())?;
        let info = root
            .open_private_child(OsStr::new("info"), &entry_root.path.join("info"), self.uid)
            .map_err(|error| error.to_string())?;
        Ok((root, files, info))
    }

    fn open_root(&self, root: &TrashRoot) -> Result<PinnedDirectory, String> {
        let directory = PinnedDirectory::open_private(&root.path, self.uid)
            .map_err(|error| error.to_string())?;
        let metadata = fs::metadata(format!("/proc/self/fd/{}", directory.raw_fd()))
            .map_err(|error| format!("cannot revalidate Trash root identity: {error}"))?;
        if metadata.dev() != root.device || metadata.ino() != root.inode {
            return Err("refusing action because Trash root identity changed".into());
        }
        Ok(directory)
    }

    fn filesystem_selector(&self, path: &Path) -> Result<FilesystemSelector, String> {
        let resolved = path.canonicalize().map_err(|error| {
            format!(
                "cannot resolve filesystem selector {}: {error}",
                quote(path.as_os_str())
            )
        })?;
        let mounts = mounted_paths().map_err(|error| format!("cannot inspect mounts: {error}"))?;
        let selected_mount = mount_for(&resolved, &mounts);
        Ok(filesystem_selector_for_mount(
            &self.home_mount,
            selected_mount,
        ))
    }

    fn reject_destination_in_trash(&self, path: &Path) -> Result<(), String> {
        let resolved = resolve_prospective_path(path)?;
        if let Some(root) = self
            .roots
            .iter()
            .find(|root| root.aliases.iter().any(|alias| resolved.starts_with(alias)))
        {
            Err(format!(
                "refusing destination {} because it is inside Trash root {}",
                quote(path.as_os_str()),
                quote(root.path.as_os_str())
            ))
        } else {
            Ok(())
        }
    }

    fn reject_recovery_overlap(&self, path: &Path) -> Result<(), String> {
        let resolved = resolve_prospective_path(path)?;
        if let Some(root) = self
            .roots
            .iter()
            .find(|root| root.aliases.iter().any(|alias| resolved.starts_with(alias)))
        {
            Err(format!(
                "refusing recovery directory {} because it overlaps Trash root {}",
                quote(path.as_os_str()),
                quote(root.path.as_os_str())
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
