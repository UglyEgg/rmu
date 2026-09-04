mod fsops;

use fsops::create_info_file_at;
pub(crate) use fsops::{
    PinnedDirectory, open_file_at, open_new_private_file_at, quarantine_recovery_state,
    remove_tree_if_matches, rename_no_replace_at, rename_replace_at, with_cleanup_error,
    write_and_sync,
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, DirBuilder, File};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
use std::cell::Cell;

use crate::directory_sizes::{
    ensure_serialized, read as read_cache_file, replace as replace_cache_file,
};
use crate::display::quote;
use crate::linux::local_calendar_time;
#[cfg(test)]
use crate::scanner::scan_tree;

const TRASH_INFO_SUFFIX: &[u8] = b".trashinfo";
static NEXT_CACHE_TEMP: AtomicU64 = AtomicU64::new(0);
static NEXT_COLLISION_SUFFIX: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
thread_local! {
    static REMOVE_INFO_AFTER_MOVE: Cell<bool> = const { Cell::new(false) };
    static MUTATE_INFO_AFTER_MOVE: Cell<bool> = const { Cell::new(false) };
    static REMOVE_PAYLOAD_AFTER_MOVE: Cell<bool> = const { Cell::new(false) };
    static REMOVE_PAYLOAD_AFTER_CACHE: Cell<bool> = const { Cell::new(false) };
    static MOVE_TRASH_ROOT_AFTER_PAYLOAD: Cell<bool> = const { Cell::new(false) };
    static SWAP_TREE_AFTER_PIN: Cell<bool> = const { Cell::new(false) };
    static SWAP_EMPTY_DIRECTORY_AFTER_REVALIDATION: Cell<bool> = const { Cell::new(false) };
    static FORCE_CHILD_MOUNT_BOUNDARY: Cell<bool> = const { Cell::new(false) };
    static FORCE_INFO_CLEANUP_FAILURE: Cell<bool> = const { Cell::new(false) };
    static FORCE_PAYLOAD_MOVE_FAILURE: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
    static REMOVE_CAPTURED_CLEANUP_BEFORE_UNLINK: Cell<bool> = const { Cell::new(false) };
}

#[cfg(test)]
mod integration_tests;

unsafe extern "C" {
    fn geteuid() -> u32;
}

pub(crate) fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail on Linux.
    unsafe { geteuid() }
}

#[cfg(test)]
pub(crate) fn force_child_mount_boundary_for_test() {
    FORCE_CHILD_MOUNT_BOUNDARY.with(|force| force.set(true));
}

#[derive(Clone, Debug)]
pub struct TrashBackend {
    data_home: PathBuf,
    uid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrashReceipt {
    pub payload: PathBuf,
    pub info: PathBuf,
    pub warnings: Vec<String>,
    pub(crate) cache_update: Option<DirectorySizeUpdate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DirectorySizeUpdate {
    root: PathBuf,
    root_identity: SourceIdentity,
    name: OsString,
    size: u64,
    info_mtime: i64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceIdentity {
    device: u64,
    inode: u64,
    file_type: u32,
}

impl SourceIdentity {
    #[must_use]
    pub fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            file_type: metadata.mode() & 0o170_000,
        }
    }

    pub(crate) fn matches(self, metadata: &fs::Metadata) -> bool {
        self == Self::from_metadata(metadata)
    }

    #[must_use]
    pub fn is_directory(self) -> bool {
        self.file_type == 0o040_000
    }
}

#[derive(Debug)]
pub struct TrashError {
    message: String,
    source: Option<io::Error>,
    fallback_safe: bool,
}

impl TrashError {
    fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
            fallback_safe: true,
        }
    }

    fn io(context: impl fmt::Display, source: io::Error) -> Self {
        Self {
            message: context.to_string(),
            source: Some(source),
            fallback_safe: true,
        }
    }

    fn after_cleanup(
        context: impl fmt::Display,
        source: io::Error,
        cleanup: io::Result<()>,
    ) -> Self {
        let fallback_safe = cleanup.is_ok();
        Self {
            message: context.to_string(),
            source: Some(with_cleanup_error(source, cleanup)),
            fallback_safe,
        }
    }

    fn unresolved_residue(context: impl fmt::Display, source: io::Error) -> Self {
        Self {
            message: context.to_string(),
            source: Some(source),
            fallback_safe: false,
        }
    }
}

impl fmt::Display for TrashError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TrashError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|source| source as _)
    }
}

impl TrashBackend {
    pub fn from_environment() -> Result<Self, TrashError> {
        let data_home = match env::var_os("XDG_DATA_HOME") {
            Some(path) if !path.is_empty() && Path::new(&path).is_absolute() => PathBuf::from(path),
            _ => {
                let home = env::var_os("HOME").ok_or_else(|| {
                    TrashError::message("cannot locate desktop Trash because HOME is not set")
                })?;
                let home = PathBuf::from(home);
                if !home.is_absolute() {
                    return Err(TrashError::message(
                        "cannot locate desktop Trash because HOME is not an absolute path",
                    ));
                }
                home.join(".local/share")
            }
        };
        let uid = effective_uid();
        Ok(Self { data_home, uid })
    }

    #[must_use]
    #[cfg(test)]
    pub fn with_data_home(data_home: PathBuf, uid: u32) -> Self {
        Self { data_home, uid }
    }

    #[must_use]
    pub fn home_trash(&self) -> PathBuf {
        self.data_home.join("Trash")
    }

    #[cfg(test)]
    pub fn trash(&self, source: &Path) -> Result<TrashReceipt, TrashError> {
        let metadata = fs::symlink_metadata(source).map_err(|error| {
            TrashError::io(
                format!("cannot inspect {}", quote(source.as_os_str())),
                error,
            )
        })?;
        self.trash_verified(source, SourceIdentity::from_metadata(&metadata))
    }

    #[cfg(test)]
    pub fn trash_verified(
        &self,
        source: &Path,
        expected: SourceIdentity,
    ) -> Result<TrashReceipt, TrashError> {
        let directory_size = if expected.is_directory() {
            scan_tree(source, |_| {})
                .ok()
                .filter(crate::scanner::ScanSummary::complete)
                .map(|summary| summary.bytes)
        } else {
            None
        };
        self.trash_verified_with_directory_size(source, expected, directory_size)
    }

    #[cfg(test)]
    pub fn trash_verified_with_directory_size(
        &self,
        source: &Path,
        expected: SourceIdentity,
        directory_size: Option<u64>,
    ) -> Result<TrashReceipt, TrashError> {
        self.trash_verified_internal(source, expected, directory_size, false)
    }

    pub(crate) fn trash_verified_deferred_cache(
        &self,
        source: &Path,
        expected: SourceIdentity,
        directory_size: Option<u64>,
    ) -> Result<TrashReceipt, TrashError> {
        self.trash_verified_internal(source, expected, directory_size, true)
    }

    fn trash_verified_internal(
        &self,
        source: &Path,
        expected: SourceIdentity,
        directory_size: Option<u64>,
        defer_cache: bool,
    ) -> Result<TrashReceipt, TrashError> {
        if !source.is_absolute() {
            return Err(TrashError::message(
                "trash backend requires an absolute source path",
            ));
        }
        let source = normalize_source_path(source)?;
        let source = source.as_path();
        let location = self.trash_location(source)?;
        let basename = source
            .file_name()
            .ok_or_else(|| TrashError::message("path has no file name"))?;
        let source_parent = source
            .parent()
            .ok_or_else(|| TrashError::message("path has no parent directory"))?;
        let source_directory = PinnedDirectory::open(source_parent)?;
        let current = source_directory.metadata(basename).map_err(|error| {
            TrashError::io(
                format!(
                    "cannot revalidate {} before trashing",
                    quote(source.as_os_str())
                ),
                error,
            )
        })?;
        if !expected.matches(&current) {
            return Err(TrashError::message(
                "operand identity changed after validation; refusing to move it",
            ));
        }

        let primary = self.trash_to_root(
            source,
            &source_directory,
            basename,
            expected,
            &location.primary,
            &location.info_path,
            directory_size,
            defer_cache,
            &location.related_roots,
        );
        let primary_error = match primary {
            Ok(receipt) => return Ok(receipt),
            Err(error) => error,
        };
        let Some(fallback) = location.fallback else {
            return Err(primary_error);
        };
        if !primary_error.fallback_safe {
            return Err(primary_error);
        }
        if !source_directory
            .metadata(basename)
            .is_ok_and(|metadata| expected.matches(&metadata))
        {
            return Err(primary_error);
        }
        self.trash_to_root(
            source,
            &source_directory,
            basename,
            expected,
            &fallback,
            &location.info_path,
            directory_size,
            defer_cache,
            &location.related_roots,
        )
        .map_err(|fallback_error| {
            TrashError::message(format!(
                "cannot use the shared Trash ({primary_error}); the per-user fallback also failed ({fallback_error})"
            ))
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn trash_to_root(
        &self,
        source: &Path,
        source_directory: &PinnedDirectory,
        basename: &OsStr,
        expected: SourceIdentity,
        trash_root: &Path,
        info_path: &Path,
        directory_size: Option<u64>,
        defer_cache: bool,
        related_roots: &[PathBuf],
    ) -> Result<TrashReceipt, TrashError> {
        for related_root in related_roots {
            validate_resolved_trash_relation(source, related_root)?;
        }
        ensure_trash_layout(trash_root, self.uid)?;
        for related_root in related_roots {
            validate_resolved_trash_relation(source, related_root)?;
        }
        let root_directory = PinnedDirectory::open_private(trash_root, self.uid)?;
        let files_directory = root_directory.open_private_child(
            OsStr::new("files"),
            &trash_root.join("files"),
            self.uid,
        )?;
        let info_directory = root_directory.open_private_child(
            OsStr::new("info"),
            &trash_root.join("info"),
            self.uid,
        )?;
        let payload_name_max = files_directory.name_max().min(
            info_directory
                .name_max()
                .saturating_sub(TRASH_INFO_SUFFIX.len()),
        );
        if payload_name_max == 0 {
            return Err(TrashError::message(
                "the Trash filesystem component-name limit is too small for .trashinfo files",
            ));
        }
        move_verified(&MoveRequest {
            source,
            source_directory,
            basename,
            expected,
            trash_root,
            info_path,
            directory_size,
            defer_cache,
            payload_name_max,
            root_directory: &root_directory,
            files_directory: &files_directory,
            info_directory: &info_directory,
        })
    }

    fn trash_location(&self, source: &Path) -> Result<TrashLocation, TrashError> {
        let mounts = mounted_paths().ok();
        let source_mount = mount_point_for(source, mounts.as_deref())?;
        let home_mount = mount_point_for(&existing_ancestor(&self.data_home)?, mounts.as_deref())?;
        let source_mount_device = fs::metadata(&source_mount)
            .map_err(|error| TrashError::io("cannot inspect source mount", error))?
            .dev();
        let home_mount_device = fs::metadata(&home_mount)
            .map_err(|error| TrashError::io("cannot inspect home Trash mount", error))?
            .dev();

        if source_mount != home_mount && source_mount_device == home_mount_device {
            return Err(TrashError::message(
                "the source is reached through a separate mount of the home filesystem; desktop environments use the home Trash for this device, but an atomic move cannot cross the mount boundary and copy fallback is not implemented",
            ));
        }

        let location = if source_mount == home_mount {
            TrashLocation {
                primary: self.home_trash(),
                fallback: None,
                info_path: source.to_path_buf(),
                related_roots: vec![self.home_trash()],
            }
        } else {
            if source == source_mount {
                return Err(TrashError::message(
                    "refusing to trash a filesystem mount point",
                ));
            }
            let relative = source.strip_prefix(&source_mount).map_err(|_| {
                TrashError::message("cannot express the original path relative to its mount point")
            })?;
            let (primary, fallback) = self.volume_trash(&source_mount);
            let shared = source_mount.join(".Trash").join(self.uid.to_string());
            let per_user = source_mount.join(format!(".Trash-{}", self.uid));
            TrashLocation {
                primary,
                fallback,
                info_path: relative.to_path_buf(),
                related_roots: vec![shared, per_user],
            }
        };

        validate_trash_relation(source, &location.primary)?;
        Ok(location)
    }

    pub fn contains_nested_mount(path: &Path) -> Result<bool, TrashError> {
        let mounts = mounted_paths().map_err(|error| {
            TrashError::io(
                "cannot inspect mounted filesystems for --one-file-system",
                error,
            )
        })?;
        Ok(mounts
            .iter()
            .any(|mount| mount != path && mount.starts_with(path)))
    }

    pub(crate) fn apply_directory_size_updates(
        &self,
        updates: Vec<DirectorySizeUpdate>,
    ) -> Vec<String> {
        let mut groups: HashMap<(PathBuf, SourceIdentity), Vec<DirectorySizeUpdate>> =
            HashMap::new();
        for update in updates {
            groups
                .entry((update.root.clone(), update.root_identity))
                .or_default()
                .push(update);
        }
        let mut warnings = Vec::new();
        for ((root, expected), updates) in groups {
            let result = PinnedDirectory::open_private(&root, self.uid).and_then(|directory| {
                directory.revalidate_path(&root).map_err(|error| {
                    TrashError::io("cannot revalidate Trash cache directory", error)
                })?;
                if directory.identity().map_err(|error| {
                    TrashError::io("cannot inspect Trash cache directory", error)
                })? != expected
                {
                    return Err(TrashError::message(
                        "Trash cache directory identity changed",
                    ));
                }
                update_directory_sizes_batch(&directory, &updates)
                    .map_err(|error| TrashError::io("cannot update directory size cache", error))
            });
            if let Err(error) = result {
                warnings.push(format!(
                    "items were trashed, but the directory size cache for {} could not be updated: {error}",
                    quote(root.as_os_str())
                ));
            }
        }
        warnings
    }

    fn volume_trash(&self, mount_point: &Path) -> (PathBuf, Option<PathBuf>) {
        let shared = mount_point.join(".Trash");
        let fallback = mount_point.join(format!(".Trash-{}", self.uid));
        if let Ok(metadata) = fs::symlink_metadata(&shared) {
            if trusted_shared_trash_metadata(&metadata, self.uid) {
                return (shared.join(self.uid.to_string()), Some(fallback));
            }
        }
        (fallback, None)
    }
}

pub(crate) fn trusted_shared_trash_metadata(metadata: &fs::Metadata, uid: u32) -> bool {
    trusted_shared_trash_attributes(
        metadata.file_type().is_dir(),
        metadata.permissions().mode(),
        metadata.uid(),
        uid,
    )
}

fn trusted_shared_trash_attributes(is_directory: bool, mode: u32, owner: u32, uid: u32) -> bool {
    is_directory && mode & 0o1000 != 0 && (owner == 0 || owner == uid)
}

pub fn remove_empty_directory_verified(
    source: &Path,
    expected: SourceIdentity,
) -> Result<(), TrashError> {
    let basename = source
        .file_name()
        .ok_or_else(|| TrashError::message("path has no file name"))?;
    let parent = source
        .parent()
        .ok_or_else(|| TrashError::message("path has no parent directory"))?;
    let directory = PinnedDirectory::open(parent)?;
    let current = directory.metadata(basename).map_err(|error| {
        TrashError::io(
            format!(
                "cannot revalidate {} before removing it",
                quote(source.as_os_str())
            ),
            error,
        )
    })?;
    if !expected.matches(&current) {
        return Err(TrashError::message(
            "operand identity changed after validation; refusing to remove it",
        ));
    }
    #[cfg(test)]
    SWAP_EMPTY_DIRECTORY_AFTER_REVALIDATION.with(|swap| {
        if swap.replace(false) {
            let parked = parent.join(".rmu-test-original-empty");
            fs::rename(source, parked).unwrap();
            fs::create_dir(source).unwrap();
        }
    });
    directory
        .remove_directory_if_matches(basename, expected)
        .map_err(|error| TrashError::io("the kernel refused the empty-directory removal", error))
}

fn normalize_source_path(source: &Path) -> Result<PathBuf, TrashError> {
    let basename = source
        .file_name()
        .ok_or_else(|| TrashError::message("path has no file name"))?;
    if matches!(basename.as_bytes(), b"." | b"..") {
        return Err(TrashError::message(
            "refusing a source path ending in '.' or '..'",
        ));
    }
    let parent = source
        .parent()
        .ok_or_else(|| TrashError::message("path has no parent directory"))?;
    let parent = parent.canonicalize().map_err(|error| {
        TrashError::io(
            format!("cannot resolve source parent {}", quote(parent.as_os_str())),
            error,
        )
    })?;
    Ok(parent.join(basename))
}

struct TrashLocation {
    primary: PathBuf,
    fallback: Option<PathBuf>,
    info_path: PathBuf,
    related_roots: Vec<PathBuf>,
}

fn validate_trash_relation(source: &Path, trash_root: &Path) -> Result<(), TrashError> {
    if source.starts_with(trash_root) {
        return Err(TrashError::message(
            "the file is already inside the desktop Trash",
        ));
    }
    if trash_root.starts_with(source) {
        return Err(TrashError::message(
            "the Trash directory is inside the requested operand",
        ));
    }
    Ok(())
}

fn validate_resolved_trash_relation(source: &Path, trash_root: &Path) -> Result<(), TrashError> {
    let resolved_root = resolve_allowing_missing_tail(trash_root).map_err(|error| {
        TrashError::io(
            format!(
                "cannot resolve Trash root {}",
                quote(trash_root.as_os_str())
            ),
            error,
        )
    })?;
    let parent = source
        .parent()
        .ok_or_else(|| TrashError::message("path has no parent directory"))?;
    let basename = source
        .file_name()
        .ok_or_else(|| TrashError::message("path has no file name"))?;
    let resolved_source = parent
        .canonicalize()
        .map_err(|error| {
            TrashError::io(
                format!("cannot resolve source parent {}", quote(parent.as_os_str())),
                error,
            )
        })?
        .join(basename);
    validate_trash_relation(&resolved_source, &resolved_root)
}

fn resolve_allowing_missing_tail(path: &Path) -> io::Result<PathBuf> {
    let mut candidate = path;
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
                let Some(name) = candidate.file_name() else {
                    return Err(error);
                };
                missing.push(name.to_os_string());
                let Some(parent) = candidate.parent() else {
                    return Err(error);
                };
                candidate = parent;
            }
            Err(error) => return Err(error),
        }
    }
}

struct MoveRequest<'a> {
    source: &'a Path,
    source_directory: &'a PinnedDirectory,
    basename: &'a OsStr,
    expected: SourceIdentity,
    trash_root: &'a Path,
    info_path: &'a Path,
    directory_size: Option<u64>,
    defer_cache: bool,
    payload_name_max: usize,
    root_directory: &'a PinnedDirectory,
    files_directory: &'a PinnedDirectory,
    info_directory: &'a PinnedDirectory,
}

impl MoveRequest<'_> {
    fn revalidate_paths(&self) -> Result<(), TrashError> {
        let source_parent = self
            .source
            .parent()
            .ok_or_else(|| TrashError::message("source path has no parent directory"))?;
        let files_path = self.trash_root.join("files");
        let info_path = self.trash_root.join("info");
        for (directory, path, label) in [
            (self.source_directory, source_parent, "source parent"),
            (self.root_directory, self.trash_root, "Trash root"),
            (
                self.files_directory,
                files_path.as_path(),
                "Trash files directory",
            ),
            (
                self.info_directory,
                info_path.as_path(),
                "Trash info directory",
            ),
        ] {
            directory.revalidate_path(path).map_err(|error| {
                TrashError::io(
                    format!("cannot revalidate {label} before publication"),
                    error,
                )
            })?;
        }
        Ok(())
    }

    fn revalidate_source_parent(&self) -> io::Result<()> {
        let parent = self
            .source
            .parent()
            .ok_or_else(|| io::Error::other("source path has no parent directory"))?;
        self.source_directory.revalidate_path(parent)
    }
}

#[allow(clippy::too_many_lines)]
fn move_verified(request: &MoveRequest<'_>) -> Result<TrashReceipt, TrashError> {
    let info_contents = trash_info(request.info_path);
    for attempt in 0_u64..1024 {
        let suffix = if attempt == 0 {
            0
        } else {
            collision_suffix(attempt)
        };
        let unique_name = suffixed_name(request.basename, suffix, request.payload_name_max);
        let payload = request.trash_root.join("files").join(&unique_name);
        if request.files_directory.metadata(&unique_name).is_ok() {
            continue;
        }
        let unique_info_name = info_name(&unique_name);
        let info = request.trash_root.join("info").join(&unique_info_name);
        let (mut info_file, info_identity) =
            match create_info_file_at(request.info_directory, &unique_info_name) {
                Ok(created) => created,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(TrashError::io(
                        format!("cannot create {}", quote(info.as_os_str())),
                        error,
                    ));
                }
            };
        if let Err(error) = write_and_sync(&mut info_file, info_contents.as_bytes()) {
            drop(info_file);
            return Err(TrashError::after_cleanup(
                format!("cannot write {}", quote(info.as_os_str())),
                error,
                cleanup_reserved_info(request, &unique_info_name, info_identity),
            ));
        }
        let info_metadata = info_file.metadata().ok();
        let info_mtime = request
            .directory_size
            .and_then(|_| info_metadata.as_ref().map(MetadataExt::mtime));
        if let Err(error) = request.revalidate_paths() {
            drop(info_file);
            return Err(TrashError::after_cleanup(
                "a directory namespace changed before the Trash move",
                io::Error::other(error.to_string()),
                cleanup_reserved_info(request, &unique_info_name, info_identity),
            ));
        }
        match move_payload(request, &unique_name) {
            Ok(()) => {
                return verify_moved_item(
                    request,
                    &unique_name,
                    &unique_info_name,
                    &info_file,
                    info_identity,
                    info_contents.as_bytes(),
                    info_mtime,
                    payload,
                    info,
                );
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if let Err(cleanup_error) =
                    cleanup_reserved_info(request, &unique_info_name, info_identity)
                {
                    return Err(TrashError::unresolved_residue(
                        format!(
                            "cannot clean reserved metadata {} after a Trash payload name collision",
                            quote(info.as_os_str())
                        ),
                        cleanup_error,
                    ));
                }
            }
            Err(error) => {
                return Err(TrashError::after_cleanup(
                    format!(
                        "cannot move {} into the desktop Trash",
                        quote(request.source.as_os_str())
                    ),
                    error,
                    cleanup_reserved_info(request, &unique_info_name, info_identity),
                ));
            }
        }
    }
    Err(TrashError::message(
        "could not reserve a unique Trash payload name after 1024 attempts",
    ))
}

fn collision_suffix(attempt: u64) -> u64 {
    let clock = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_secs().rotate_left(32) ^ u64::from(duration.subsec_nanos())
        });
    let sequence = NEXT_COLLISION_SUFFIX.fetch_add(1, Ordering::Relaxed);
    (clock ^ u64::from(std::process::id()).rotate_left(32) ^ sequence ^ attempt.rotate_left(17)) | 1
}

fn move_payload(request: &MoveRequest<'_>, unique_name: &OsStr) -> io::Result<()> {
    #[cfg(test)]
    if let Some(kind) = FORCE_PAYLOAD_MOVE_FAILURE.with(Cell::take) {
        return Err(io::Error::from(kind));
    }
    rename_no_replace_at(
        request.source_directory,
        request.basename,
        request.files_directory,
        unique_name,
    )
}

fn cleanup_reserved_info(
    request: &MoveRequest<'_>,
    name: &OsStr,
    identity: SourceIdentity,
) -> io::Result<()> {
    #[cfg(test)]
    if FORCE_INFO_CLEANUP_FAILURE.with(Cell::get) {
        return Err(io::Error::other(
            "injected reserved metadata cleanup failure",
        ));
    }
    request
        .info_directory
        .clean_up_file_if_matches(name, identity)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn verify_moved_item(
    request: &MoveRequest<'_>,
    unique_name: &OsStr,
    unique_info_name: &OsStr,
    info_file: &File,
    info_identity: SourceIdentity,
    info_contents: &[u8],
    info_mtime: Option<i64>,
    payload: PathBuf,
    info: PathBuf,
) -> Result<TrashReceipt, TrashError> {
    #[cfg(test)]
    MOVE_TRASH_ROOT_AFTER_PAYLOAD.with(|move_root| {
        if move_root.replace(false) {
            let parked = request
                .trash_root
                .parent()
                .expect("Trash root has a parent")
                .join(".rmu-test-moved-trash-root");
            fs::rename(request.trash_root, &parked).unwrap();
            fs::create_dir(request.trash_root).unwrap();
        }
    });

    if let Err(error) = request.revalidate_paths() {
        return Err(rollback_after_namespace_change(
            request,
            unique_name,
            unique_info_name,
            info_identity,
            &error,
        ));
    }

    #[cfg(test)]
    REMOVE_INFO_AFTER_MOVE.with(|remove| {
        if remove.replace(false) {
            let _ = request.info_directory.remove_file(unique_info_name);
        }
    });
    #[cfg(test)]
    MUTATE_INFO_AFTER_MOVE.with(|mutate| {
        if mutate.replace(false) {
            let _ = info_file.write_all_at(b"X", 0);
        }
    });
    #[cfg(test)]
    REMOVE_PAYLOAD_AFTER_MOVE.with(|remove| {
        if remove.replace(false) {
            let _ = request.files_directory.remove_file(unique_name);
        }
    });

    let moved = request.files_directory.metadata(unique_name);
    if moved
        .as_ref()
        .is_ok_and(|metadata| request.expected.matches(metadata))
    {
        if !info_entry_is_intact(
            request,
            unique_info_name,
            info_file,
            info_identity,
            info_contents,
        ) {
            return fail_after_integrity_failure(
                request,
                unique_name,
                &payload,
                "the .trashinfo entry vanished or changed during the move",
            );
        }
        let mut warnings = Vec::new();
        let mut cache_update = None;
        if let Some(directory_size) = request.directory_size {
            if let Some(info_mtime) = info_mtime {
                if request.defer_cache {
                    match request.root_directory.identity() {
                        Ok(root_identity) => {
                            cache_update = Some(DirectorySizeUpdate {
                                root: request.trash_root.to_path_buf(),
                                root_identity,
                                name: unique_name.to_os_string(),
                                size: directory_size,
                                info_mtime,
                            });
                        }
                        Err(error) => warnings.push(format!(
                            "the item was trashed, but its directory size cache update could not be staged: {error}"
                        )),
                    }
                } else if let Err(error) = update_directory_sizes(
                    request.root_directory,
                    unique_name,
                    directory_size,
                    info_mtime,
                ) {
                    warnings.push(format!(
                        "the item was trashed, but the directory size cache could not be updated: {error}"
                    ));
                }
            } else {
                warnings.push(
                    "the item was trashed, but the trashinfo timestamp could not be read for the directory size cache"
                        .into(),
                );
            }
        }
        if !info_entry_is_intact(
            request,
            unique_info_name,
            info_file,
            info_identity,
            info_contents,
        ) {
            return fail_after_integrity_failure(
                request,
                unique_name,
                &payload,
                "the .trashinfo entry vanished or changed during the move",
            );
        }
        #[cfg(test)]
        REMOVE_PAYLOAD_AFTER_CACHE.with(|remove| {
            if remove.replace(false) {
                let _ = request.files_directory.remove_file(unique_name);
            }
        });
        if !request
            .files_directory
            .metadata(unique_name)
            .is_ok_and(|metadata| request.expected.matches(&metadata))
        {
            return Err(TrashError::message(format!(
                "the Trash payload vanished or changed after the move; a concurrent Trash operation may have removed it from {}",
                quote(payload.as_os_str())
            )));
        }
        if let Err(error) = request.revalidate_paths() {
            return Err(rollback_after_namespace_change(
                request,
                unique_name,
                unique_info_name,
                info_identity,
                &error,
            ));
        }
        return Ok(TrashReceipt {
            payload,
            info,
            warnings,
            cache_update,
        });
    }

    let recovery = fsops::quarantine_recovery_state(
        request.files_directory,
        unique_name,
        Some(request.expected),
    );
    Err(TrashError::message(format!(
        "operand identity could not be verified after the move; no rollback was attempted because the Trash pathname is untrusted; payload recovery state: {recovery}"
    )))
}

fn rollback_after_namespace_change(
    request: &MoveRequest<'_>,
    unique_name: &OsStr,
    unique_info_name: &OsStr,
    info_identity: SourceIdentity,
    namespace_error: &TrashError,
) -> TrashError {
    let rollback = request
        .revalidate_source_parent()
        .and_then(|()| {
            rename_no_replace_at(
                request.files_directory,
                unique_name,
                request.source_directory,
                request.basename,
            )
        })
        .and_then(|()| {
            let restored = request.source_directory.metadata(request.basename)?;
            if request.expected.matches(&restored) {
                Ok(())
            } else {
                Err(io::Error::other("restored source identity changed"))
            }
        })
        .and_then(|()| cleanup_reserved_info(request, unique_info_name, info_identity));

    match rollback {
        Ok(()) => TrashError::io(
            "a directory namespace changed during the Trash move; the source was restored",
            io::Error::other(namespace_error.to_string()),
        ),
        Err(rollback_error) => TrashError::unresolved_residue(
            "a directory namespace changed during the Trash move and the source could not be safely restored",
            with_cleanup_error(
                io::Error::other(namespace_error.to_string()),
                Err(rollback_error),
            ),
        ),
    }
}

fn info_entry_is_intact(
    request: &MoveRequest<'_>,
    unique_info_name: &OsStr,
    info_file: &File,
    expected: SourceIdentity,
    expected_contents: &[u8],
) -> bool {
    let expected_length = u64::try_from(expected_contents.len()).unwrap_or(u64::MAX);
    let handle = info_file.metadata();
    let named = request.info_directory.metadata(unique_info_name);
    let metadata_matches = handle.as_ref().is_ok_and(|metadata| {
        expected.matches(metadata) && metadata.nlink() > 0 && metadata.len() == expected_length
    }) && named
        .as_ref()
        .is_ok_and(|metadata| expected.matches(metadata) && metadata.len() == expected_length);
    if !metadata_matches {
        return false;
    }
    let mut actual = vec![0; expected_contents.len()];
    info_file.read_exact_at(&mut actual, 0).is_ok() && actual == expected_contents
}

fn fail_after_integrity_failure(
    request: &MoveRequest<'_>,
    unique_name: &OsStr,
    payload: &Path,
    message: &str,
) -> Result<TrashReceipt, TrashError> {
    let recovery = fsops::quarantine_recovery_state(
        request.files_directory,
        unique_name,
        Some(request.expected),
    );
    Err(TrashError::message(format!(
        "{message}; no rollback was attempted because the Trash pathname {} is untrusted; payload recovery state: {recovery}",
        quote(payload.as_os_str())
    )))
}

fn update_directory_sizes(
    root_directory: &PinnedDirectory,
    directory_name: &OsStr,
    size: u64,
    info_mtime: i64,
) -> io::Result<()> {
    let existing = read_cache_file(root_directory)?;
    let encoded_name = percent_encode(directory_name.as_bytes());
    let contents = rewritten_cache(&existing, encoded_name.as_bytes(), size, info_mtime)?;
    replace_cache_file(root_directory, &contents)
}

fn update_directory_sizes_batch(
    root_directory: &PinnedDirectory,
    updates: &[DirectorySizeUpdate],
) -> io::Result<()> {
    let existing = read_cache_file(root_directory)?;
    let encoded: Vec<_> = updates
        .iter()
        .map(|update| percent_encode(update.name.as_bytes()).into_bytes())
        .collect();
    let names: HashSet<_> = encoded.iter().map(Vec::as_slice).collect();
    let mut contents = Vec::with_capacity(existing.len());
    for line in existing.split_inclusive(|byte| *byte == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        if cache_line_name(body).is_none_or(|name| !names.contains(name)) {
            contents.extend_from_slice(line);
        }
    }
    if !contents.is_empty() && !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    for (update, name) in updates.iter().zip(encoded) {
        write!(&mut contents, "{} {} ", update.size, update.info_mtime)
            .expect("writing to a byte vector cannot fail");
        contents.extend_from_slice(&name);
        contents.push(b'\n');
        ensure_serialized(&contents)?;
    }
    replace_cache_file(root_directory, &contents)
}

fn rewritten_cache(
    existing: &[u8],
    encoded_name: &[u8],
    size: u64,
    info_mtime: i64,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::with_capacity(existing.len().saturating_add(encoded_name.len() + 48));
    for line in existing.split_inclusive(|byte| *byte == b'\n') {
        let body = line.strip_suffix(b"\n").unwrap_or(line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        if cache_line_name(body) != Some(encoded_name) {
            output.extend_from_slice(line);
        }
    }
    if !output.is_empty() && !output.ends_with(b"\n") {
        output.push(b'\n');
    }
    write!(&mut output, "{size} {info_mtime} ").expect("writing to a byte vector cannot fail");
    output.extend_from_slice(encoded_name);
    output.push(b'\n');
    ensure_serialized(&output)?;
    Ok(output)
}

fn cache_line_name(line: &[u8]) -> Option<&[u8]> {
    let first_space = line.iter().position(|byte| *byte == b' ')?;
    let remainder = &line[first_space + 1..];
    let second_space = remainder.iter().position(|byte| *byte == b' ')?;
    Some(&remainder[second_space + 1..])
}

fn existing_ancestor(path: &Path) -> Result<PathBuf, TrashError> {
    let mut candidate = path;
    loop {
        if fs::metadata(candidate).is_ok() {
            return candidate.canonicalize().map_err(|error| {
                TrashError::io(
                    format!("cannot resolve {}", quote(candidate.as_os_str())),
                    error,
                )
            });
        }
        candidate = candidate.parent().ok_or_else(|| {
            TrashError::message(format!(
                "cannot find an existing ancestor of {}",
                quote(path.as_os_str())
            ))
        })?;
    }
}

fn mount_point_for(path: &Path, mounts: Option<&[PathBuf]>) -> Result<PathBuf, TrashError> {
    if let Some(mounts) = mounts {
        let mut best: Option<PathBuf> = None;
        for mount in mounts {
            if path.starts_with(mount)
                && best
                    .as_ref()
                    .is_none_or(|current| mount.as_os_str().len() > current.as_os_str().len())
            {
                best = Some(mount.clone());
            }
        }
        if let Some(best) = best {
            return Ok(best);
        }
    }

    let mut candidate = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let device = fs::metadata(candidate)
        .map_err(|error| {
            TrashError::io(
                format!("cannot inspect {}", quote(candidate.as_os_str())),
                error,
            )
        })?
        .dev();
    while let Some(parent) = candidate.parent() {
        let parent_device = match fs::metadata(parent) {
            Ok(metadata) => metadata.dev(),
            Err(_) => break,
        };
        if parent_device != device {
            break;
        }
        candidate = parent;
    }
    Ok(candidate.to_path_buf())
}

pub(crate) struct MountPaths {
    pub(crate) all: Vec<PathBuf>,
    pub(crate) trash_candidates: Vec<PathBuf>,
}

pub(crate) fn mount_paths() -> io::Result<MountPaths> {
    let contents = fs::read("/proc/self/mountinfo")?;
    Ok(parse_mount_paths(&contents))
}

pub(crate) fn mounted_paths() -> io::Result<Vec<PathBuf>> {
    mount_paths().map(|paths| paths.all)
}

fn parse_mount_paths(contents: &[u8]) -> MountPaths {
    let mut paths = MountPaths {
        all: Vec::new(),
        trash_candidates: Vec::new(),
    };
    for line in contents.split(|byte| *byte == b'\n') {
        let mut fields = line.split(|byte| *byte == b' ');
        let Some(mount_field) = fields.nth(4) else {
            continue;
        };
        let Some(filesystem_type) = fields
            .by_ref()
            .find(|field| *field == b"-")
            .and_then(|_| fields.next())
        else {
            continue;
        };
        let path = PathBuf::from(OsString::from_vec(unescape_mount_field(mount_field)));
        paths.all.push(path.clone());
        if !is_kernel_interface_filesystem(filesystem_type) {
            paths.trash_candidates.push(path);
        }
    }
    paths
}

fn is_kernel_interface_filesystem(filesystem_type: &[u8]) -> bool {
    matches!(
        filesystem_type,
        b"bpf"
            | b"cgroup"
            | b"cgroup2"
            | b"configfs"
            | b"debugfs"
            | b"devpts"
            | b"devtmpfs"
            | b"efivarfs"
            | b"fusectl"
            | b"hugetlbfs"
            | b"mqueue"
            | b"proc"
            | b"pstore"
            | b"securityfs"
            | b"sysfs"
            | b"tracefs"
    )
}

fn unescape_mount_field(field: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(field.len());
    let mut index = 0;
    while index < field.len() {
        if field[index] == b'\\'
            && index + 3 < field.len()
            && field[index + 1..=index + 3]
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'7'))
        {
            let value = (field[index + 1] - b'0') * 64
                + (field[index + 2] - b'0') * 8
                + (field[index + 3] - b'0');
            output.push(value);
            index += 4;
        } else {
            output.push(field[index]);
            index += 1;
        }
    }
    output
}

fn ensure_trash_layout(root: &Path, uid: u32) -> Result<(), TrashError> {
    if let Some(parent) = root.parent() {
        create_private_parents(parent).map_err(|error| {
            TrashError::io(
                format!("cannot create trash parent {}", quote(parent.as_os_str())),
                error,
            )
        })?;
    }
    ensure_private_directory(root, uid)?;
    ensure_private_directory(&root.join("files"), uid)?;
    ensure_private_directory(&root.join("info"), uid)?;
    Ok(())
}

fn create_private_parents(path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a parent component is not a directory",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("directory has no parent"))?;
    create_private_parents(parent)?;
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => fs::set_permissions(path, fs::Permissions::from_mode(0o700)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

fn ensure_private_directory(path: &Path, uid: u32) -> Result<(), TrashError> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(TrashError::io(
                format!("cannot create {}", quote(path.as_os_str())),
                error,
            ));
        }
    }
    PinnedDirectory::open_secure_private(path, uid).map(|_| ())
}

pub(crate) fn validate_private_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    uid: u32,
) -> Result<(), TrashError> {
    validate_owner_controlled_metadata(path, metadata, uid)
}

pub(crate) fn validate_confidential_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    uid: u32,
) -> Result<(), TrashError> {
    validate_owner_controlled_metadata(path, metadata, uid)?;
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(TrashError::message(format!(
            "{} is accessible by other users",
            quote(path.as_os_str())
        )));
    }
    Ok(())
}

pub(crate) fn validate_owner_controlled_metadata(
    path: &Path,
    metadata: &fs::Metadata,
    uid: u32,
) -> Result<(), TrashError> {
    if !metadata.file_type().is_dir() {
        return Err(TrashError::message(format!(
            "{} is not a real directory",
            quote(path.as_os_str())
        )));
    }
    if metadata.uid() != uid {
        return Err(TrashError::message(format!(
            "{} is not owned by the current user",
            quote(path.as_os_str())
        )));
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(TrashError::message(format!(
            "{} is writable by other users",
            quote(path.as_os_str())
        )));
    }
    Ok(())
}

fn suffixed_name(basename: &OsStr, suffix: u64, maximum_length: usize) -> OsString {
    let suffix = if suffix == 0 {
        Vec::new()
    } else {
        format!(".{suffix}").into_bytes()
    };
    let basename_length = maximum_length.saturating_sub(suffix.len());
    let mut bytes = basename.as_bytes()[..basename.as_bytes().len().min(basename_length)].to_vec();
    bytes.extend_from_slice(&suffix);
    OsString::from_vec(bytes)
}

pub(crate) fn info_name(payload_name: &OsStr) -> OsString {
    let mut bytes = payload_name.as_bytes().to_vec();
    bytes.extend_from_slice(TRASH_INFO_SUFFIX);
    OsString::from_vec(bytes)
}

fn trash_info(original_path: &Path) -> String {
    format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        percent_encode(original_path.as_os_str().as_bytes()),
        deletion_timestamp()
    )
}

pub(crate) fn percent_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[usize::from(byte >> 4)]));
            output.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
    }
    output
}

fn deletion_timestamp() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Some(local) = local_calendar_time(seconds) {
        return format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            local.year, local.month, local.day, local.hour, local.minute, local.second
        );
    }
    utc_timestamp(seconds)
}

fn utc_timestamp(seconds: u64) -> String {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let seconds_today = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_today / 3_600;
    let minute = seconds_today % 3_600 / 60;
    let second = seconds_today % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_preserves_path_separators_and_encodes_controls() {
        assert_eq!(percent_encode(b"/tmp/a b\n%"), "/tmp/a%20b%0A%25");
    }

    #[test]
    fn mount_field_unescapes_octal_sequences() {
        assert_eq!(
            unescape_mount_field(b"/media/My\\040Disk"),
            b"/media/My Disk"
        );
    }

    #[test]
    fn trash_discovery_keeps_read_only_data_mounts_and_excludes_kernel_interfaces() {
        let mountinfo = b"1 0 8:1 / /media/read-only ro,nosuid - ext4 /dev/a ro\n\
                          2 0 8:2 / /media/write\\040here rw,nosuid - ext4 /dev/b rw\n\
                          3 0 0:3 / /sys/kernel/debug rw,nosuid - debugfs debugfs rw\n";
        let paths = parse_mount_paths(mountinfo);
        assert_eq!(
            paths.trash_candidates,
            [
                PathBuf::from("/media/read-only"),
                PathBuf::from("/media/write here")
            ]
        );
        assert_eq!(paths.all.len(), 3);
    }

    #[test]
    fn civil_date_conversion_matches_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_453), (2025, 12, 31));
    }

    #[test]
    fn cache_rewrite_preserves_other_lines_and_replaces_the_named_entry() {
        let rewritten = rewritten_cache(
            b"10 20 keep\n30 40 replace%20me\nmalformed\n",
            b"replace%20me",
            50,
            60,
        )
        .unwrap();
        assert_eq!(rewritten, b"10 20 keep\nmalformed\n50 60 replace%20me\n");
    }

    #[test]
    fn recursive_removal_never_deletes_a_swapped_replacement_directory() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("rmu-tree-swap-{}-{sequence}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let selected = root.join("selected");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("intended"), b"selected data").unwrap();
        let replacement = root.join(".rmu-test-replacement");
        fs::create_dir(&replacement).unwrap();
        fs::write(replacement.join("sentinel"), b"must survive").unwrap();
        let parent = PinnedDirectory::open(&root).unwrap();
        let identity = SourceIdentity::from_metadata(&fs::metadata(&selected).unwrap());
        SWAP_TREE_AFTER_PIN.with(|swap| swap.set(true));

        let result = remove_tree_if_matches(&parent, OsStr::new("selected"), identity);

        assert!(result.is_err());
        assert_eq!(
            fs::read(root.join("selected/sentinel")).unwrap(),
            b"must survive"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_directory_removal_never_deletes_a_post_validation_replacement() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("rmu-empty-swap-{}-{sequence}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let selected = root.join("selected");
        fs::create_dir(&selected).unwrap();
        let identity = SourceIdentity::from_metadata(&fs::metadata(&selected).unwrap());
        SWAP_EMPTY_DIRECTORY_AFTER_REVALIDATION.with(|swap| swap.set(true));

        let error = remove_empty_directory_verified(&selected, identity)
            .unwrap_err()
            .to_string();

        assert!(error.contains("identity changed"), "{error}");
        assert!(selected.is_dir());
        assert!(root.join(".rmu-test-original-empty").is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recursive_removal_refuses_a_mount_boundary_before_deleting_through_it() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-tree-mount-boundary-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let selected = root.join("selected");
        fs::create_dir(&selected).unwrap();
        fs::write(selected.join("sentinel"), b"must survive").unwrap();
        let parent = PinnedDirectory::open(&root).unwrap();
        let identity = SourceIdentity::from_metadata(&fs::metadata(&selected).unwrap());
        FORCE_CHILD_MOUNT_BOUNDARY.with(|force| force.set(true));

        let error = remove_tree_if_matches(&parent, OsStr::new("selected"), identity)
            .unwrap_err()
            .to_string();

        assert!(error.contains("mounted filesystem boundary"));
        assert_eq!(
            fs::read(selected.join("sentinel")).unwrap(),
            b"must survive"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn leaves_the_payload_for_recovery_when_reserved_info_vanishes() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("rmu-info-race-{}-{sequence}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"recoverable").unwrap();
        // SAFETY: geteuid has no preconditions and cannot fail on Linux.
        let backend = TrashBackend::with_data_home(root.join("data"), unsafe { geteuid() });
        REMOVE_INFO_AFTER_MOVE.with(|remove| remove.set(true));

        let error = backend.trash(&source).unwrap_err();

        assert!(error.to_string().contains(".trashinfo entry vanished"));
        assert!(error.to_string().contains("verified recoverable"));
        assert!(!source.exists());
        assert_eq!(
            fs::read_dir(backend.home_trash().join("files"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(backend.home_trash().join("info"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn leaves_the_pair_for_recovery_when_reserved_info_is_modified() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-info-mutation-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"recoverable").unwrap();
        // SAFETY: geteuid has no preconditions and cannot fail on Linux.
        let backend = TrashBackend::with_data_home(root.join("data"), unsafe { geteuid() });
        MUTATE_INFO_AFTER_MOVE.with(|mutate| mutate.set(true));

        let error = backend.trash(&source).unwrap_err();

        assert!(
            error
                .to_string()
                .contains(".trashinfo entry vanished or changed")
        );
        assert!(error.to_string().contains("verified recoverable"));
        assert!(!source.exists());
        assert_eq!(
            fs::read_dir(backend.home_trash().join("files"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(backend.home_trash().join("info"))
                .unwrap()
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn never_reports_success_when_the_payload_disappears_during_publication() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-payload-race-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"removed by a simulated Trash cleaner").unwrap();
        // SAFETY: geteuid has no preconditions and cannot fail on Linux.
        let backend = TrashBackend::with_data_home(root.join("data"), unsafe { geteuid() });
        REMOVE_PAYLOAD_AFTER_CACHE.with(|remove| remove.set(true));

        let error = backend.trash(&source).unwrap_err();

        assert!(error.to_string().contains("Trash payload vanished"));
        assert!(!source.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn moved_payload_failure_never_claims_an_unverified_recovery_location() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-payload-first-verify-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"removed by a simulated Trash cleaner").unwrap();
        let backend = TrashBackend::with_data_home(root.join("data"), effective_uid());
        REMOVE_PAYLOAD_AFTER_MOVE.with(|remove| remove.set(true));

        let error = backend.trash(&source).unwrap_err().to_string();

        assert!(error.contains("identity could not be verified after the move"));
        assert!(error.contains("recovery state is unknown"));
        assert!(!error.contains("verified recoverable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unverified_cleanup_restores_a_captured_replacement() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-unverified-cleanup-{}-{sequence}",
            std::process::id()
        ));
        DirBuilder::new().mode(0o700).create(&root).unwrap();
        let created_path = root.join("created-handle");
        let created = File::create(&created_path).unwrap();
        let replacement_name = OsStr::new("victim.trashinfo");
        fs::write(root.join(replacement_name), b"existing Trash metadata").unwrap();
        // SAFETY: geteuid has no preconditions and cannot fail on Linux.
        let directory = PinnedDirectory::open_private(&root, unsafe { geteuid() }).unwrap();

        let error = directory
            .clean_up_unverified_new_file(replacement_name, &created)
            .unwrap_err();

        assert!(error.to_string().contains("captured entry was restored"));
        assert_eq!(
            fs::read(root.join(replacement_name)).unwrap(),
            b"existing Trash metadata"
        );
        assert!(created_path.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_move_failure_reports_reserved_metadata_cleanup_failure() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-info-cleanup-failure-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"still recoverable").unwrap();
        let backend = TrashBackend::with_data_home(root.join("data"), effective_uid());
        FORCE_PAYLOAD_MOVE_FAILURE
            .with(|failure| failure.set(Some(io::ErrorKind::PermissionDenied)));
        FORCE_INFO_CLEANUP_FAILURE.with(|force| force.set(true));

        let error = backend.trash(&source).unwrap_err();

        FORCE_INFO_CLEANUP_FAILURE.with(|force| force.set(false));
        assert!(source.is_file());
        assert!(!error.fallback_safe);
        assert!(error.to_string().contains("safe cleanup also reported"));
        assert!(
            error
                .to_string()
                .contains("reserved metadata cleanup failure")
        );
        assert_eq!(
            fs::read_dir(backend.home_trash().join("info"))
                .unwrap()
                .count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_capture_not_found_is_reported_as_uncertain_cleanup() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-info-post-capture-cleanup-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"still recoverable").unwrap();
        let backend = TrashBackend::with_data_home(root.join("data"), effective_uid());
        FORCE_PAYLOAD_MOVE_FAILURE
            .with(|failure| failure.set(Some(io::ErrorKind::PermissionDenied)));
        REMOVE_CAPTURED_CLEANUP_BEFORE_UNLINK.with(|remove| remove.set(true));

        let error = backend.trash(&source).unwrap_err().to_string();

        assert!(source.is_file());
        assert!(error.contains("safe cleanup also reported"));
        assert!(error.contains("recovery state is unknown"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quarantine_recovery_is_reported_only_after_identity_revalidation() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-quarantine-state-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let quarantine = OsStr::new(".rmu-quarantine-123-456");
        let quarantined_path = root.join(quarantine);
        fs::write(&quarantined_path, b"captured").unwrap();
        let expected = SourceIdentity::from_metadata(&fs::metadata(&quarantined_path).unwrap());
        let directory = PinnedDirectory::open(&root).unwrap();

        let verified = fsops::quarantine_recovery_state(&directory, quarantine, Some(expected));
        assert!(verified.contains("verified recoverable"));

        fs::rename(&quarantined_path, root.join("moved-captured")).unwrap();
        fs::write(&quarantined_path, b"replacement").unwrap();
        let replaced = fsops::quarantine_recovery_state(&directory, quarantine, Some(expected));
        assert!(replaced.contains("recovery state is unknown"));
        assert!(!replaced.contains("verified recoverable"));

        fs::remove_file(&quarantined_path).unwrap();
        let absent = fsops::quarantine_recovery_state(&directory, quarantine, Some(expected));
        assert!(absent.contains("recovery state is unknown"));
        assert!(!absent.contains("verified recoverable"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn payload_collision_aborts_when_reserved_metadata_cannot_be_cleaned() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-info-collision-cleanup-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"still recoverable").unwrap();
        let backend = TrashBackend::with_data_home(root.join("data"), effective_uid());
        FORCE_PAYLOAD_MOVE_FAILURE.with(|failure| failure.set(Some(io::ErrorKind::AlreadyExists)));
        FORCE_INFO_CLEANUP_FAILURE.with(|force| force.set(true));

        let error = backend.trash(&source).unwrap_err();

        FORCE_INFO_CLEANUP_FAILURE.with(|force| force.set(false));
        assert!(source.is_file());
        assert!(!error.fallback_safe);
        assert!(error.to_string().contains("cannot clean reserved metadata"));
        assert!(
            error
                .to_string()
                .contains("reserved metadata cleanup failure")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_move_failure_removes_reserved_metadata_when_cleanup_succeeds() {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-info-cleanup-success-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let source = root.join("source");
        fs::write(&source, b"still recoverable").unwrap();
        let backend = TrashBackend::with_data_home(root.join("data"), effective_uid());
        FORCE_PAYLOAD_MOVE_FAILURE
            .with(|failure| failure.set(Some(io::ErrorKind::PermissionDenied)));

        let error = backend.trash(&source).unwrap_err();

        assert!(source.is_file());
        assert!(error.fallback_safe);
        assert_eq!(
            fs::read_dir(backend.home_trash().join("info"))
                .unwrap()
                .count(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_rewrite_refuses_to_cross_the_shared_serialized_limit() {
        let existing = vec![b'x'; usize::try_from(crate::directory_sizes::MAX_BYTES).unwrap()];
        let error = rewritten_cache(&existing, b"new-directory", 1, 2).unwrap_err();
        assert!(error.to_string().contains("64 MiB safety limit"));
    }
}
