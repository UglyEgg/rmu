use std::ffi::{CString, OsStr, OsString, c_char, c_int, c_long, c_uint};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::Path;
use std::sync::atomic::Ordering;

#[cfg(test)]
use super::{
    FORCE_CHILD_MOUNT_BOUNDARY, REMOVE_CAPTURED_CLEANUP_BEFORE_UNLINK, SWAP_TREE_AFTER_PIN,
};
use super::{
    NEXT_CACHE_TEMP, SourceIdentity, TrashError, validate_confidential_metadata,
    validate_owner_controlled_metadata, validate_private_metadata,
};
use crate::display::quote;
#[cfg(target_env = "musl")]
use crate::linux::RENAMEAT2_SYSCALL;
use crate::linux::{
    DirectoryReader, O_CLOEXEC, O_CREAT, O_DIRECTORY, O_EXCL, O_NOFOLLOW, O_PATH, O_RDWR,
    descriptor_mount_id,
};

const RENAME_NOREPLACE: c_uint = 1;
const AT_REMOVEDIR: c_int = 0x200;
const PC_NAME_MAX: c_int = 3;

unsafe extern "C" {
    fn fpathconf(descriptor: c_int, name: c_int) -> c_long;
    fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
    fn openat(directory: c_int, path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
    #[cfg(not(target_env = "musl"))]
    fn renameat2(
        old_directory: c_int,
        old_path: *const c_char,
        new_directory: c_int,
        new_path: *const c_char,
        flags: c_uint,
    ) -> c_int;
    #[cfg(target_env = "musl")]
    fn syscall(number: c_long, ...) -> c_long;
    fn unlinkat(directory: c_int, path: *const c_char, flags: c_int) -> c_int;
}

pub(crate) struct PinnedDirectory {
    descriptor: File,
}

impl PinnedDirectory {
    pub(crate) fn open(path: &Path) -> Result<Self, TrashError> {
        let path_bytes = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| TrashError::message("directory path contains NUL"))?;
        // SAFETY: path_bytes is NUL-terminated and open does not retain the pointer.
        let descriptor = unsafe {
            open(
                path_bytes.as_ptr(),
                O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
        if descriptor < 0 {
            return Err(TrashError::io(
                format!("cannot pin directory {}", quote(path.as_os_str())),
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: open returned a new owned file descriptor.
        let descriptor = unsafe { File::from_raw_fd(descriptor) };
        Ok(Self { descriptor })
    }

    pub(crate) fn open_private(path: &Path, uid: u32) -> Result<Self, TrashError> {
        let directory = Self::open(path)?;
        let metadata = directory.descriptor.metadata().map_err(|error| {
            TrashError::io(
                format!("cannot revalidate {}", quote(path.as_os_str())),
                error,
            )
        })?;
        validate_private_metadata(path, &metadata, uid)?;
        Ok(directory)
    }

    pub(crate) fn open_secure_private(path: &Path, uid: u32) -> Result<Self, TrashError> {
        let directory = Self::open(path)?;
        directory.secure_private(path, uid)?;
        Ok(directory)
    }

    fn secure_private(&self, path: &Path, uid: u32) -> Result<(), TrashError> {
        let mut metadata = self.descriptor.metadata().map_err(|error| {
            TrashError::io(
                format!("cannot revalidate {}", quote(path.as_os_str())),
                error,
            )
        })?;
        validate_owner_controlled_metadata(path, &metadata, uid)?;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            self.set_private_permissions().map_err(|error| {
                TrashError::io(format!("cannot secure {}", quote(path.as_os_str())), error)
            })?;
            metadata = self.descriptor.metadata().map_err(|error| {
                TrashError::io(
                    format!("cannot revalidate {}", quote(path.as_os_str())),
                    error,
                )
            })?;
        }
        validate_confidential_metadata(path, &metadata, uid)?;
        Ok(())
    }

    pub(crate) fn open_private_child(
        &self,
        name: &OsStr,
        logical_path: &Path,
        uid: u32,
    ) -> Result<Self, TrashError> {
        let name = c_name(name).map_err(|error| {
            TrashError::io(
                format!("cannot pin {}", quote(logical_path.as_os_str())),
                error,
            )
        })?;
        // SAFETY: name is NUL-terminated, the parent descriptor is live, and
        // a successful descriptor is uniquely owned by the returned directory.
        let descriptor = unsafe {
            openat(
                self.raw_fd(),
                name.as_ptr(),
                O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
        if descriptor < 0 {
            return Err(TrashError::io(
                format!("cannot pin {}", quote(logical_path.as_os_str())),
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: openat returned a new owned descriptor.
        let descriptor = unsafe { File::from_raw_fd(descriptor) };
        let directory = Self { descriptor };
        let metadata = directory.descriptor.metadata().map_err(|error| {
            TrashError::io(
                format!("cannot revalidate {}", quote(logical_path.as_os_str())),
                error,
            )
        })?;
        validate_private_metadata(logical_path, &metadata, uid)?;
        Ok(directory)
    }

    fn set_private_permissions(&self) -> io::Result<()> {
        let descriptor_path = format!("/proc/self/fd/{}", self.raw_fd());
        fs::set_permissions(descriptor_path, fs::Permissions::from_mode(0o700))
    }

    fn open_child_directory(&self, name: &OsStr) -> io::Result<Self> {
        let name = c_name(name)?;
        // SAFETY: name is NUL-terminated, the parent descriptor is live, and
        // the returned descriptor is uniquely owned.
        let descriptor = unsafe {
            openat(
                self.raw_fd(),
                name.as_ptr(),
                O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor.
        Ok(Self {
            descriptor: unsafe { File::from_raw_fd(descriptor) },
        })
    }

    pub(crate) fn raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }

    pub(crate) fn revalidate_path(&self, path: &Path) -> io::Result<()> {
        let pinned = self.descriptor.metadata()?;
        let current = fs::symlink_metadata(path)?;
        if SourceIdentity::from_metadata(&pinned).matches(&current) {
            Ok(())
        } else {
            Err(io::Error::other("directory path identity changed"))
        }
    }

    pub(crate) fn identity(&self) -> io::Result<SourceIdentity> {
        self.descriptor
            .metadata()
            .map(|metadata| SourceIdentity::from_metadata(&metadata))
    }

    fn mount_id(&self) -> io::Result<u64> {
        descriptor_mount_id(self.descriptor.as_raw_fd())
    }

    fn metadata_and_mount_id(&self, name: &OsStr) -> io::Result<(fs::Metadata, u64)> {
        let entry = self.open_entry(name)?;
        let metadata = entry.metadata()?;
        let mount_id = descriptor_mount_id(entry.as_raw_fd())?;
        Ok((metadata, mount_id))
    }

    pub(super) fn open_entry(&self, name: &OsStr) -> io::Result<File> {
        let name = c_name(name)?;
        // O_PATH with O_NOFOLLOW pins the directory entry itself, including a
        // symlink, without opening or executing the referenced object.
        // SAFETY: name is NUL-terminated, the parent descriptor is live, and
        // openat does not retain the pointer.
        let descriptor = unsafe {
            openat(
                self.raw_fd(),
                name.as_ptr(),
                O_PATH | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }

    pub(crate) fn metadata(&self, name: &OsStr) -> io::Result<fs::Metadata> {
        self.open_entry(name)?.metadata()
    }

    pub(crate) fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        let name = c_name(name)?;
        // SAFETY: name is NUL-terminated, the directory descriptor is live, and
        // unlinkat does not retain the pointer.
        let result = unsafe { unlinkat(self.raw_fd(), name.as_ptr(), 0) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(crate) fn remove_directory(&self, name: &OsStr) -> io::Result<()> {
        let name = c_name(name)?;
        // SAFETY: name is NUL-terminated, the directory descriptor is live, and
        // unlinkat does not retain the pointer. AT_REMOVEDIR asks the kernel to
        // atomically refuse a nonempty directory.
        let result = unsafe { unlinkat(self.raw_fd(), name.as_ptr(), AT_REMOVEDIR) };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub(crate) fn remove_file_if_matches(
        &self,
        name: &OsStr,
        expected: SourceIdentity,
    ) -> io::Result<()> {
        self.remove_file_if_matches_in(name, expected, self, false)
    }

    pub(crate) fn clean_up_file_if_matches(
        &self,
        name: &OsStr,
        expected: SourceIdentity,
    ) -> io::Result<()> {
        self.remove_file_if_matches_in(name, expected, self, true)
    }

    pub(super) fn remove_directory_if_matches(
        &self,
        name: &OsStr,
        expected: SourceIdentity,
    ) -> io::Result<()> {
        for _ in 0..1024 {
            let quarantine = self.quarantine_name();
            match rename_no_replace_at(self, name, self, &quarantine) {
                Ok(()) => {
                    let metadata = self.metadata(&quarantine);
                    if metadata
                        .as_ref()
                        .is_ok_and(|metadata| expected.matches(metadata))
                    {
                        return match self.remove_directory(&quarantine) {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                let restore = rename_no_replace_at(self, &quarantine, self, name);
                                if restore.is_ok() {
                                    Err(error)
                                } else {
                                    Err(io::Error::new(
                                        error.kind(),
                                        format!(
                                            "{error}; {}",
                                            quarantine_recovery_state(
                                                self,
                                                &quarantine,
                                                Some(expected)
                                            )
                                        ),
                                    ))
                                }
                            }
                        };
                    }
                    let restore = rename_no_replace_at(self, &quarantine, self, name);
                    return if restore.is_ok() {
                        Err(io::Error::other(
                            "refusing to remove a directory whose identity changed; it was restored",
                        ))
                    } else {
                        Err(io::Error::other(format!(
                            "refusing to remove a directory whose identity changed; {}",
                            quarantine_recovery_state(self, &quarantine, Some(expected))
                        )))
                    };
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve a quarantine name for directory removal",
        ))
    }

    pub(super) fn clean_up_unverified_new_file(
        &self,
        name: &OsStr,
        created: &File,
    ) -> io::Result<()> {
        for _ in 0..1024 {
            let quarantine = self.quarantine_name();
            match rename_no_replace_at(self, name, self, &quarantine) {
                Ok(()) => {
                    let created_metadata = created.metadata();
                    let captured_metadata = self.metadata(&quarantine);
                    let created_identity = created_metadata
                        .as_ref()
                        .ok()
                        .map(SourceIdentity::from_metadata);
                    if let (Some(created_identity), Ok(captured_metadata)) =
                        (created_identity, &captured_metadata)
                    {
                        if created_identity.matches(captured_metadata) {
                            return self.remove_file_if_matches(&quarantine, created_identity);
                        }
                    }
                    let restore = rename_no_replace_at(self, &quarantine, self, name);
                    return if restore.is_ok() {
                        Err(io::Error::other(
                            "could not verify a newly created entry during cleanup; the captured entry was restored",
                        ))
                    } else {
                        Err(io::Error::other(format!(
                            "could not verify a newly created entry during cleanup; {}",
                            quarantine_recovery_state(self, &quarantine, created_identity)
                        )))
                    };
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve a quarantine name for unverified cleanup",
        ))
    }

    fn remove_file_if_matches_in(
        &self,
        name: &OsStr,
        expected: SourceIdentity,
        quarantine_directory: &Self,
        initial_absence_is_success: bool,
    ) -> io::Result<()> {
        for _ in 0..1024 {
            let quarantine = quarantine_directory.quarantine_name();
            match rename_no_replace_at(self, name, quarantine_directory, &quarantine) {
                Ok(()) => {
                    let metadata = quarantine_directory.metadata(&quarantine);
                    if metadata
                        .as_ref()
                        .is_ok_and(|metadata| expected.matches(metadata))
                    {
                        #[cfg(test)]
                        REMOVE_CAPTURED_CLEANUP_BEFORE_UNLINK.with(|remove| {
                            if remove.replace(false) {
                                quarantine_directory.remove_file(&quarantine).unwrap();
                            }
                        });
                        return match quarantine_directory.remove_file(&quarantine) {
                            Ok(()) => Ok(()),
                            Err(error) => {
                                let restore = rename_no_replace_at(
                                    quarantine_directory,
                                    &quarantine,
                                    self,
                                    name,
                                );
                                match restore {
                                    Ok(()) => Err(error),
                                    Err(restore_error) => Err(io::Error::new(
                                        error.kind(),
                                        format!(
                                            "{error}; cleanup could not restore {}: {}; {}",
                                            quote(&quarantine),
                                            restore_error,
                                            quarantine_recovery_state(
                                                quarantine_directory,
                                                &quarantine,
                                                Some(expected)
                                            )
                                        ),
                                    )),
                                }
                            }
                        };
                    }
                    let restore =
                        rename_no_replace_at(quarantine_directory, &quarantine, self, name);
                    return if restore.is_ok() {
                        Err(io::Error::other(
                            "refusing to remove an entry whose identity changed; it was restored",
                        ))
                    } else {
                        Err(io::Error::other(format!(
                            "refusing to remove an entry whose identity changed; {}",
                            quarantine_recovery_state(
                                quarantine_directory,
                                &quarantine,
                                Some(expected)
                            )
                        )))
                    };
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error)
                    if initial_absence_is_success && error.kind() == io::ErrorKind::NotFound =>
                {
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve a quarantine name for safe cleanup",
        ))
    }

    pub(crate) fn quarantine_name(&self) -> OsString {
        let sequence = NEXT_CACHE_TEMP.fetch_add(1, Ordering::Relaxed);
        let mut bytes = format!(".rmu-quarantine-{}-{sequence}", std::process::id()).into_bytes();
        let maximum = self.name_max();
        if bytes.len() > maximum {
            bytes = bytes.split_off(bytes.len() - maximum);
        }
        OsString::from_vec(bytes)
    }

    pub(crate) fn name_max(&self) -> usize {
        // SAFETY: the descriptor is live and PC_NAME_MAX has no pointer arguments.
        let value = unsafe { fpathconf(self.raw_fd(), PC_NAME_MAX) };
        usize::try_from(value)
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or(255)
    }
}

pub(crate) fn quarantine_recovery_state(
    directory: &PinnedDirectory,
    name: &OsStr,
    expected: Option<SourceIdentity>,
) -> String {
    match directory.metadata(name) {
        Ok(metadata) if expected.is_some_and(|identity| identity.matches(&metadata)) => {
            format!(
                "the captured entry is verified recoverable as {}",
                quote(name)
            )
        }
        Ok(_) => format!(
            "recovery state is unknown; an entry exists as {}, but its identity is unverified",
            quote(name)
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => format!(
            "recovery state is unknown; the quarantine name {} is no longer present",
            quote(name)
        ),
        Err(error) => format!(
            "recovery state is unknown; cannot inspect quarantine name {}: {error}",
            quote(name)
        ),
    }
}

pub(crate) fn remove_tree_if_matches(
    parent: &PinnedDirectory,
    name: &OsStr,
    expected: SourceIdentity,
) -> io::Result<()> {
    let parent_mount_id = parent.mount_id()?;
    let directory = parent.open_child_directory(name)?;
    let metadata = directory.descriptor.metadata()?;
    if !expected.matches(&metadata) {
        return Err(io::Error::other(
            "refusing recursive removal because directory identity changed",
        ));
    }
    let directory_mount_id = directory.mount_id()?;
    if directory_mount_id != parent_mount_id {
        return Err(io::Error::other(
            "refusing permanent removal across a mounted filesystem boundary",
        ));
    }
    #[cfg(test)]
    SWAP_TREE_AFTER_PIN.with(|swap| {
        if swap.replace(false) {
            let original = OsStr::new(".rmu-test-original");
            let replacement = OsStr::new(".rmu-test-replacement");
            rename_no_replace_at(parent, name, parent, original).unwrap();
            rename_no_replace_at(parent, replacement, parent, name).unwrap();
        }
    });
    clear_pinned_tree(directory, directory_mount_id)?;
    parent.remove_directory_if_matches(name, expected)
}

struct RemovalFrame {
    directory: PinnedDirectory,
    entries: DirectoryReader,
    parent_entry: Option<(OsString, SourceIdentity)>,
    mount_id: u64,
}

fn removal_frame(
    directory: PinnedDirectory,
    parent_entry: Option<(OsString, SourceIdentity)>,
    mount_id: u64,
) -> RemovalFrame {
    RemovalFrame {
        directory,
        entries: DirectoryReader::new(),
        parent_entry,
        mount_id,
    }
}

fn clear_pinned_tree(directory: PinnedDirectory, mount_id: u64) -> io::Result<()> {
    let mut stack = vec![removal_frame(directory, None, mount_id)];
    while !stack.is_empty() {
        let next = {
            let frame = stack
                .last_mut()
                .expect("removal stack was checked as nonempty");
            frame.entries.next(frame.directory.raw_fd())?
        };
        let Some(name) = next else {
            let completed = stack.pop().expect("removal stack was checked as nonempty");
            if let Some((name, identity)) = completed.parent_entry {
                stack
                    .last()
                    .expect("a child removal frame has a parent")
                    .directory
                    .remove_directory_if_matches(&name, identity)?;
            }
            continue;
        };
        let parent = &stack
            .last()
            .expect("removal stack was checked as nonempty")
            .directory;
        let (initial, entry_mount_id) = parent.metadata_and_mount_id(&name)?;
        #[cfg(test)]
        let entry_mount_id = FORCE_CHILD_MOUNT_BOUNDARY.with(|force| {
            if force.replace(false) {
                entry_mount_id.wrapping_add(1)
            } else {
                entry_mount_id
            }
        });
        let parent_mount_id = stack
            .last()
            .expect("the removal stack was checked as nonempty")
            .mount_id;
        if entry_mount_id != parent_mount_id {
            return Err(io::Error::other(
                "refusing permanent removal across a mounted filesystem boundary",
            ));
        }
        let identity = SourceIdentity::from_metadata(&initial);
        if initial.file_type().is_dir() {
            let child = parent.open_child_directory(&name)?;
            let opened = child.descriptor.metadata()?;
            if !identity.matches(&opened) {
                return Err(io::Error::other(
                    "refusing recursive removal because child identity changed",
                ));
            }
            stack.push(removal_frame(child, Some((name, identity)), entry_mount_id));
        } else {
            parent.remove_file_if_matches(&name, identity)?;
        }
    }
    Ok(())
}

pub(super) fn create_info_file_at(
    directory: &PinnedDirectory,
    name: &OsStr,
) -> io::Result<(File, SourceIdentity)> {
    open_new_private_file_at(
        directory,
        name,
        O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
        0o600,
    )
}

pub(crate) fn open_new_private_file_at(
    directory: &PinnedDirectory,
    name: &OsStr,
    flags: c_int,
    mode: c_uint,
) -> io::Result<(File, SourceIdentity)> {
    let file = open_file_at(directory, name, flags, mode)?;
    let identity = match file.metadata() {
        Ok(metadata) => SourceIdentity::from_metadata(&metadata),
        Err(error) => {
            let cleanup = directory.clean_up_unverified_new_file(name, &file);
            return Err(with_cleanup_error(error, cleanup));
        }
    };
    if let Err(error) = file.set_permissions(fs::Permissions::from_mode(mode)) {
        drop(file);
        return Err(with_cleanup_error(
            error,
            directory.clean_up_file_if_matches(name, identity),
        ));
    }
    Ok((file, identity))
}

pub(crate) fn with_cleanup_error(primary: io::Error, cleanup: io::Result<()>) -> io::Error {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup_error) => io::Error::new(
            primary.kind(),
            format!("{primary}; safe cleanup also reported: {cleanup_error}"),
        ),
    }
}

pub(crate) fn open_file_at(
    directory: &PinnedDirectory,
    name: &OsStr,
    flags: c_int,
    mode: c_uint,
) -> io::Result<File> {
    let name = c_name(name)?;
    // SAFETY: name is NUL-terminated, the directory descriptor is live, and
    // openat does not retain the pointer. A successful descriptor is uniquely owned.
    let descriptor = unsafe { openat(directory.raw_fd(), name.as_ptr(), flags, mode) };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: openat returned a new owned file descriptor.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

pub(crate) fn write_and_sync(file: &mut File, contents: &[u8]) -> io::Result<()> {
    file.write_all(contents)?;
    file.sync_data()
}

pub(crate) fn rename_no_replace_at(
    source_directory: &PinnedDirectory,
    source: &OsStr,
    destination_directory: &PinnedDirectory,
    destination: &OsStr,
) -> io::Result<()> {
    let source = c_name(source)?;
    let destination = c_name(destination)?;
    // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
    let result = rename_at2(
        source_directory.raw_fd(),
        &source,
        destination_directory.raw_fd(),
        &destination,
        RENAME_NOREPLACE,
    );
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn rename_replace_at(
    source_directory: &PinnedDirectory,
    source: &OsStr,
    destination_directory: &PinnedDirectory,
    destination: &OsStr,
) -> io::Result<()> {
    let source = c_name(source)?;
    let destination = c_name(destination)?;
    // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
    let result = rename_at2(
        source_directory.raw_fd(),
        &source,
        destination_directory.raw_fd(),
        &destination,
        0,
    );
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(target_env = "musl"))]
fn rename_at2(
    source_directory: c_int,
    source: &CString,
    destination_directory: c_int,
    destination: &CString,
    flags: c_uint,
) -> c_int {
    // SAFETY: both C strings are NUL-terminated and live for the duration of the call.
    unsafe {
        renameat2(
            source_directory,
            source.as_ptr(),
            destination_directory,
            destination.as_ptr(),
            flags,
        )
    }
}

#[cfg(target_env = "musl")]
fn rename_at2(
    source_directory: c_int,
    source: &CString,
    destination_directory: c_int,
    destination: &CString,
    flags: c_uint,
) -> c_int {
    // musl does not expose a renameat2 wrapper in every supported release.
    // SAFETY: the syscall number is selected for the target architecture;
    // both C strings are NUL-terminated and live for the duration of the call.
    let result = unsafe {
        syscall(
            RENAMEAT2_SYSCALL,
            source_directory,
            source.as_ptr(),
            destination_directory,
            destination.as_ptr(),
            flags,
        )
    };
    c_int::try_from(result).unwrap_or(-1)
}

fn c_name(name: &OsStr) -> io::Result<CString> {
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "file name contains NUL"))
}
