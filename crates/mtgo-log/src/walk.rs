//! The one bounded folder walk behind every file search under MTGO's folders
//! (logs, deck settings, game logs, history files).
//!
//! Linked folders are never entered: every Wine prefix has `dosdevices/z:`
//! pointing at the host's `/`, so following links would walk the whole file
//! system on every search. A linked file still counts. The folder the walk
//! starts in may itself be a link (`read_dir` resolves it).

use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

/// Walk `dir` for files, at most `max_depth` folder levels: `dir` itself is
/// level 0, so files in `dir` are seen when `max_depth` is at least 1, and
/// nothing is seen when it is 0. `wanted` picks files by path before they
/// are looked at, so only wanted files cost a `metadata` call; `on_file` gets
/// each wanted path that is a file (following a link) with its metadata.
/// Folders that cannot be read are skipped.
pub(crate) fn walk_files(
    dir: &Path,
    max_depth: usize,
    wanted: &dyn Fn(&Path) -> bool,
    on_file: &mut dyn FnMut(PathBuf, Metadata),
) {
    walk_level(dir, 0, max_depth, wanted, on_file);
}

fn walk_level(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    wanted: &dyn Fn(&Path) -> bool,
    on_file: &mut dyn FnMut(PathBuf, Metadata),
) {
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `file_type` does not follow links: a linked folder is not a folder
        // here, so it is never entered.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk_level(&path, depth + 1, max_depth, wanted, on_file);
            continue;
        }
        if !wanted(&path) {
            continue;
        }
        // `metadata` follows a link, so a linked file counts.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        if meta.is_file() {
            on_file(path, meta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture_tools::TestDir;

    fn files_under(dir: &Path, max_depth: usize) -> Vec<PathBuf> {
        let mut found = Vec::new();
        walk_files(dir, max_depth, &|_| true, &mut |path, _| found.push(path));
        found.sort();
        found
    }

    #[test]
    fn the_root_is_level_zero_and_the_limit_is_exclusive() {
        let dir = TestDir::new("walk-depth");
        let top = dir.write("top.txt", b"x");
        let one = dir.write("a/one.txt", b"x");
        let two = dir.write("a/b/two.txt", b"x");

        assert_eq!(files_under(dir.path(), 0), Vec::<PathBuf>::new());
        assert_eq!(files_under(dir.path(), 1), vec![top.clone()]);
        assert_eq!(files_under(dir.path(), 2), vec![one.clone(), top.clone()]);
        assert_eq!(files_under(dir.path(), 3), vec![two, one, top]);
    }

    #[test]
    fn only_wanted_files_reach_the_callback() {
        let dir = TestDir::new("walk-wanted");
        let keep = dir.write("a/keep.txt", b"x");
        dir.write("a/skip.txt", b"x");

        let mut found = Vec::new();
        walk_files(
            dir.path(),
            5,
            &|path| path.ends_with("keep.txt"),
            &mut |path, meta| {
                assert!(meta.is_file());
                found.push(path);
            },
        );
        assert_eq!(found, vec![keep]);
    }

    #[cfg(unix)]
    fn link_folder(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(unix)]
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn link_folder(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[cfg(windows)]
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    #[test]
    fn a_linked_folder_is_never_entered_but_a_linked_file_counts() {
        let dir = TestDir::new("walk-links");
        let outside = TestDir::new("walk-links-outside");
        outside.write("hidden/far.txt", b"x");
        let target = dir.write("real/near.txt", b"x");
        let root = dir.join("root");
        fs::create_dir_all(&root).unwrap();
        // Creating links can fail (Windows without developer mode): then
        // there is nothing to check here.
        if link_folder(outside.path(), &root.join("away")).is_err()
            || link_file(&target, &root.join("near.txt")).is_err()
        {
            return;
        }

        assert_eq!(files_under(&root, 5), vec![root.join("near.txt")]);
    }
}
