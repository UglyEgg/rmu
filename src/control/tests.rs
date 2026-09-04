use super::*;
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

use crate::ctl_args::SortField;

#[test]
fn doctor_problem_identity_is_separate_from_rendered_text() {
    let inventory = Inventory {
        warnings: vec![
            InventoryWarning {
                key: ProblemKey::RootWarning {
                    device: 1,
                    inode: 2,
                    index: 0,
                },
                message: "same diagnostic".into(),
            },
            InventoryWarning {
                key: ProblemKey::RootWarning {
                    device: 3,
                    inode: 4,
                    index: 0,
                },
                message: "same diagnostic".into(),
            },
        ],
        ..Inventory::default()
    };

    let problems = inventory_problems(&inventory);

    assert_eq!(problems[0].message, problems[1].message);
    assert_ne!(problems[0].key, problems[1].key);
}
use std::ffi::CString;
use std::os::raw::{c_char, c_uint};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::freedesktop::TrashBackend;

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    fn mkfifo(path: *const c_char, mode: c_uint) -> i32;
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!(
            "/tmp/rmu-control-{label}-{}-{sequence}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn isolated_manager(sandbox: &TestDirectory) -> TrashManager {
    let data_home = sandbox.path().join("data");
    let root = data_home.join("Trash");
    fs::create_dir_all(root.join("files")).unwrap();
    fs::create_dir_all(root.join("info")).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("files"), fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(root.join("info"), fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let root_metadata = fs::metadata(&root).unwrap();
    TrashManager {
        manager_id: NEXT_MANAGER_ID.fetch_add(1, AtomicOrdering::Relaxed),
        uid: effective_uid(),
        home_mount: sandbox.path().to_path_buf(),
        roots: vec![TrashRoot {
            path: root.clone(),
            aliases: vec![root],
            volume_mount_points: Vec::new(),
            info_bases: vec![data_home],
            is_home: true,
            device: root_metadata.dev(),
            inode: root_metadata.ino(),
        }],
        discovery_warnings: Vec::new(),
        discovery_complete: true,
    }
}

fn backend(sandbox: &TestDirectory) -> TrashBackend {
    TrashBackend::with_data_home(sandbox.path().join("data"), effective_uid())
}

#[test]
fn ids_change_with_root_and_name() {
    let root_a = TrashRoot {
        path: PathBuf::from("/alias/a"),
        aliases: vec![PathBuf::from("/alias/a")],
        volume_mount_points: Vec::new(),
        info_bases: vec![PathBuf::from("/data")],
        is_home: true,
        device: 1,
        inode: 10,
    };
    let root_b = TrashRoot {
        path: PathBuf::from("/alias/b"),
        device: 1,
        inode: 11,
        ..root_a.clone()
    };
    assert_ne!(
        stable_id(&root_a, OsStr::new("x"), None, None),
        stable_id(&root_b, OsStr::new("x"), None, None)
    );
    assert_ne!(
        stable_id(&root_a, OsStr::new("x"), None, None),
        stable_id(&root_a, OsStr::new("y"), None, None)
    );
    let alias = TrashRoot {
        path: PathBuf::from("/different-alias"),
        ..root_a.clone()
    };
    assert_eq!(
        stable_id(&root_a, OsStr::new("x"), None, None),
        stable_id(&alias, OsStr::new("x"), None, None)
    );
}

#[test]
fn percent_decoder_is_byte_preserving() {
    assert_eq!(percent_decode(b"a%20b%FF").unwrap(), b"a b\xff");
    assert!(percent_decode(b"a%2").is_err());
    assert!(percent_decode(b"%00").is_err());
}

#[test]
fn civil_date_matches_unix_epoch() {
    assert!(parse_deletion_date_local("1970-01-02T00:00:00").is_some());
    assert!(parse_deletion_date_local("2024-01-01T00:00:00").is_some());
    assert_eq!(parse_deletion_date_local("2023-02-29T00:00:00"), None);
    assert!(parse_deletion_date_local("2024-02-29T00:00:00").is_some());
}

#[test]
fn trashinfo_uses_first_fields_and_the_correct_relative_base() {
    let root = TrashRoot {
        path: PathBuf::from("/tmp/example-data/Trash"),
        aliases: vec![PathBuf::from("/tmp/example-data/Trash")],
        volume_mount_points: Vec::new(),
        info_bases: vec![PathBuf::from("/tmp/example-data")],
        is_home: true,
        device: 1,
        inode: 2,
    };
    let parsed = parse_info(
            b"[Trash Info]\nPath=first/name\nPath=second/name\nDeletionDate=2024-01-02T03:04:05\nDeletionDate=2025-01-01T00:00:00\n",
            &root,
        )
        .unwrap();
    assert_eq!(
        parsed.original,
        Some(PathBuf::from("/tmp/example-data/first/name"))
    );
    assert_eq!(parsed.deletion_date, "2024-01-02T03:04:05");
}

#[test]
fn per_volume_trash_rejects_absolute_metadata_paths() {
    let root = TrashRoot {
        path: PathBuf::from("/media/disk/.Trash-1000"),
        aliases: vec![PathBuf::from("/media/disk/.Trash-1000")],
        volume_mount_points: vec![PathBuf::from("/media/disk")],
        info_bases: vec![PathBuf::from("/media/disk")],
        is_home: false,
        device: 1,
        inode: 2,
    };
    assert!(
        parse_info(
            b"[Trash Info]\nPath=/absolute/name\nDeletionDate=2024-01-02T03:04:05\n",
            &root
        )
        .is_err()
    );
}

#[test]
fn same_device_bind_alias_never_selects_home_trash() {
    let selector = filesystem_selector_for_mount(Path::new("/home-mount"), PathBuf::from("/bind"));
    let volume = TrashRoot {
        path: PathBuf::from("/volume-trash"),
        aliases: vec![PathBuf::from("/volume-trash")],
        volume_mount_points: vec![PathBuf::from("/volume"), PathBuf::from("/bind")],
        info_bases: vec![PathBuf::from("/volume"), PathBuf::from("/bind")],
        is_home: false,
        device: 7,
        inode: 9,
    };
    let home = TrashRoot {
        path: PathBuf::from("/home-trash"),
        aliases: vec![PathBuf::from("/home-trash")],
        volume_mount_points: Vec::new(),
        info_bases: vec![PathBuf::from("/home-data")],
        is_home: true,
        device: 7,
        inode: 10,
    };
    assert!(selector.matches(&volume));
    assert!(!selector.matches(&home));
}

#[test]
fn relative_metadata_from_bind_aliases_requires_an_explicit_destination() {
    let sandbox = TestDirectory::new("ambiguous-bind-restore");
    let mut manager = isolated_manager(&sandbox);
    manager.roots[0]
        .info_bases
        .push(sandbox.path().join("bind-alias"));
    let files = sandbox.path().join("data/Trash/files");
    let info = sandbox.path().join("data/Trash/info");
    fs::write(files.join("document"), b"recoverable").unwrap();
    fs::write(
        info.join("document.trashinfo"),
        b"[Trash Info]\nPath=folder/document\nDeletionDate=2024-01-02T03:04:05\n",
    )
    .unwrap();

    let inventory = manager.inventory();
    assert!(inventory.complete);
    assert_eq!(inventory.entries[0].original_path, None);
    assert_eq!(
        inventory.entries[0].restore_name.as_deref(),
        Some(OsStr::new("document"))
    );
    let authorized = manager.authorize_mutations(&inventory, "restore").unwrap();
    assert!(
        authorized
            .restore(
                &inventory.entries[0],
                None,
                ConflictPolicy::Fail,
                false,
                true,
                None,
            )
            .unwrap_err()
            .contains("use restore --to")
    );

    let destination = sandbox.path().join("restored");
    fs::create_dir(&destination).unwrap();
    let result = authorized
        .restore(
            &inventory.entries[0],
            Some(&destination),
            ConflictPolicy::Fail,
            false,
            false,
            None,
        )
        .unwrap();
    assert_eq!(result.path, Some(destination.join("document")));
    assert_eq!(
        fs::read(destination.join("document")).unwrap(),
        b"recoverable"
    );
}

#[test]
fn filesystem_selectors_require_the_selected_path_to_exist() {
    let sandbox = TestDirectory::new("missing-filesystem-selector");
    let manager = isolated_manager(&sandbox);
    let missing = sandbox.path().join("does-not-exist");

    let error = manager.filesystem_selector(&missing).err().unwrap();

    assert!(error.contains("cannot resolve filesystem selector"));
    assert!(!missing.exists());
}

#[test]
fn pinned_destination_check_catches_an_intermediate_symlink_into_trash() {
    let sandbox = TestDirectory::new("destination-symlink");
    let manager = isolated_manager(&sandbox);
    let bridge = sandbox.path().join("bridge");
    std::os::unix::fs::symlink(&manager.roots[0].path, &bridge).unwrap();
    let requested_parent = bridge.join("files");
    let pinned = PinnedDirectory::open(&requested_parent).unwrap();
    let requested = requested_parent.join("recovered");
    assert!(
        reject_pinned_destination_in_roots(
            &pinned,
            OsStr::new("recovered"),
            &requested,
            &manager.roots,
        )
        .is_err()
    );
}

#[test]
fn directory_size_cleanup_compares_decoded_names() {
    let sandbox = TestDirectory::new("cache-decoding");
    let manager = isolated_manager(&sandbox);
    let root = &manager.roots[0];
    let cache = root.path.join("directorysizes");
    fs::write(&cache, b"10 20 space%20name\n30 40 keep\n").unwrap();
    let pinned = PinnedDirectory::open_private(&root.path, effective_uid()).unwrap();
    remove_cache_name(&pinned, OsStr::new("space name")).unwrap();
    assert_eq!(fs::read(cache).unwrap(), b"30 40 keep\n");
}

#[test]
fn date_sort_and_age_filters_use_deletion_date_not_info_mtime() {
    let sandbox = TestDirectory::new("deletion-date");
    let manager = isolated_manager(&sandbox);
    let future = sandbox.path().join("future");
    let old = sandbox.path().join("old");
    fs::write(&future, b"future").unwrap();
    fs::write(&old, b"old").unwrap();
    let future_receipt = backend(&sandbox).trash(&future).unwrap();
    let old_receipt = backend(&sandbox).trash(&old).unwrap();

    rewrite_deletion_date(&future_receipt.info, "2030-01-01T00:00:00");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    rewrite_deletion_date(&old_receipt.info, "2000-01-01T00:00:00");
    assert!(
        fs::metadata(&future_receipt.info).unwrap().mtime()
            < fs::metadata(&old_receipt.info).unwrap().mtime()
    );

    let inventory = manager.inventory();
    let sorted = manager
        .filter_list(
            &inventory,
            &ListOptions {
                filesystem: None,
                older_than: None,
                newer_than: None,
                sort: SortField::Date,
                reverse: false,
            },
        )
        .unwrap();
    assert_eq!(sorted[0].original_path.as_deref(), Some(old.as_path()));
    assert_eq!(sorted[1].original_path.as_deref(), Some(future.as_path()));
    let old_only = manager
        .filter_list(
            &inventory,
            &ListOptions {
                filesystem: None,
                older_than: Some(365 * 24 * 60 * 60),
                newer_than: None,
                sort: SortField::Date,
                reverse: false,
            },
        )
        .unwrap();
    assert_eq!(old_only.len(), 1);
    assert_eq!(old_only[0].original_path.as_deref(), Some(old.as_path()));
}

fn rewrite_deletion_date(path: &Path, replacement: &str) {
    let mut contents = fs::read_to_string(path).unwrap();
    let start = contents.find("DeletionDate=").unwrap() + "DeletionDate=".len();
    contents.replace_range(start..start + 19, replacement);
    fs::write(path, contents).unwrap();
}

#[test]
fn doctor_recovers_only_from_an_explicit_complete_root() {
    let sandbox = TestDirectory::new("doctor-orphan");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("orphan.txt");
    fs::write(&source, b"still valuable").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    fs::remove_file(&receipt.info).unwrap();

    let inventory = manager.inventory();
    assert!(inventory.complete);
    let authorized = manager
        .authorize_mutations(&inventory, "recover orphans")
        .unwrap();
    let recovery = sandbox.path().join("nested/recovery");
    let report = authorized.doctor(false, Some(&recovery), false);
    assert_eq!(report.actions.len(), 1);
    assert!(report.unresolved.is_empty());
    assert!(!report.observed.is_empty());
    assert!(!receipt.payload.exists());
    let recovered = fs::read_dir(&recovery)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(fs::read(recovered).unwrap(), b"still valuable");
    assert_eq!(fs::metadata(&recovery).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn doctor_does_not_infer_orphans_from_a_missing_counterpart_directory() {
    let sandbox = TestDirectory::new("doctor-incomplete");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("paired.txt");
    fs::write(&source, b"paired").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    fs::rename(
        sandbox.path().join("data/Trash/info"),
        sandbox.path().join("hidden-info"),
    )
    .unwrap();

    let inventory = manager.inventory();
    assert!(!inventory.complete);
    let recovery = sandbox.path().join("recovery");
    let error = manager
        .authorize_mutations(&inventory, "recover orphans")
        .unwrap_err();
    assert!(error.contains("inventory was incomplete"));
    assert!(receipt.payload.exists());
    assert!(!recovery.exists());
}

#[test]
fn doctor_never_splits_a_pair_completed_after_inventory() {
    let sandbox = TestDirectory::new("doctor-concurrent-pair");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("document");
    fs::write(&source, b"payload").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let metadata = fs::read(&receipt.info).unwrap();
    fs::remove_file(&receipt.info).unwrap();
    let inventory = manager.inventory();
    let authorized = manager
        .authorize_mutations(&inventory, "recover orphans")
        .unwrap();
    fs::write(&receipt.info, metadata).unwrap();
    let recovery = sandbox.path().join("recovery");

    let report = authorized.doctor(false, Some(&recovery), false);

    assert!(report.actions.is_empty());
    assert!(
        report
            .unresolved
            .iter()
            .any(|problem| problem.message.contains("changed during capture"))
    );
    assert!(receipt.payload.is_file());
    assert!(receipt.info.is_file());
}

#[test]
fn doctor_reports_a_counterpart_directory_lost_after_inventory() {
    let sandbox = TestDirectory::new("doctor-late-directory-loss");
    let manager = isolated_manager(&sandbox);
    let inventory = manager.inventory();
    assert!(inventory.complete);
    let authorized = manager
        .authorize_mutations(&inventory, "recover orphans")
        .unwrap();
    let info = sandbox.path().join("data/Trash/info");
    fs::rename(&info, sandbox.path().join("hidden-info")).unwrap();

    let report = authorized.doctor(false, Some(&sandbox.path().join("recovery")), false);

    assert!(report.actions.is_empty());
    assert!(report.unresolved.iter().any(|problem| {
        problem.message.contains("cannot inspect") && problem.message.contains("/info")
    }));
}

#[test]
fn inaccessible_trash_candidates_make_discovery_incomplete() {
    let candidate = Path::new("/volume/.Trash-1000");
    let mut discovery = DiscoveryReport::new();
    assert!(!trash_candidate_exists(
        candidate,
        Err(io::Error::from(io::ErrorKind::PermissionDenied)),
        &mut discovery,
    ));
    assert!(!discovery.complete);
    assert_eq!(discovery.warnings.len(), 1);
    assert!(discovery.warnings[0].contains("cannot inspect Trash candidate"));

    let mut absent = DiscoveryReport::new();
    assert!(!trash_candidate_exists(
        candidate,
        Err(io::Error::from(io::ErrorKind::NotFound)),
        &mut absent,
    ));
    assert!(absent.complete);
    assert!(absent.warnings.is_empty());
}

#[test]
fn known_unsafe_shared_trash_is_a_warning_not_unknown_state() {
    let mut discovery = DiscoveryReport::new();
    discovery.warn("ignoring unsafe shared Trash: sticky bit is not set".into());

    assert!(discovery.complete);
    assert_eq!(discovery.warnings.len(), 1);
}

#[test]
fn mutation_authority_is_complete_manager_bound_and_entry_bound() {
    let sandbox = TestDirectory::new("mutation-authority");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("file");
    fs::write(&source, b"keep").unwrap();
    backend(&sandbox).trash(&source).unwrap();
    let inventory = manager.inventory();
    let copied_entry = inventory.entries[0].clone();
    let authorized = manager.authorize_mutations(&inventory, "purge").unwrap();

    assert!(
        authorized
            .purge(&copied_entry, true)
            .unwrap_err()
            .contains("outside the authorized inventory")
    );

    let other_manager = isolated_manager(&sandbox);
    assert!(
        other_manager
            .authorize_mutations(&inventory, "purge")
            .unwrap_err()
            .contains("another Trash manager")
    );
    assert!(
        other_manager
            .diagnose(&inventory)
            .unwrap_err()
            .contains("another Trash manager")
    );

    let mut incomplete = manager.inventory();
    incomplete.complete = false;
    assert!(
        manager
            .authorize_mutations(&incomplete, "empty")
            .unwrap_err()
            .contains("inventory was incomplete")
    );
    assert!(inventory.entries[0].payload.exists());
}

#[test]
fn purge_never_deletes_a_pair_completed_after_inventory() {
    for missing_payload in [false, true] {
        let sandbox = TestDirectory::new("purge-concurrent-pair");
        let manager = isolated_manager(&sandbox);
        let source = sandbox.path().join("document");
        fs::write(&source, b"payload").unwrap();
        let receipt = backend(&sandbox).trash(&source).unwrap();
        let payload = fs::read(&receipt.payload).unwrap();
        let metadata = fs::read(&receipt.info).unwrap();
        if missing_payload {
            fs::remove_file(&receipt.payload).unwrap();
        } else {
            fs::remove_file(&receipt.info).unwrap();
        }
        let inventory = manager.inventory();
        let authorized = manager.authorize_mutations(&inventory, "purge").unwrap();
        if missing_payload {
            fs::write(&receipt.payload, &payload).unwrap();
        } else {
            fs::write(&receipt.info, &metadata).unwrap();
        }

        let error = authorized.purge(&inventory.entries[0], false).unwrap_err();

        assert!(error.contains("changed during capture"), "{error}");
        assert_eq!(fs::read(&receipt.payload).unwrap(), payload);
        assert_eq!(fs::read(&receipt.info).unwrap(), metadata);
    }
}

#[test]
fn isolated_restore_completes_a_real_transaction() {
    let sandbox = TestDirectory::new("restore-transaction");
    let manager = isolated_manager(&sandbox);
    let original = sandbox.path().join("document");
    fs::write(&original, b"recoverable").unwrap();
    backend(&sandbox).trash(&original).unwrap();
    let inventory = manager.inventory();
    let authorized = manager.authorize_mutations(&inventory, "restore").unwrap();

    authorized
        .restore(
            &inventory.entries[0],
            None,
            ConflictPolicy::Fail,
            false,
            false,
            None,
        )
        .unwrap();

    assert_eq!(fs::read(&original).unwrap(), b"recoverable");
    assert_eq!(
        fs::read_dir(sandbox.path().join("data/Trash/files"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn inaccessible_user_mounts_are_unknown_but_system_mounts_are_out_of_scope() {
    let sandbox = TestDirectory::new("mount-inspection-scope");
    let user_mount = sandbox.path().join("volume");
    assert!(matches!(
        inspect_mount(
            &user_mount,
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            effective_uid(),
        ),
        MountInspection::Unknown(_)
    ));
    assert!(matches!(
        inspect_mount(
            Path::new("/boot/efi"),
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            effective_uid(),
        ),
        MountInspection::OutsideUserScope
    ));
}

#[test]
fn non_searchable_mount_scope_depends_on_user_ownership() {
    let sandbox = TestDirectory::new("non-searchable-mount");
    let mount = sandbox.path().join("volume");
    fs::create_dir(&mount).unwrap();
    fs::set_permissions(&mount, fs::Permissions::from_mode(0o000)).unwrap();
    let metadata = fs::metadata(&mount).unwrap();

    let inspection = inspect_mount(&mount, Ok(metadata), effective_uid());

    fs::set_permissions(&mount, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(inspection, MountInspection::Unknown(_)));
    assert!(matches!(
        classify_mount_access(effective_uid().wrapping_add(1), effective_uid(), Ok(false)),
        MountInspection::OutsideUserScope
    ));
}

#[test]
fn doctor_mutations_require_complete_global_discovery() {
    let sandbox = TestDirectory::new("doctor-global-incomplete");
    let manager = isolated_manager(&sandbox);
    let mut inventory = manager.inventory();
    inventory.complete = false;
    inventory.warnings.push(InventoryWarning {
        key: ProblemKey::DiscoveryWarning(usize::MAX),
        message: "an external Trash root could not be inspected".into(),
    });
    let recovery = sandbox.path().join("recovery");

    let error = manager
        .authorize_mutations(&inventory, "run doctor repairs")
        .unwrap_err();
    assert!(!recovery.exists());
    assert!(error.contains("inventory was incomplete"));
}

#[test]
fn doctor_reauthorizes_cache_repair_after_recovery() {
    let sandbox = TestDirectory::new("doctor-refresh-authorization");
    let manager = isolated_manager(&sandbox);
    let cache = sandbox.path().join("data/Trash/directorysizes");
    fs::write(&cache, b"preserve this cache\n").unwrap();
    let inventory = manager.inventory();
    let authorized = manager
        .authorize_mutations(&inventory, "run doctor repairs")
        .unwrap();
    let recovery = sandbox.path().join("recovery");
    FORCE_NEXT_INVENTORY_INCOMPLETE.with(|force| force.set(true));

    let report = authorized.doctor(true, Some(&recovery), false);

    assert!(report.actions.is_empty());
    assert!(
        report
            .unresolved
            .iter()
            .any(|problem| problem.message.contains("inventory was incomplete"))
    );
    assert_eq!(fs::read(cache).unwrap(), b"preserve this cache\n");
}

#[test]
fn cache_repair_uses_trashinfo_mtime_not_deletion_date() {
    let sandbox = TestDirectory::new("doctor-cache");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("child"), b"cache me").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let expected_mtime = fs::metadata(&receipt.info).unwrap().mtime();
    let cache = sandbox.path().join("data/Trash/directorysizes");
    fs::write(&cache, b"not a cache line\n").unwrap();

    let inventory = manager.inventory();
    let authorized = manager
        .authorize_mutations(&inventory, "repair cache")
        .unwrap();
    let report = authorized.doctor(true, None, false);
    assert_eq!(report.actions.len(), 1);
    let contents = fs::read_to_string(cache).unwrap();
    let fields: Vec<_> = contents.split_whitespace().collect();
    assert_eq!(fields[1].parse::<i64>().unwrap(), expected_mtime);
}

#[test]
fn cache_repair_never_publishes_after_a_selected_directory_changes() {
    let sandbox = TestDirectory::new("doctor-cache-race");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("child"), b"cache me").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let cache = sandbox.path().join("data/Trash/directorysizes");
    fs::write(&cache, b"preserve this cache\n").unwrap();
    let inventory = manager.inventory();
    let parked = sandbox.path().join("parked-payload");
    fs::rename(&receipt.payload, &parked).unwrap();
    let authorized = manager
        .authorize_mutations(&inventory, "repair cache")
        .unwrap();

    let report = authorized.doctor(true, None, false);

    assert!(report.actions.is_empty());
    assert!(report.unresolved.iter().any(|problem| {
        problem
            .message
            .contains("refusing to publish a partial directorysizes cache")
    }));
    assert_eq!(fs::read(&cache).unwrap(), b"preserve this cache\n");
}

#[test]
fn advisory_cache_failure_does_not_revoke_namespace_authority() {
    let sandbox = TestDirectory::new("doctor-advisory-cache");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("orphan.txt");
    fs::write(&source, b"recoverable").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    fs::remove_file(&receipt.info).unwrap();
    let cache = sandbox.path().join("data/Trash/directorysizes");
    let cache_bytes = CString::new(cache.as_os_str().as_bytes()).unwrap();
    // SAFETY: cache_bytes is NUL-terminated and mkfifo does not retain it.
    assert_eq!(unsafe { mkfifo(cache_bytes.as_ptr(), 0o600) }, 0);

    let inventory = manager.inventory();
    assert!(inventory.complete);
    let authorized = manager
        .authorize_mutations(&inventory, "run doctor repairs")
        .unwrap();
    let recovery = sandbox.path().join("recovery");

    let report = authorized.doctor(true, Some(&recovery), false);

    assert_eq!(report.actions.len(), 2);
    assert!(!receipt.payload.exists());
    assert_eq!(fs::read_dir(recovery).unwrap().count(), 1);
    assert!(fs::metadata(cache).unwrap().is_file());
}

#[test]
fn doctor_recovers_backend_cache_temporaries_but_not_paired_user_names() {
    let sandbox = TestDirectory::new("doctor-temporaries");
    let manager = isolated_manager(&sandbox);
    let root = sandbox.path().join("data/Trash");
    let temporary = root.join(".directorysizes.rmu-123-4.tmp");
    fs::write(&temporary, b"cache temp").unwrap();
    let source = sandbox.path().join(".rmu-quarantine-123-456");
    fs::write(&source, b"legitimate user data").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();

    let inventory = manager.inventory();
    let authorized = manager
        .authorize_mutations(&inventory, "recover orphans")
        .unwrap();
    let recovery = sandbox.path().join("recovery");
    let report = authorized.doctor(false, Some(&recovery), false);
    assert_eq!(report.actions.len(), 1);
    assert!(!temporary.exists());
    assert!(receipt.payload.exists());
    assert!(receipt.info.exists());
    assert_eq!(fs::read_dir(recovery).unwrap().count(), 1);
}

#[test]
fn doctor_never_recovers_prefix_only_quarantine_names() {
    for name in [".rmu-quarantine-user-data", ".rtrash-quarantine-user-data"] {
        let sandbox = TestDirectory::new("doctor-prefix-only-quarantine");
        let manager = isolated_manager(&sandbox);
        let unexpected = sandbox.path().join("data/Trash/info").join(name);
        fs::write(&unexpected, b"user data").unwrap();
        let inventory = manager.inventory();
        assert!(!inventory.complete);
        let recovery = sandbox.path().join("recovery");

        assert!(
            manager
                .authorize_mutations(&inventory, "recover orphans")
                .unwrap_err()
                .contains("inventory was incomplete")
        );
        assert_eq!(fs::read(&unexpected).unwrap(), b"user data");
        assert!(!recovery.exists());
    }
}

#[test]
fn doctor_recognizes_pre_rename_internal_names() {
    assert!(is_internal_quarantine_name(OsStr::new(
        ".rtrash-quarantine-123-456"
    )));
    assert!(is_internal_root_temporary(OsStr::new(
        ".directorysizes.rtrash-123-456.tmp"
    )));
    assert!(!is_internal_quarantine_name(OsStr::new(
        ".rtrash-quarantine-user-data"
    )));
}

#[test]
fn restore_dry_run_reserves_renamed_destinations() {
    let sandbox = TestDirectory::new("restore-reservations");
    let manager = isolated_manager(&sandbox);
    for directory in ["one", "two"] {
        let parent = sandbox.path().join(directory);
        fs::create_dir(&parent).unwrap();
        let source = parent.join("same.txt");
        fs::write(&source, directory.as_bytes()).unwrap();
        backend(&sandbox).trash(&source).unwrap();
    }
    let inventory = manager.inventory();
    let authorized = manager.authorize_mutations(&inventory, "restore").unwrap();
    let destination = sandbox.path().join("restore-here");
    let mut reserved = HashSet::new();
    let mut paths = Vec::new();
    for entry in &inventory.entries {
        let result = authorized
            .restore(
                entry,
                Some(&destination),
                ConflictPolicy::Rename,
                false,
                true,
                Some(&reserved),
            )
            .unwrap();
        let path = result.path.unwrap();
        reserved.insert(path.clone());
        paths.push(path);
    }
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0], paths[1]);
    assert!(inventory.entries.iter().all(|entry| entry.payload.exists()));
}

#[test]
fn failed_restore_publication_rolls_back_the_original_trash_pair() {
    let sandbox = TestDirectory::new("restore-publication-rollback");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("document");
    fs::write(&source, b"recoverable").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let inventory = manager.inventory();
    let original_id = inventory.entries[0].id.clone();
    let destination = sandbox.path().join("restore-here");
    fs::create_dir(&destination).unwrap();
    let authorized = manager.authorize_mutations(&inventory, "restore").unwrap();
    FORCE_RESTORE_PUBLICATION_FAILURE.with(|force| force.set(true));

    let error = authorized
        .restore(
            &inventory.entries[0],
            Some(&destination),
            ConflictPolicy::Fail,
            false,
            false,
            None,
        )
        .unwrap_err();

    assert_eq!(error.matches("verified restored").count(), 2);
    assert!(!error.contains("unknown"));
    assert_eq!(fs::read(&receipt.payload).unwrap(), b"recoverable");
    assert!(receipt.info.exists());
    assert!(!destination.join("document").exists());
    for directory in [
        receipt.payload.parent().unwrap(),
        receipt.info.parent().unwrap(),
    ] {
        assert!(
            fs::read_dir(directory)
                .unwrap()
                .all(|item| { !is_internal_quarantine_name(&item.unwrap().file_name()) })
        );
    }
    let restored = manager.inventory();
    assert!(restored.complete);
    assert_eq!(restored.entries.len(), 1);
    assert_eq!(restored.entries[0].id, original_id);
    assert_eq!(restored.entries[0].original_path.as_ref(), Some(&source));
    assert_eq!(restored.entries[0].state, EntryState::Healthy);
}

#[test]
fn restore_destination_relocation_rolls_back_the_original_trash_pair() {
    let sandbox = TestDirectory::new("restore-destination-relocation");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("document");
    fs::write(&source, b"recoverable").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let inventory = manager.inventory();
    let destination = sandbox.path().join("restore-here");
    fs::create_dir(&destination).unwrap();
    let authorized = manager.authorize_mutations(&inventory, "restore").unwrap();
    MOVE_RESTORE_DESTINATION_AFTER_PUBLICATION.with(|move_parent| move_parent.set(true));

    let error = authorized
        .restore(
            &inventory.entries[0],
            Some(&destination),
            ConflictPolicy::Fail,
            false,
            false,
            None,
        )
        .unwrap_err();

    assert!(error.contains("destination parent changed"));
    assert!(
        error.contains("payload recovery state: verified restored"),
        "unexpected error: {error}"
    );
    assert_eq!(fs::read(&receipt.payload).unwrap(), b"recoverable");
    assert!(receipt.info.exists());
    assert!(!destination.join("document").exists());
    assert!(
        !sandbox
            .path()
            .join(".rmu-test-moved-restore-parent/document")
            .exists()
    );
}

#[test]
fn metadata_capture_verification_failure_rolls_back_both_components() {
    let sandbox = TestDirectory::new("metadata-capture-rollback");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("document");
    fs::write(&source, b"recoverable").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let inventory = manager.inventory();
    let destination = sandbox.path().join("restore-here");
    fs::create_dir(&destination).unwrap();
    let authorized = manager.authorize_mutations(&inventory, "restore").unwrap();
    FAIL_CAPTURE_VERIFICATION_ON.with(|capture| capture.set(2));

    let error = authorized
        .restore(
            &inventory.entries[0],
            Some(&destination),
            ConflictPolicy::Fail,
            false,
            false,
            None,
        )
        .unwrap_err();

    assert!(error.contains("cannot verify captured entry"));
    assert_eq!(error.matches("verified restored").count(), 2);
    assert!(!error.contains("metadata recovery state: not needed"));
    assert_eq!(fs::read(&receipt.payload).unwrap(), b"recoverable");
    assert!(receipt.info.exists());
    assert!(!destination.join("document").exists());
}

#[test]
fn hardlinked_payloads_remain_actionable_independently() {
    let sandbox = TestDirectory::new("hardlinks");
    let manager = isolated_manager(&sandbox);
    let first = sandbox.path().join("first");
    let second = sandbox.path().join("second");
    fs::write(&first, b"same inode").unwrap();
    fs::hard_link(&first, &second).unwrap();
    backend(&sandbox).trash(&first).unwrap();
    backend(&sandbox).trash(&second).unwrap();
    let inventory = manager.inventory();
    assert_eq!(inventory.entries.len(), 2);
    let authorized = manager.authorize_mutations(&inventory, "purge").unwrap();
    authorized.purge(&inventory.entries[0], false).unwrap();
    let remaining = manager.inventory();
    assert_eq!(remaining.entries.len(), 1);
    let authorized = manager.authorize_mutations(&remaining, "purge").unwrap();
    authorized.purge(&remaining.entries[0], false).unwrap();
}

#[test]
fn purge_path_refuses_a_new_mount_boundary_without_deleting_payload_data() {
    let sandbox = TestDirectory::new("purge-mount-boundary");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("tree");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("sentinel"), b"must survive").unwrap();
    backend(&sandbox).trash(&source).unwrap();
    let inventory = manager.inventory();
    let authorized = manager.authorize_mutations(&inventory, "purge").unwrap();
    crate::freedesktop::force_child_mount_boundary_for_test();

    let error = authorized.purge(&inventory.entries[0], false).unwrap_err();

    assert!(error.contains("mounted filesystem boundary"));
    let files = sandbox.path().join("data/Trash/files");
    let captured = fs::read_dir(files).unwrap().next().unwrap().unwrap().path();
    assert_eq!(
        fs::read(captured.join("sentinel")).unwrap(),
        b"must survive"
    );
    assert_eq!(
        fs::read_dir(sandbox.path().join("data/Trash/info"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn purge_reports_failure_when_payload_is_gone_but_metadata_cleanup_fails() {
    let sandbox = TestDirectory::new("purge-metadata-failure");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("file");
    fs::write(&source, b"remove me").unwrap();
    backend(&sandbox).trash(&source).unwrap();
    let inventory = manager.inventory();
    let authorized = manager.authorize_mutations(&inventory, "purge").unwrap();
    FORCE_METADATA_CLEANUP_FAILURE.with(|force| force.set(true));

    let error = authorized.purge(&inventory.entries[0], false).unwrap_err();

    assert!(error.contains("payload was permanently removed"));
    assert_eq!(
        fs::read_dir(sandbox.path().join("data/Trash/files"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        fs::read_dir(sandbox.path().join("data/Trash/info"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn a_stale_id_never_selects_a_reused_trash_name() {
    let sandbox = TestDirectory::new("stale-id");
    let manager = isolated_manager(&sandbox);
    let source = sandbox.path().join("reused");
    fs::write(&source, b"first incarnation").unwrap();
    backend(&sandbox).trash(&source).unwrap();
    let first = manager.inventory();
    let old_id = first.entries[0].id.clone();
    let authorized = manager.authorize_mutations(&first, "purge").unwrap();
    authorized.purge(&first.entries[0], false).unwrap();

    fs::write(&source, b"second incarnation").unwrap();
    let receipt = backend(&sandbox).trash(&source).unwrap();
    let second = manager.inventory();
    assert_ne!(old_id, second.entries[0].id);
    assert!(manager.select(&second, &[old_id]).is_err());
    assert_eq!(fs::read(receipt.payload).unwrap(), b"second incarnation");
    assert!(receipt.info.exists());
}
