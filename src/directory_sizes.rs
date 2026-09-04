use std::ffi::OsStr;
use std::io::{self, Read};

use crate::freedesktop::{
    PinnedDirectory, SourceIdentity, open_file_at, open_new_private_file_at, rename_replace_at,
    with_cleanup_error, write_and_sync,
};
use crate::linux::{O_CLOEXEC, O_CREAT, O_EXCL, O_NOFOLLOW, O_NONBLOCK, O_WRONLY};

pub(crate) const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_RECORDS: usize = 262_144;
pub(crate) const MAX_ENCODED_NAME_BYTES: usize = 255 * 3;
pub(crate) const MAX_DECODED_NAME_BYTES: usize = 255;

pub(crate) fn read_bounded(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut contents = Vec::new();
    reader.take(MAX_BYTES + 1).read_to_end(&mut contents)?;
    ensure_size(contents.len())?;
    Ok(contents)
}

pub(crate) fn ensure_serialized(contents: &[u8]) -> io::Result<()> {
    ensure_size(contents.len())
}

fn ensure_size(length: usize) -> io::Result<()> {
    if u64::try_from(length).unwrap_or(u64::MAX) > MAX_BYTES {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directorysizes exceeds the 64 MiB safety limit",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn read(root: &PinnedDirectory) -> io::Result<Vec<u8>> {
    let expected = match root.metadata(OsStr::new("directorysizes")) {
        Ok(metadata) if metadata.file_type().is_file() => SourceIdentity::from_metadata(&metadata),
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "directorysizes is not a regular file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut file = open_file_at(
        root,
        OsStr::new("directorysizes"),
        O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK,
        0,
    )?;
    let opened = file.metadata()?;
    if !opened.file_type().is_file() || !expected.matches(&opened) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "directorysizes changed while it was being opened",
        ));
    }
    read_bounded(&mut file)
}

pub(crate) fn replace(root: &PinnedDirectory, contents: &[u8]) -> io::Result<()> {
    ensure_serialized(contents)?;
    for _ in 0..1024 {
        let temporary = root.quarantine_name();
        match open_new_private_file_at(
            root,
            &temporary,
            O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC,
            0o600,
        ) {
            Ok((mut file, identity)) => {
                if let Err(error) = write_and_sync(&mut file, contents) {
                    return Err(with_cleanup_error(
                        error,
                        root.clean_up_file_if_matches(&temporary, identity),
                    ));
                }
                return rename_replace_at(root, &temporary, root, OsStr::new("directorysizes"))
                    .map_err(|error| {
                        with_cleanup_error(
                            error,
                            root.clean_up_file_if_matches(&temporary, identity),
                        )
                    });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "cannot reserve a directorysizes temporary file",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialized_size_limit_has_one_authoritative_boundary() {
        assert!(ensure_size(usize::try_from(MAX_BYTES).unwrap()).is_ok());
        assert!(ensure_size(usize::try_from(MAX_BYTES + 1).unwrap()).is_err());
    }
}
