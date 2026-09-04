use std::collections::HashSet;
use std::ffi::{CString, OsStr, c_char, c_int, c_uint};
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::path::{Path, PathBuf};

use crate::display::quote;
use crate::linux::{
    DirectoryReader, MetadataSnapshot, O_CLOEXEC, O_DIRECTORY, O_NOFOLLOW, O_PATH, metadata_at,
    statx_unavailable,
};

#[cfg(test)]
mod integration_tests;

const UPDATE_INTERVAL: u64 = 256;
const HARD_LINK_IDENTITY_LIMIT: usize = 65_536;

unsafe extern "C" {
    fn open(path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
    fn openat(directory: c_int, path: *const c_char, flags: c_int, mode: c_uint) -> c_int;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanSummary {
    pub entries: u64,
    pub bytes: u64,
    pub unreadable: u64,
    pub first_error: Option<String>,
}

impl ScanSummary {
    #[must_use]
    pub fn complete(&self) -> bool {
        self.unreadable == 0
    }
}

#[derive(Debug)]
pub struct ScanError {
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for ScanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot scan {}: {}",
            quote(self.path.as_os_str()),
            self.source
        )
    }
}

impl std::error::Error for ScanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[allow(clippy::too_many_lines)]
pub fn scan_tree(
    root: &Path,
    mut progress: impl FnMut(&ScanSummary),
) -> Result<ScanSummary, ScanError> {
    let initial_root_metadata = fs::symlink_metadata(root).map_err(|source| ScanError {
        path: root.to_path_buf(),
        source,
    })?;
    let initial_root_metadata = MetadataSnapshot::from_std(&initial_root_metadata);
    let mut summary = ScanSummary::default();
    // Unique files need no identity bookkeeping. Retain only multiply-linked
    // non-directory identities so memory tracks hard-link groups rather than
    // every entry in a large tree.
    let mut counted_hardlinks = HashSet::new();
    let mut use_statx = true;
    if !initial_root_metadata.is_directory() {
        count_entry(
            &initial_root_metadata,
            &mut summary,
            &mut counted_hardlinks,
            HARD_LINK_IDENTITY_LIMIT,
        )
        .map_err(|source| ScanError {
            path: root.to_path_buf(),
            source,
        })?;
        progress(&summary);
        return Ok(summary);
    }

    let (root_directory, root_metadata) = pin_root(root, &initial_root_metadata)?;
    count_entry(
        &root_metadata,
        &mut summary,
        &mut counted_hardlinks,
        HARD_LINK_IDENTITY_LIMIT,
    )
    .map_err(|source| ScanError {
        path: root.to_path_buf(),
        source,
    })?;
    progress(&summary);

    let mut stack = vec![ScanFrame {
        directory: root_directory,
        entries: DirectoryReader::new(),
    }];
    while !stack.is_empty() {
        let next = {
            let frame = stack
                .last_mut()
                .expect("the scanner stack was checked as nonempty");
            frame.entries.next(frame.directory.raw_fd())
        };
        let name = match next {
            Ok(Some(name)) => name,
            Ok(None) => {
                stack.pop();
                continue;
            }
            Err(error) => {
                let directory = &stack
                    .last()
                    .expect("the scanner stack was checked as nonempty")
                    .directory;
                record_error(&mut summary, &directory.logical_path, &error);
                stack.pop();
                continue;
            }
        };
        let directory = &stack
            .last()
            .expect("the scanner stack was checked as nonempty")
            .directory;
        let initial_metadata = match directory.entry_metadata(&name, &mut use_statx) {
            Ok(metadata) => metadata,
            Err(error) => {
                let path = directory.logical_path.join(&name);
                record_error(&mut summary, &path, &error);
                continue;
            }
        };
        if initial_metadata.is_directory() {
            let path = directory.logical_path.join(&name);
            let child = match directory.open_child(&name, path.clone()) {
                Ok(child) => child,
                Err(error) => {
                    count_entry(
                        &initial_metadata,
                        &mut summary,
                        &mut counted_hardlinks,
                        HARD_LINK_IDENTITY_LIMIT,
                    )
                    .map_err(|source| ScanError {
                        path: path.clone(),
                        source,
                    })?;
                    record_error(&mut summary, &path, &error);
                    continue;
                }
            };
            let metadata = match child.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    record_error(&mut summary, &path, &error);
                    continue;
                }
            };
            if !same_identity(&initial_metadata, &metadata) {
                record_error(
                    &mut summary,
                    &path,
                    &io::Error::other("directory identity changed while it was being pinned"),
                );
                continue;
            }
            count_entry(
                &metadata,
                &mut summary,
                &mut counted_hardlinks,
                HARD_LINK_IDENTITY_LIMIT,
            )
            .map_err(|source| ScanError {
                path: path.clone(),
                source,
            })?;
            stack.push(ScanFrame {
                directory: child,
                entries: DirectoryReader::new(),
            });
        } else {
            count_entry(
                &initial_metadata,
                &mut summary,
                &mut counted_hardlinks,
                HARD_LINK_IDENTITY_LIMIT,
            )
            .map_err(|source| ScanError {
                path: directory.logical_path.join(&name),
                source,
            })?;
        }
        if summary.entries % UPDATE_INTERVAL == 0 {
            progress(&summary);
        }
    }
    progress(&summary);
    Ok(summary)
}

struct ScanFrame {
    directory: ScanDirectory,
    entries: DirectoryReader,
}

fn pin_root(
    root: &Path,
    initial_metadata: &MetadataSnapshot,
) -> Result<(ScanDirectory, MetadataSnapshot), ScanError> {
    let directory = ScanDirectory::open(root).map_err(|source| ScanError {
        path: root.to_path_buf(),
        source,
    })?;
    let metadata = directory.metadata().map_err(|source| ScanError {
        path: root.to_path_buf(),
        source,
    })?;
    if !same_identity(initial_metadata, &metadata) {
        return Err(ScanError {
            path: root.to_path_buf(),
            source: io::Error::other("directory identity changed while it was being pinned"),
        });
    }
    Ok((directory, metadata))
}

struct ScanDirectory {
    descriptor: File,
    logical_path: PathBuf,
}

impl ScanDirectory {
    fn open(path: &Path) -> io::Result<Self> {
        let path_bytes = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
        // SAFETY: path_bytes is NUL-terminated and open does not retain it.
        let descriptor =
            unsafe { open(path_bytes.as_ptr(), O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC, 0) };
        Self::from_descriptor(descriptor, path.to_path_buf())
    }

    fn open_child(&self, name: &OsStr, logical_path: PathBuf) -> io::Result<Self> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))?;
        // SAFETY: name is NUL-terminated, the parent descriptor is live, and
        // openat does not retain the pointer.
        let descriptor = unsafe {
            openat(
                self.raw_fd(),
                name.as_ptr(),
                O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                0,
            )
        };
        Self::from_descriptor(descriptor, logical_path)
    }

    fn from_descriptor(descriptor: c_int, logical_path: PathBuf) -> io::Result<Self> {
        if descriptor < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: open or openat returned a new owned descriptor.
        let descriptor = unsafe { File::from_raw_fd(descriptor) };
        Ok(Self {
            descriptor,
            logical_path,
        })
    }

    fn raw_fd(&self) -> RawFd {
        self.descriptor.as_raw_fd()
    }

    fn entry_metadata(&self, name: &OsStr, use_statx: &mut bool) -> io::Result<MetadataSnapshot> {
        if *use_statx {
            match metadata_at(self.raw_fd(), name) {
                Ok(metadata) => return Ok(metadata),
                Err(error) if statx_unavailable(&error) => *use_statx = false,
                Err(error) => return Err(error),
            }
        }
        self.entry_metadata_with_openat(name)
    }

    fn entry_metadata_with_openat(&self, name: &OsStr) -> io::Result<MetadataSnapshot> {
        let name = CString::new(name.as_bytes())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))?;
        // SAFETY: name is NUL-terminated, the directory descriptor is live,
        // and openat does not retain the pointer.
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
        unsafe { File::from_raw_fd(descriptor) }
            .metadata()
            .map(|metadata| MetadataSnapshot::from_std(&metadata))
    }

    fn metadata(&self) -> io::Result<MetadataSnapshot> {
        self.descriptor
            .metadata()
            .map(|metadata| MetadataSnapshot::from_std(&metadata))
    }
}

fn same_identity(left: &MetadataSnapshot, right: &MetadataSnapshot) -> bool {
    left.file_identity() == right.file_identity()
}

fn count_entry(
    metadata: &MetadataSnapshot,
    summary: &mut ScanSummary,
    counted_hardlinks: &mut HashSet<(u32, u32, u64)>,
    hard_link_limit: usize,
) -> io::Result<()> {
    summary.entries = summary.entries.saturating_add(1);
    let unique_or_directory = metadata.link_count() <= 1 || metadata.is_directory();
    let should_count = if unique_or_directory {
        true
    } else {
        let identity = metadata.hard_link_identity();
        if counted_hardlinks.contains(&identity) {
            false
        } else {
            if counted_hardlinks.len() >= hard_link_limit {
                return Err(io::Error::other(format!(
                    "hard-link accounting exceeds the {hard_link_limit}-identity safety limit"
                )));
            }
            counted_hardlinks.try_reserve(1).map_err(|error| {
                io::Error::other(format!(
                    "cannot reserve hard-link accounting memory: {error}"
                ))
            })?;
            counted_hardlinks.insert(identity);
            true
        }
    };
    if should_count {
        summary.bytes = summary
            .bytes
            .saturating_add(metadata.blocks().saturating_mul(512));
    }
    Ok(())
}

fn record_error(summary: &mut ScanSummary, path: &Path, error: &io::Error) {
    summary.unreadable = summary.unreadable.saturating_add(1);
    if summary.first_error.is_none() {
        summary.first_error = Some(format!("{}: {error}", quote(path.as_os_str())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn hard_link_accounting_is_exact_at_the_limit_and_fails_before_growth() {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "rmu-hard-link-limit-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("first"), b"first").unwrap();
        fs::hard_link(root.join("first"), root.join("first-link")).unwrap();
        fs::write(root.join("second"), b"second").unwrap();
        fs::hard_link(root.join("second"), root.join("second-link")).unwrap();

        let first = MetadataSnapshot::from_std(&fs::metadata(root.join("first")).unwrap());
        let first_link =
            MetadataSnapshot::from_std(&fs::metadata(root.join("first-link")).unwrap());
        let second = MetadataSnapshot::from_std(&fs::metadata(root.join("second")).unwrap());
        let mut summary = ScanSummary::default();
        let mut identities = HashSet::new();

        count_entry(&first, &mut summary, &mut identities, 1).unwrap();
        let bytes_at_limit = summary.bytes;
        count_entry(&first_link, &mut summary, &mut identities, 1).unwrap();
        assert_eq!(summary.bytes, bytes_at_limit);
        let error = count_entry(&second, &mut summary, &mut identities, 1).unwrap_err();
        assert!(error.to_string().contains("1-identity safety limit"));
        assert_eq!(identities.len(), 1);

        fs::remove_dir_all(root).unwrap();
    }
}
