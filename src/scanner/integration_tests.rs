use std::fs;

use crate::scanner::scan_tree;
use crate::test_support::Sandbox;

#[test]
fn scans_iteratively_without_following_symlinks_or_double_counting_hardlinks() {
    let sandbox = Sandbox::new("scanner-physical");
    let root = sandbox.path().join("root");
    let outside = sandbox.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("must-not-be-counted"), b"outside").unwrap();
    fs::write(root.join("file"), vec![0_u8; 8192]).unwrap();
    fs::hard_link(root.join("file"), root.join("hard-link")).unwrap();
    fs::create_dir(root.join("directory")).unwrap();
    fs::write(root.join("directory/child"), b"child").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("directory-link")).unwrap();
    std::os::unix::fs::symlink("missing", root.join("broken-link")).unwrap();

    let mut updates = 0;
    let summary = scan_tree(&root, |_| updates += 1).unwrap();

    assert_eq!(summary.entries, 7);
    assert!(summary.complete());
    assert!(summary.bytes >= 8192);
    assert!(updates >= 2);
}

#[test]
fn handles_deep_trees_without_recursive_calls() {
    let sandbox = Sandbox::new("scanner-deep");
    let root = sandbox.path().join("root");
    fs::create_dir(&root).unwrap();
    let mut current = root.clone();
    for _ in 0..200 {
        current.push("d");
        fs::create_dir(&current).unwrap();
    }

    let summary = scan_tree(&root, |_| {}).unwrap();

    assert_eq!(summary.entries, 201);
    assert!(summary.complete());
}

#[test]
fn handles_wide_trees_without_retaining_every_sibling_descriptor() {
    let sandbox = Sandbox::new("scanner-wide");
    let root = sandbox.path().join("root");
    fs::create_dir(&root).unwrap();
    for index in 0..256 {
        let directory = root.join(format!("directory-{index}"));
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("file"), b"contents").unwrap();
    }

    let summary = scan_tree(&root, |_| {}).unwrap();

    assert_eq!(summary.entries, 513);
    assert!(summary.complete());
}

#[test]
fn a_root_replaced_by_a_symlink_during_progress_is_never_followed() {
    let sandbox = Sandbox::new("scanner-root-swap");
    let root = sandbox.path().join("root");
    let parked = sandbox.path().join("parked");
    let outside = sandbox.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(root.join("inside"), b"inside").unwrap();
    fs::write(outside.join("outside-one"), b"outside").unwrap();
    fs::write(outside.join("outside-two"), b"outside").unwrap();
    let mut swapped = false;

    let summary = scan_tree(&root, |_| {
        if !swapped {
            fs::rename(&root, &parked).unwrap();
            std::os::unix::fs::symlink(&outside, &root).unwrap();
            swapped = true;
        }
    })
    .unwrap();

    assert!(swapped);
    assert_eq!(summary.entries, 2);
    assert!(summary.complete());
}
