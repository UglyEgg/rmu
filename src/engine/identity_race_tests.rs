use std::ffi::OsString;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::args::{Invocation, PreserveRoot, ProgressMode, PromptMode};
use crate::engine::{Event, Observer, Prompter, execute};
use crate::freedesktop::TrashBackend;
use crate::test_support::Sandbox;

struct NoPrompt;
struct NoObserver;

impl Observer for NoObserver {
    fn observe(&mut self, _event: &Event) {}
}

impl Prompter for NoPrompt {
    fn confirm(&mut self, _prompt: &str) -> io::Result<bool> {
        Ok(true)
    }

    fn stdin_is_terminal(&self) -> bool {
        false
    }
}

struct SwapLeaf {
    source: PathBuf,
    parked: PathBuf,
    swapped: bool,
}

impl Observer for SwapLeaf {
    fn observe(&mut self, event: &Event) {
        if !self.swapped && matches!(event, Event::Moving { .. }) {
            fs::rename(&self.source, &self.parked).unwrap();
            fs::write(&self.source, b"replacement").unwrap();
            self.swapped = true;
        }
    }
}

struct SwapParent {
    parent: PathBuf,
    parked_parent: PathBuf,
    swapped: bool,
}

struct SwapEmptyDirectory {
    source: PathBuf,
    parked: PathBuf,
    swapped: bool,
}

impl Observer for SwapEmptyDirectory {
    fn observe(&mut self, event: &Event) {
        if !self.swapped && matches!(event, Event::Validated { .. }) {
            fs::rename(&self.source, &self.parked).unwrap();
            fs::create_dir(&self.source).unwrap();
            self.swapped = true;
        }
    }
}

impl Observer for SwapParent {
    fn observe(&mut self, event: &Event) {
        if !self.swapped && matches!(event, Event::Moving { .. }) {
            fs::rename(&self.parent, &self.parked_parent).unwrap();
            fs::create_dir(&self.parent).unwrap();
            fs::write(self.parent.join("victim"), b"replacement").unwrap();
            self.swapped = true;
        }
    }
}

#[test]
fn refuses_a_leaf_replaced_after_validation() {
    let sandbox = Sandbox::new("race-leaf");
    let source = sandbox.path().join("victim");
    let parked = sandbox.path().join("original");
    fs::write(&source, b"original").unwrap();
    let backend = backend(&sandbox);
    let mut observer = SwapLeaf {
        source: source.clone(),
        parked: parked.clone(),
        swapped: false,
    };

    let result = execute(&invocation(&source), &backend, &mut NoPrompt, &mut observer);

    assert!(!result.success());
    assert!(result.failures[0].message.contains("identity changed"));
    assert_eq!(fs::read(source).unwrap(), b"replacement");
    assert_eq!(fs::read(parked).unwrap(), b"original");
    assert_trash_empty(&backend);
}

#[test]
fn refuses_an_ancestor_replaced_after_validation() {
    let sandbox = Sandbox::new("race-parent");
    let parent = sandbox.path().join("parent");
    let parked_parent = sandbox.path().join("original-parent");
    fs::create_dir(&parent).unwrap();
    let source = parent.join("victim");
    fs::write(&source, b"original").unwrap();
    let backend = backend(&sandbox);
    let mut observer = SwapParent {
        parent: parent.clone(),
        parked_parent: parked_parent.clone(),
        swapped: false,
    };

    let result = execute(&invocation(&source), &backend, &mut NoPrompt, &mut observer);

    assert!(!result.success());
    assert!(result.failures[0].message.contains("identity changed"));
    assert_eq!(fs::read(parent.join("victim")).unwrap(), b"replacement");
    assert_eq!(fs::read(parked_parent.join("victim")).unwrap(), b"original");
    assert_trash_empty(&backend);
}

#[test]
fn dir_mode_refuses_an_empty_directory_replaced_after_validation() {
    let sandbox = Sandbox::new("race-empty-dir-leaf");
    let source = sandbox.path().join("empty");
    let parked = sandbox.path().join("original-empty");
    fs::create_dir(&source).unwrap();
    let backend = backend(&sandbox);
    let mut observer = SwapEmptyDirectory {
        source: source.clone(),
        parked: parked.clone(),
        swapped: false,
    };
    let mut invocation = invocation(&source);
    invocation.dir = true;

    let result = execute(&invocation, &backend, &mut NoPrompt, &mut observer);

    assert!(!result.success());
    assert!(result.failures[0].message.contains("identity changed"));
    assert!(source.is_dir());
    assert!(parked.is_dir());
    assert_trash_empty(&backend);
}

#[test]
fn one_execution_batches_directory_cache_updates() {
    let sandbox = Sandbox::new("batched-directory-cache");
    let first = sandbox.path().join("first");
    let second = sandbox.path().join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    fs::write(first.join("one"), b"one").unwrap();
    fs::write(second.join("two"), b"two").unwrap();
    let backend = backend(&sandbox);
    let mut invocation = invocation(&first);
    invocation.operands.push(second.as_os_str().to_os_string());
    invocation.recursive = true;

    let result = execute(&invocation, &backend, &mut NoPrompt, &mut NoObserver);

    assert!(result.success());
    let cache = fs::read_to_string(backend.home_trash().join("directorysizes")).unwrap();
    assert_eq!(cache.lines().count(), 2);
    assert!(cache.lines().any(|line| line.ends_with(" first")));
    assert!(cache.lines().any(|line| line.ends_with(" second")));
}

fn backend(sandbox: &Sandbox) -> TrashBackend {
    let uid = fs::metadata(sandbox.path()).unwrap().uid();
    TrashBackend::with_data_home(sandbox.path().join("data"), uid)
}

fn invocation(source: &Path) -> Invocation {
    Invocation {
        operands: vec![OsString::from(source)],
        recursive: false,
        dir: false,
        force: false,
        prompt: PromptMode::Default,
        one_file_system: false,
        preserve_root: PreserveRoot::Root,
        verbose: false,
        progress: ProgressMode::Never,
    }
}

fn assert_trash_empty(backend: &TrashBackend) {
    let root = backend.home_trash();
    if root.exists() {
        assert_eq!(fs::read_dir(root.join("files")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(root.join("info")).unwrap().count(), 0);
    }
}
