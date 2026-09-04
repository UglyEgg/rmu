use std::ffi::{CString, OsString, c_char, c_uint};
use std::fs::{self, DirBuilder};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};

use crate::freedesktop::{
    MOVE_TRASH_ROOT_AFTER_PAYLOAD, TrashBackend, trusted_shared_trash_attributes,
};
use crate::scanner::scan_tree;
use crate::test_support::Sandbox;

unsafe extern "C" {
    fn mkfifo(path: *const c_char, mode: c_uint) -> i32;
}

fn backend(sandbox: &Sandbox) -> TrashBackend {
    let uid = fs::metadata(sandbox.path()).unwrap().uid();
    TrashBackend::with_data_home(sandbox.path().join("data"), uid)
}

#[test]
fn moves_a_file_and_writes_desktop_metadata() {
    let sandbox = Sandbox::new("basic");
    let source = sandbox.path().join("document with spaces.txt");
    fs::write(&source, b"recover me").unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();

    assert!(!source.exists());
    assert_eq!(fs::read(&receipt.payload).unwrap(), b"recover me");
    let info = fs::read_to_string(&receipt.info).unwrap();
    assert!(info.starts_with("[Trash Info]\n"));
    assert!(info.contains("document%20with%20spaces.txt"));
    assert!(info.contains("\nDeletionDate="));
}

#[test]
fn moves_the_symlink_and_never_its_target() {
    let sandbox = Sandbox::new("symlink");
    let target = sandbox.path().join("target");
    let link = sandbox.path().join("link");
    fs::write(&target, b"target stays put").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let receipt = backend(&sandbox).trash(&link).unwrap();

    assert!(!link.exists());
    assert_eq!(fs::read(&target).unwrap(), b"target stays put");
    assert_eq!(fs::read_link(&receipt.payload).unwrap(), target);
}

#[test]
fn preserves_colliding_trash_entries() {
    let sandbox = Sandbox::new("collision");
    let source = sandbox.path().join("same-name");
    fs::write(&source, b"first").unwrap();
    let first = backend(&sandbox).trash(&source).unwrap();
    fs::write(&source, b"second").unwrap();
    let second = backend(&sandbox).trash(&source).unwrap();

    assert_ne!(first.payload, second.payload);
    assert_eq!(fs::read(first.payload).unwrap(), b"first");
    assert_eq!(fs::read(second.payload).unwrap(), b"second");
}

#[test]
fn percent_encodes_non_utf8_unix_names() {
    let sandbox = Sandbox::new("non-utf8");
    let name = OsString::from_vec(b"bad-\xff-name".to_vec());
    let source = sandbox.path().join(name);
    fs::write(&source, b"bytes").unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();
    let info = fs::read_to_string(receipt.info).unwrap();

    assert!(info.contains("bad-%FF-name"));
}

#[test]
fn normalizes_parent_components_before_writing_metadata() {
    let sandbox = Sandbox::new("normalized-parent");
    fs::create_dir(sandbox.path().join("base")).unwrap();
    let source = sandbox.path().join("base/../victim");
    fs::write(sandbox.path().join("victim"), b"recoverable").unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();
    let info = fs::read_to_string(receipt.info).unwrap();

    assert!(!info.contains("/../"));
    assert!(info.contains("/victim\n"));
}

#[test]
fn shortens_maximum_length_payload_names_without_losing_original_metadata() {
    let sandbox = Sandbox::new("name-max");
    let name = OsString::from_vec(vec![b'a'; 255]);
    let source = sandbox.path().join(&name);
    fs::write(&source, b"first").unwrap();

    let first = backend(&sandbox).trash(&source).unwrap();
    fs::write(&source, b"second").unwrap();
    let second = backend(&sandbox).trash(&source).unwrap();

    assert_ne!(first.payload, second.payload);
    assert!(first.payload.file_name().unwrap().len() <= 245);
    assert!(second.payload.file_name().unwrap().len() <= 245);
    assert!(first.info.file_name().unwrap().len() <= 255);
    assert!(second.info.file_name().unwrap().len() <= 255);
    let original_name_encoding = "a".repeat(255);
    assert!(
        fs::read_to_string(first.info)
            .unwrap()
            .contains(&original_name_encoding)
    );
    assert_eq!(fs::read(first.payload).unwrap(), b"first");
    assert_eq!(fs::read(second.payload).unwrap(), b"second");
}

#[test]
fn refuses_an_insecure_existing_trash_directory() {
    let sandbox = Sandbox::new("permissions");
    let trash = sandbox.path().join("data/Trash");
    DirBuilder::new()
        .recursive(true)
        .mode(0o777)
        .create(&trash)
        .unwrap();
    fs::set_permissions(&trash, std::os::unix::fs::PermissionsExt::from_mode(0o777)).unwrap();
    let source = sandbox.path().join("keep");
    fs::write(&source, b"not moved").unwrap();

    let error = backend(&sandbox).trash(&source).unwrap_err();

    assert!(error.to_string().contains("writable by other users"));
    assert_eq!(fs::read(source).unwrap(), b"not moved");
}

#[test]
fn accepts_an_owner_controlled_kde_style_0755_layout() {
    let sandbox = Sandbox::new("kde-permissions");
    let trash = sandbox.path().join("data/Trash");
    for directory in [&trash, &trash.join("files"), &trash.join("info")] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(directory)
            .unwrap();
    }
    let source = sandbox.path().join("compatible");
    fs::write(&source, b"moved").unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();

    assert_eq!(fs::read(receipt.payload).unwrap(), b"moved");
    for directory in [&trash, &trash.join("files"), &trash.join("info")] {
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o777, 0o700);
    }
}

#[test]
fn shared_trash_requires_a_sticky_directory_owned_by_root_or_the_user() {
    assert!(trusted_shared_trash_attributes(true, 0o1777, 0, 1000));
    assert!(trusted_shared_trash_attributes(true, 0o1700, 1000, 1000));
    assert!(!trusted_shared_trash_attributes(true, 0o1777, 2000, 1000));
    assert!(!trusted_shared_trash_attributes(true, 0o0777, 0, 1000));
    assert!(!trusted_shared_trash_attributes(false, 0o1777, 0, 1000));
}

#[test]
fn restores_the_source_when_the_trash_root_moves_during_publication() {
    let sandbox = Sandbox::new("moved-trash-root");
    let source = sandbox.path().join("recoverable");
    fs::write(&source, b"keep me").unwrap();
    MOVE_TRASH_ROOT_AFTER_PAYLOAD.with(|move_root| move_root.set(true));

    let error = backend(&sandbox).trash(&source).unwrap_err();

    assert!(error.to_string().contains("source was restored"));
    assert_eq!(fs::read(&source).unwrap(), b"keep me");
    let parked = sandbox.path().join("data/.rmu-test-moved-trash-root");
    assert_eq!(fs::read_dir(parked.join("files")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(parked.join("info")).unwrap().count(), 0);
}

#[test]
fn refuses_to_retrash_an_entry_through_a_symlinked_data_home() {
    let sandbox = Sandbox::new("symlinked-data-home");
    let actual_data_home = sandbox.path().join("actual-data");
    let trash = actual_data_home.join("Trash");
    for directory in [&trash, &trash.join("files"), &trash.join("info")] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .unwrap();
    }
    let linked_data_home = sandbox.path().join("linked-data");
    std::os::unix::fs::symlink(&actual_data_home, &linked_data_home).unwrap();
    let source = trash.join("files/already-trashed");
    fs::write(&source, b"recoverable").unwrap();
    fs::write(
        trash.join("info/already-trashed.trashinfo"),
        b"[Trash Info]\nPath=/original\nDeletionDate=2026-09-01T00:00:00\n",
    )
    .unwrap();
    let uid = fs::metadata(sandbox.path()).unwrap().uid();
    let backend = TrashBackend::with_data_home(linked_data_home, uid);

    let error = backend.trash(&source).unwrap_err();

    assert!(error.to_string().contains("already inside"));
    assert_eq!(fs::read(source).unwrap(), b"recoverable");
    assert_eq!(fs::read_dir(trash.join("files")).unwrap().count(), 1);
    assert_eq!(fs::read_dir(trash.join("info")).unwrap().count(), 1);
}

#[test]
fn accepts_a_symlinked_data_home_for_an_unrelated_source() {
    let sandbox = Sandbox::new("symlinked-data-home-success");
    let actual_data_home = sandbox.path().join("actual-data");
    DirBuilder::new()
        .mode(0o700)
        .create(&actual_data_home)
        .unwrap();
    let linked_data_home = sandbox.path().join("linked-data");
    std::os::unix::fs::symlink(&actual_data_home, &linked_data_home).unwrap();
    let source = sandbox.path().join("source");
    fs::write(&source, b"recoverable").unwrap();
    let uid = fs::metadata(sandbox.path()).unwrap().uid();
    let backend = TrashBackend::with_data_home(linked_data_home, uid);

    let receipt = backend.trash(&source).unwrap();

    assert_eq!(fs::read(receipt.payload).unwrap(), b"recoverable");
    assert!(
        actual_data_home
            .join("Trash/info/source.trashinfo")
            .is_file()
    );
}

#[test]
fn refuses_a_future_trash_root_inside_the_operand_without_creating_it() {
    let sandbox = Sandbox::new("future-trash-inside-operand");
    let source = sandbox.path().join("empty-operand");
    fs::create_dir(&source).unwrap();
    let linked_data_home = sandbox.path().join("linked-data");
    std::os::unix::fs::symlink(&source, &linked_data_home).unwrap();
    let uid = fs::metadata(sandbox.path()).unwrap().uid();
    let backend = TrashBackend::with_data_home(linked_data_home, uid);

    let error = backend.trash(&source).unwrap_err();

    assert!(error.to_string().contains("inside the requested operand"));
    assert!(source.is_dir());
    assert!(!source.join("Trash").exists());
    assert_eq!(fs::read_dir(&source).unwrap().count(), 0);
}

#[test]
fn caches_a_trashed_directorys_allocated_size_and_info_mtime() {
    let sandbox = Sandbox::new("directory-size");
    let source = sandbox.path().join("folder with space");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("contents"), vec![0_u8; 8192]).unwrap();
    let expected_size = scan_tree(&source, |_| {}).unwrap().bytes;

    let receipt = backend(&sandbox).trash(&source).unwrap();

    assert!(receipt.warnings.is_empty());
    let info_mtime = fs::metadata(&receipt.info).unwrap().mtime();
    let cache = fs::read_to_string(backend(&sandbox).home_trash().join("directorysizes")).unwrap();
    assert_eq!(
        cache,
        format!("{expected_size} {info_mtime} folder%20with%20space\n")
    );
}

#[test]
fn replaces_a_stale_cache_entry_for_a_reused_payload_name() {
    let sandbox = Sandbox::new("stale-directory-size");
    let trash = backend(&sandbox).home_trash();
    for directory in [&trash, &trash.join("files"), &trash.join("info")] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .unwrap();
    }
    fs::write(trash.join("directorysizes"), b"1 2 keep\n3 4 reused\n").unwrap();
    let source = sandbox.path().join("reused");
    fs::create_dir(&source).unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();

    assert!(receipt.warnings.is_empty());
    let cache = fs::read_to_string(trash.join("directorysizes")).unwrap();
    assert!(cache.starts_with("1 2 keep\n"));
    assert!(!cache.contains("3 4 reused\n"));
    assert_eq!(
        cache
            .lines()
            .filter(|line| line.ends_with(" reused"))
            .count(),
        1
    );
}

#[test]
fn a_broken_optional_cache_never_undoes_a_successful_trash_move() {
    let sandbox = Sandbox::new("broken-directory-size");
    let trash = backend(&sandbox).home_trash();
    for directory in [
        &trash,
        &trash.join("files"),
        &trash.join("info"),
        &trash.join("directorysizes"),
    ] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .unwrap();
    }
    let source = sandbox.path().join("recoverable");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("contents"), b"still here").unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();

    assert!(!source.exists());
    assert_eq!(
        fs::read(receipt.payload.join("contents")).unwrap(),
        b"still here"
    );
    assert_eq!(receipt.warnings.len(), 1);
    assert!(receipt.warnings[0].contains("size cache could not be updated"));
}

#[test]
fn a_fifo_cache_never_blocks_or_undoes_a_successful_trash_move() {
    let sandbox = Sandbox::new("fifo-cache");
    let trash = backend(&sandbox).home_trash();
    for directory in [&trash, &trash.join("files"), &trash.join("info")] {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .unwrap();
    }
    let fifo = trash.join("directorysizes");
    let fifo_bytes = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: fifo_bytes is NUL-terminated and mkfifo does not retain it.
    assert_eq!(unsafe { mkfifo(fifo_bytes.as_ptr(), 0o600) }, 0);
    let source = sandbox.path().join("directory");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("contents"), b"recoverable").unwrap();

    let receipt = backend(&sandbox).trash(&source).unwrap();

    assert!(!source.exists());
    assert!(receipt.payload.join("contents").is_file());
    assert_eq!(receipt.warnings.len(), 1);
    assert!(receipt.warnings[0].contains("not a regular file"));
}
