use std::collections::{HashMap, HashSet};
#[cfg(test)]
use std::ffi::OsStr;
use std::io;
#[cfg(test)]
use std::os::unix::ffi::OsStrExt;

use super::metadata::{percent_decode, trim_cr};
use crate::directory_sizes::{
    MAX_DECODED_NAME_BYTES, MAX_ENCODED_NAME_BYTES, MAX_RECORDS, read as read_cache_bytes,
    replace as replace_cache_file,
};
use crate::freedesktop::PinnedDirectory;

pub(super) fn read_directory_sizes(
    root: &PinnedDirectory,
) -> io::Result<HashMap<Vec<u8>, (u64, i64)>> {
    let contents = read_cache_bytes(root)?;
    let mut cache = HashMap::new();
    let mut records = 0_usize;
    let mut decoded_name_bytes = 0_usize;
    for (index, line) in contents.split(|byte| *byte == b'\n').enumerate() {
        let line = trim_cr(line);
        if line.is_empty() {
            continue;
        }
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid directorysizes record on line {}", index + 1),
            )
        };
        let mut fields = line.splitn(3, |byte| *byte == b' ');
        let size = fields
            .next()
            .and_then(|field| std::str::from_utf8(field).ok())
            .and_then(|field| field.parse().ok())
            .ok_or_else(invalid)?;
        let mtime = fields
            .next()
            .and_then(|field| std::str::from_utf8(field).ok())
            .and_then(|field| field.parse().ok())
            .ok_or_else(invalid)?;
        let name = fields.next().ok_or_else(invalid)?;
        records = records.checked_add(1).ok_or_else(invalid)?;
        if records > MAX_RECORDS || name.len() > MAX_ENCODED_NAME_BYTES {
            return Err(invalid());
        }
        let name = percent_decode(name).map_err(|_| invalid())?;
        if name.len() > MAX_DECODED_NAME_BYTES {
            return Err(invalid());
        }
        decoded_name_bytes = decoded_name_bytes
            .checked_add(name.len())
            .ok_or_else(invalid)?;
        if u64::try_from(decoded_name_bytes).unwrap_or(u64::MAX) > crate::directory_sizes::MAX_BYTES
        {
            return Err(invalid());
        }
        cache.try_reserve(1).map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                "cannot allocate directorysizes index",
            )
        })?;
        cache.insert(name, (size, mtime));
    }
    Ok(cache)
}

#[cfg(test)]
pub(super) fn remove_cache_name(root: &PinnedDirectory, name: &OsStr) -> io::Result<()> {
    let names = HashSet::from([name.as_bytes().to_vec()]);
    remove_cache_names(root, &names)
}

pub(super) fn remove_cache_names(
    root: &PinnedDirectory,
    names: &HashSet<Vec<u8>>,
) -> io::Result<()> {
    let cache = read_cache_bytes(root)?;
    let mut output = Vec::new();
    for line in cache.split_inclusive(|byte| *byte == b'\n') {
        let body = trim_cr(line.strip_suffix(b"\n").unwrap_or(line));
        let mut fields = body.splitn(3, |byte| *byte == b' ');
        let matches = fields.next().is_some()
            && fields.next().is_some()
            && fields
                .next()
                .filter(|field| field.len() <= MAX_ENCODED_NAME_BYTES)
                .and_then(|field| percent_decode(field).ok())
                .is_some_and(|decoded| names.contains(&decoded));
        if !matches {
            output.extend_from_slice(line);
        }
    }
    if output == cache {
        Ok(())
    } else {
        replace_cache(root, &output)
    }
}

pub(super) fn replace_cache(root: &PinnedDirectory, contents: &[u8]) -> io::Result<()> {
    replace_cache_file(root, contents)
}
