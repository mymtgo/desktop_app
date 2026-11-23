//! A temporary folder for tests, removed on drop. Std only: `tempfile` is not a
//! workspace dependency.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    /// A fresh, empty folder unique to this process and call.
    pub fn new(label: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("mymtgo-test-{label}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test dir");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    /// Write `bytes` to `relative` (replacing it), creating parent folders.
    pub fn write(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        fs::write(&path, bytes).expect("write test file");
        path
    }

    /// Append `bytes` to `relative`, creating it and its parents if needed.
    pub fn append(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent");
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open test file");
        file.write_all(bytes).expect("append test file");
        path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_dir_is_created_written_and_removed() {
        let path;
        {
            let dir = TestDir::new("self");
            path = dir.path().to_path_buf();
            dir.write("a/b.txt", b"one");
            dir.append("a/b.txt", b"two");
            assert_eq!(fs::read(dir.join("a/b.txt")).expect("read"), b"onetwo");
        }
        assert!(!path.exists());
    }

    #[test]
    fn two_dirs_with_the_same_label_do_not_collide() {
        let a = TestDir::new("same");
        let b = TestDir::new("same");
        assert_ne!(a.path(), b.path());
    }
}
