use std::ffi::{CString, OsStr, OsString, c_char, c_int, c_long, c_uint};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const SIGPIPE: c_int = 13;
const SIG_IGN: usize = 1;
const SIG_ERR: usize = usize::MAX;
const AT_FDCWD: c_int = -100;
const AT_EACCESS: c_int = 0x200;
const AT_EMPTY_PATH: c_int = 0x1000;
const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
const STATX_REQUIRED_METADATA: c_uint = 0x507;
const STATX_MNT_ID: c_uint = 0x1000;
const ENOSYS: i32 = 38;

unsafe extern "C" {
    fn localtime_r(timestamp: *const TimeT, result: *mut BrokenDownTime) -> *mut BrokenDownTime;
    fn mktime(time: *mut BrokenDownTime) -> TimeT;
    fn signal(signal: c_int, handler: usize) -> usize;
    fn faccessat(directory: c_int, path: *const c_char, mode: c_int, flags: c_int) -> c_int;
    fn syscall(number: c_long, ...) -> c_long;
}

#[derive(Clone, Copy)]
pub(crate) enum AccessMode {
    Search,
    Write,
}

pub(crate) fn effective_access(path: &Path, mode: AccessMode) -> io::Result<bool> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mode = match mode {
        AccessMode::Search => 1,
        AccessMode::Write => 2,
    };
    // SAFETY: path is NUL-terminated and faccessat does not retain its pointer.
    let result = unsafe { faccessat(AT_FDCWD, path.as_ptr(), mode, AT_EACCESS) };
    if result == 0 {
        Ok(true)
    } else {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::PermissionDenied {
            Ok(false)
        } else {
            Err(error)
        }
    }
}

#[cfg(target_arch = "x86_64")]
const GETDENTS64_SYSCALL: c_long = 217;
#[cfg(target_arch = "aarch64")]
const GETDENTS64_SYSCALL: c_long = 61;

#[cfg(target_arch = "x86_64")]
const STATX_SYSCALL: c_long = 332;
#[cfg(target_arch = "aarch64")]
const STATX_SYSCALL: c_long = 291;

#[repr(C)]
#[derive(Default)]
struct LinuxStatxTimestamp {
    seconds: i64,
    nanoseconds: u32,
    reserved: i32,
}

#[repr(C)]
#[derive(Default)]
struct LinuxStatx {
    mask: u32,
    block_size: u32,
    attributes: u64,
    link_count: u32,
    uid: u32,
    gid: u32,
    mode: u16,
    spare0: u16,
    inode: u64,
    size: u64,
    blocks: u64,
    attributes_mask: u64,
    access_time: LinuxStatxTimestamp,
    birth_time: LinuxStatxTimestamp,
    change_time: LinuxStatxTimestamp,
    modification_time: LinuxStatxTimestamp,
    represented_device_major: u32,
    represented_device_minor: u32,
    device_major: u32,
    device_minor: u32,
    mount_id: u64,
    direct_io_memory_alignment: u32,
    direct_io_offset_alignment: u32,
    spare3: [u64; 12],
}

const _: [(); 256] = [(); std::mem::size_of::<LinuxStatx>()];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct MetadataSnapshot {
    device_major: u32,
    device_minor: u32,
    inode: u64,
    mode: u32,
    link_count: u64,
    blocks: u64,
}

impl MetadataSnapshot {
    pub(crate) fn from_std(metadata: &fs::Metadata) -> Self {
        let device = metadata.dev();
        Self {
            device_major: u32::try_from(
                ((device & 0x0000_0000_000f_ff00) >> 8) | ((device & 0xffff_f000_0000_0000) >> 32),
            )
            .expect("masked Linux device major fits in u32"),
            device_minor: u32::try_from((device & 0xff) | ((device & 0x0000_0fff_fff0_0000) >> 12))
                .expect("masked Linux device minor fits in u32"),
            inode: metadata.ino(),
            mode: metadata.mode(),
            link_count: metadata.nlink(),
            blocks: metadata.blocks(),
        }
    }

    pub(crate) fn is_directory(self) -> bool {
        self.mode & 0o170_000 == 0o040_000
    }

    pub(crate) fn link_count(self) -> u64 {
        self.link_count
    }

    pub(crate) fn blocks(self) -> u64 {
        self.blocks
    }

    pub(crate) fn hard_link_identity(self) -> (u32, u32, u64) {
        (self.device_major, self.device_minor, self.inode)
    }

    pub(crate) fn file_identity(self) -> (u32, u32, u64, u32) {
        (
            self.device_major,
            self.device_minor,
            self.inode,
            self.mode & 0o170_000,
        )
    }
}

pub(crate) fn metadata_at(directory: RawFd, name: &OsStr) -> io::Result<MetadataSnapshot> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "name contains NUL"))?;
    let metadata = statx_snapshot(
        directory,
        name.as_ptr(),
        AT_SYMLINK_NOFOLLOW,
        STATX_REQUIRED_METADATA,
    )?;
    Ok(metadata_snapshot(&metadata))
}

pub(crate) fn statx_unavailable(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Unsupported || error.raw_os_error() == Some(ENOSYS)
}

pub(crate) fn descriptor_mount_id(descriptor: RawFd) -> io::Result<u64> {
    statx_snapshot(
        descriptor,
        c"".as_ptr(),
        AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW,
        STATX_MNT_ID,
    )
    .map(|metadata| metadata.mount_id)
}

fn statx_snapshot(
    directory: RawFd,
    path: *const c_char,
    flags: c_int,
    required: c_uint,
) -> io::Result<LinuxStatx> {
    let mut metadata = LinuxStatx::default();
    // SAFETY: path points to a live NUL-terminated string, and metadata is
    // writable for the complete stable Linux statx structure.
    let result = unsafe {
        syscall(
            STATX_SYSCALL,
            directory,
            path,
            flags,
            required,
            &raw mut metadata,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if metadata.mask & required != required {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "required statx metadata is unavailable",
        ));
    }
    Ok(metadata)
}

fn metadata_snapshot(metadata: &LinuxStatx) -> MetadataSnapshot {
    MetadataSnapshot {
        device_major: metadata.device_major,
        device_minor: metadata.device_minor,
        inode: metadata.inode,
        mode: u32::from(metadata.mode),
        link_count: u64::from(metadata.link_count),
        blocks: metadata.blocks,
    }
}

pub(crate) struct DirectoryReader {
    buffer: Vec<u8>,
    position: usize,
    filled: usize,
    exhausted: bool,
}

impl DirectoryReader {
    pub(crate) fn new() -> Self {
        Self {
            buffer: vec![0; 16 * 1024],
            position: 0,
            filled: 0,
            exhausted: false,
        }
    }

    pub(crate) fn next(&mut self, descriptor: RawFd) -> io::Result<Option<OsString>> {
        while !self.exhausted {
            if self.position == self.filled {
                // SAFETY: the buffer is writable for its full length and the
                // live descriptor identifies an open directory.
                let read = unsafe {
                    syscall(
                        GETDENTS64_SYSCALL,
                        descriptor,
                        self.buffer.as_mut_ptr(),
                        self.buffer.len(),
                    )
                };
                if read < 0 {
                    self.exhausted = true;
                    return Err(io::Error::last_os_error());
                }
                if read == 0 {
                    self.exhausted = true;
                    return Ok(None);
                }
                self.position = 0;
                self.filled = usize::try_from(read)
                    .map_err(|_| io::Error::other("invalid directory read length"))?;
            }

            let remaining = &self.buffer[self.position..self.filled];
            if remaining.len() < 20 {
                self.exhausted = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "truncated directory entry",
                ));
            }
            let record_length = usize::from(u16::from_ne_bytes([remaining[16], remaining[17]]));
            if record_length < 20 || record_length > remaining.len() {
                self.exhausted = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid directory entry length",
                ));
            }
            let record = &remaining[..record_length];
            self.position += record_length;
            let name_bytes = &record[19..];
            let Some(end) = name_bytes.iter().position(|byte| *byte == 0) else {
                self.exhausted = true;
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry has no terminator",
                ));
            };
            let name = &name_bytes[..end];
            if name == b"." || name == b".." {
                continue;
            }
            return Ok(Some(OsString::from_vec(name.to_vec())));
        }
        Ok(None)
    }
}

#[cfg(any(
    all(
        any(target_env = "musl", target_env = "ohos"),
        target_pointer_width = "32"
    ),
    target_abi = "x32"
))]
type TimeT = i64;
#[cfg(not(any(
    all(
        any(target_env = "musl", target_env = "ohos"),
        target_pointer_width = "32"
    ),
    target_abi = "x32"
)))]
type TimeT = c_long;

#[repr(C)]
struct BrokenDownTime {
    second: c_int,
    minute: c_int,
    hour: c_int,
    day: c_int,
    month_since_january: c_int,
    year_since_1900: c_int,
    weekday: c_int,
    day_of_year: c_int,
    daylight_saving: c_int,
    utc_offset: c_long,
    timezone_name: *const c_char,
}

pub(crate) struct LocalCalendarTime {
    pub(crate) year: c_int,
    pub(crate) month: c_int,
    pub(crate) day: c_int,
    pub(crate) hour: c_int,
    pub(crate) minute: c_int,
    pub(crate) second: c_int,
}

pub(crate) fn local_calendar_time(seconds: u64) -> Option<LocalCalendarTime> {
    let timestamp = TimeT::try_from(seconds).ok()?;
    let mut local = MaybeUninit::<BrokenDownTime>::uninit();
    // SAFETY: both pointers remain valid for the call. localtime_r initializes
    // `local` on success and returns null on failure.
    let result = unsafe { localtime_r(&raw const timestamp, local.as_mut_ptr()) };
    if result.is_null() {
        return None;
    }
    // SAFETY: localtime_r returned non-null, so `local` is initialized.
    let local = unsafe { local.assume_init() };
    Some(LocalCalendarTime {
        year: local.year_since_1900 + 1900,
        month: local.month_since_january + 1,
        day: local.day,
        hour: local.hour,
        minute: local.minute,
        second: local.second,
    })
}

pub(crate) fn local_epoch(
    year: u32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    let mut local = BrokenDownTime {
        second: c_int::try_from(second).ok()?,
        minute: c_int::try_from(minute).ok()?,
        hour: c_int::try_from(hour).ok()?,
        day: c_int::try_from(day).ok()?,
        month_since_january: c_int::try_from(month).ok()?.checked_sub(1)?,
        year_since_1900: c_int::try_from(year).ok()?.checked_sub(1900)?,
        weekday: 0,
        day_of_year: 0,
        daylight_saving: -1,
        utc_offset: 0,
        timezone_name: std::ptr::null(),
    };
    // SAFETY: local is a fully initialized C-compatible tm value.
    let timestamp = unsafe { mktime(&raw mut local) };
    u64::try_from(timestamp).ok()
}

static INHERITED_SIGPIPE_IGNORED: AtomicBool = AtomicBool::new(false);

extern "C" fn capture_inherited_sigpipe() {
    // This initializer runs before Rust's runtime changes SIGPIPE. An ignored
    // disposition survives exec; any caught handler has already become the
    // default. Temporarily setting and immediately restoring SIGPIPE lets us
    // remember the only distinction GNU rm could inherit.
    let inherited = unsafe { signal(SIGPIPE, 0) };
    if inherited == SIG_ERR {
        return;
    }
    INHERITED_SIGPIPE_IGNORED.store(inherited == SIG_IGN, Ordering::Relaxed);
    let _ = unsafe { signal(SIGPIPE, inherited) };
}

#[used]
#[unsafe(link_section = ".init_array")]
static CAPTURE_INHERITED_SIGPIPE: extern "C" fn() = capture_inherited_sigpipe;

pub(crate) fn restore_inherited_sigpipe_for_exec() -> io::Result<()> {
    let disposition = if INHERITED_SIGPIPE_IGNORED.load(Ordering::Relaxed) {
        SIG_IGN
    } else {
        0
    };
    // SAFETY: Linux defines the default and ignored dispositions as the
    // constant handler values 0 and 1.
    if unsafe { signal(SIGPIPE, disposition) } == SIG_ERR {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(all(target_env = "musl", target_arch = "x86_64"))]
pub(crate) const RENAMEAT2_SYSCALL: c_long = 316;
#[cfg(all(target_env = "musl", target_arch = "aarch64"))]
pub(crate) const RENAMEAT2_SYSCALL: c_long = 276;

pub(crate) const O_WRONLY: c_int = 1;
pub(crate) const O_RDWR: c_int = 2;
pub(crate) const O_CREAT: c_int = 1 << 6;
pub(crate) const O_EXCL: c_int = 1 << 7;
pub(crate) const O_NONBLOCK: c_int = 1 << 11;

#[cfg(target_arch = "aarch64")]
pub(crate) const O_DIRECTORY: c_int = 1 << 14;
#[cfg(target_arch = "x86_64")]
pub(crate) const O_DIRECTORY: c_int = 1 << 16;

#[cfg(target_arch = "aarch64")]
pub(crate) const O_NOFOLLOW: c_int = 1 << 15;
#[cfg(target_arch = "x86_64")]
pub(crate) const O_NOFOLLOW: c_int = 1 << 17;

pub(crate) const O_CLOEXEC: c_int = 1 << 19;
pub(crate) const O_PATH: c_int = 1 << 21;

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::os::fd::AsRawFd;

    use crate::test_support::Sandbox;

    #[test]
    fn descriptor_relative_statx_matches_standard_metadata() {
        let sandbox = Sandbox::new("linux-statx");
        fs::write(sandbox.path().join("file"), b"contents").unwrap();
        std::os::unix::fs::symlink("file", sandbox.path().join("link")).unwrap();
        let directory = File::open(sandbox.path()).unwrap();

        for name in ["file", "link"] {
            let expected = super::MetadataSnapshot::from_std(
                &fs::symlink_metadata(sandbox.path().join(name)).unwrap(),
            );
            let actual = super::metadata_at(directory.as_raw_fd(), name.as_ref()).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn aarch64_open_flags_match_linux_uapi() {
        assert_eq!(super::O_WRONLY, 0o1);
        assert_eq!(super::O_RDWR, 0o2);
        assert_eq!(super::O_CREAT, 0o100);
        assert_eq!(super::O_EXCL, 0o200);
        assert_eq!(super::O_NONBLOCK, 0o4_000);
        assert_eq!(super::O_DIRECTORY, 0o40_000);
        assert_eq!(super::O_NOFOLLOW, 0o100_000);
        assert_eq!(super::O_CLOEXEC, 0o2_000_000);
        assert_eq!(super::O_PATH, 0o10_000_000);
        assert_eq!(super::STATX_SYSCALL, 291);
        #[cfg(target_env = "musl")]
        assert_eq!(super::RENAMEAT2_SYSCALL, 276);
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn x86_64_open_flags_match_linux_uapi() {
        assert_eq!(super::O_DIRECTORY, 0o200_000);
        assert_eq!(super::O_NOFOLLOW, 0o400_000);
        assert_eq!(super::STATX_SYSCALL, 332);
    }
}
