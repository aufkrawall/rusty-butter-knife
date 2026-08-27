//! Windows-specific no-follow filesystem helpers (audit findings FS-01 /
//! REPORT-02). Pure std — no FFI here.

#![deny(unsafe_code)]

use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Raw Win32 attribute bit for any reparse point (symlink OR junction OR
/// mount point). We must never recurse THROUGH these.
pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// No-follow path kind as observed through `symlink_metadata`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// Confirmed absent (NotFound-family).
    Missing,
    /// Regular file.
    File,
    /// Real directory WITHOUT a reparse attribute.
    Directory,
    /// Any reparse point (symlink/junction/mount): removable as an entry,
    /// but its target must NEVER be traversed.
    Reparse,
}

/// No-follow kind probe distinguishing genuine absence from inaccessible
/// metadata (`Path::exists()` folds both together — audit finding REPORT-02:
/// a permission-denied stat used to masquerade as "successfully deleted").
pub fn path_kind_no_follow(p: &Path) -> Result<PathKind, io::Error> {
    match std::fs::symlink_metadata(p) {
        Ok(meta) => {
            if attrs_are_reparse(meta.file_attributes()) {
                Ok(PathKind::Reparse)
            } else if meta.is_dir() {
                Ok(PathKind::Directory)
            } else {
                Ok(PathKind::File)
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(PathKind::Missing),
        Err(e) => Err(e),
    }
}

/// True when raw directory-entry attributes mark a reparse point
/// (`DirEntry::metadata` does not follow links). Callers must never descend
/// below such an entry.
pub fn attrs_are_reparse(file_attributes: u32) -> bool {
    file_attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

enum Work {
    /// Emit this path itself (kind resolved at pop time).
    Emit(PathBuf),
    /// Enumerate this real directory and push children below its own Emit.
    Expand(PathBuf),
}

/// Visit every path of the tree rooted at `root` in postorder (children
/// strictly before parents, root last), NEVER descending through reparse
/// points, using an explicit stack — abort-aware: return `false` from the
/// callback to stop early. The callback also receives the root (with its
/// kind; `Missing` included so callers decide what absence means).
pub fn for_each_postorder(root: &Path, visit: &mut dyn FnMut(&Path, PathKind) -> bool) -> bool {
    // The ROOT must be probed with no-follow semantics BEFORE any expansion:
    // read_dir on a junction/symlink root happily enumerates the TARGET's
    // children, so pushing Work::Expand for a reparse root would traverse
    // straight through the FS-01 boundary (schedule_delete_tree would then
    // schedule the target's contents for deletion). A reparse, file, or
    // unreadable root is emitted as a single entry instead.
    match path_kind_no_follow(root) {
        Ok(PathKind::Reparse) => return visit(root, PathKind::Reparse),
        Ok(PathKind::File) => return visit(root, PathKind::File),
        Err(_) => return visit(root, PathKind::Missing),
        Ok(PathKind::Missing) | Ok(PathKind::Directory) => {}
    }
    let mut stack: Vec<Work> = vec![Work::Expand(root.to_path_buf())];

    while let Some(work) = stack.pop() {
        match work {
            Work::Emit(path) => {
                let kind = path_kind_no_follow(&path).unwrap_or(PathKind::Missing);
                if !visit(&path, kind) {
                    return false;
                }
            }
            Work::Expand(dir) => {
                // The directory is emitted only after all its children were
                // processed: its Emit frame sits below every child frame.
                stack.push(Work::Emit(dir.clone()));
                let entries = std::fs::read_dir(&dir);
                if let Ok(entries) = entries {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let attrs = entry
                            .metadata()
                            .map(|m| m.file_attributes())
                            .unwrap_or(FILE_ATTRIBUTE_REPARSE_POINT);
                        if attrs_are_reparse(attrs) {
                            // Reparse entries are emitted directly; their
                            // targets are never traversed (FS-01).
                            if !visit(&path, PathKind::Reparse) {
                                return false;
                            }
                        } else if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            stack.push(Work::Expand(path));
                        } else if !visit(&path, PathKind::File) {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let base = std::env::temp_dir();
            let unique = format!(
                "gpd-fsutil-{tag}-{}-{}",
                std::process::id(),
                crate::util::now_unique_suffix()
            );
            let p = base.join(unique);
            std::fs::create_dir_all(&p).expect("create temp dir");
            TempDir(p)
        }
        fn child(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn assert_kind(path: &Path, expected: PathKind) {
        match path_kind_no_follow(path) {
            Ok(k) => assert_eq!(k, expected, "kind of {}", path.display()),
            Err(e) => panic!("unexpected stat error for {}: {e}", path.display()),
        }
    }

    #[test]
    fn kinds_distinguish_missing_file_dir() {
        let t = TempDir::new("kinds");
        assert_kind(&t.child("nope"), PathKind::Missing);
        std::fs::write(t.child("f.txt"), b"x").unwrap();
        assert_kind(&t.child("f.txt"), PathKind::File);
        assert_kind(&t.0, PathKind::Directory);
    }

    #[test]
    fn junction_is_reparse_and_never_traversed() {
        let t = TempDir::new("junction");
        // Real payload tree hidden behind a junction.
        let secret = t.child("secret");
        std::fs::create_dir_all(secret.join("deeper")).unwrap();
        std::fs::write(secret.join("deeper").join("marker.txt"), b"x").unwrap();
        let link = t.child("link");
        // Junction creation is done via mklink so no unstable std API is
        // needed; if even that fails (FAT/exotic FS), skip rather than flake.
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&secret)
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if !made {
            return;
        }

        assert_kind(&link, PathKind::Reparse);

        // Postorder walk must see ONLY the junction entry, not its target.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for_each_postorder(&t.0, &mut |p, _k| {
            seen.insert(to_lower(&p.to_string_lossy()));
            true
        });
        assert!(seen.iter().any(|s| s.ends_with("\\link")));
        let link_prefix = to_lower(&format!("{}\\", t.child("link").display()));
        assert!(
            !seen.iter().any(|s| s.starts_with(&link_prefix)),
            "walk crossed the junction target"
        );
    }

    #[test]
    fn junction_root_is_emitted_but_never_expanded() {
        // Regression (FS-01, ROOT case): walking FROM a reparse-point root
        // must emit only the junction entry itself. read_dir would happily
        // enumerate the junction TARGET, and schedule_delete_tree would then
        // schedule every file inside the target for deletion at reboot.
        let t = TempDir::new("junction-root");
        let secret = t.child("secret");
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("payload.txt"), b"x").unwrap();
        let link = t.child("link");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&secret)
            .status()
            .map(|st| st.success())
            .unwrap_or(false);
        if !made {
            return;
        }

        let mut seen: BTreeSet<String> = BTreeSet::new();
        for_each_postorder(&link, &mut |p, _k| {
            seen.insert(to_lower(&p.to_string_lossy()));
            true
        });
        let link_lower = to_lower(&link.to_string_lossy());
        assert_eq!(
            seen.len(),
            1,
            "only the junction entry may be visited, got {seen:?}"
        );
        assert!(seen.iter().any(|s| s == &link_lower));
    }

    #[test]
    fn postorder_emits_children_before_parents() {
        let t = TempDir::new("postorder");
        std::fs::create_dir_all(t.child("a").join("b")).unwrap();
        std::fs::write(t.child("a").join("b").join("leaf.txt"), b"x").unwrap();

        let mut order: Vec<(String, String)> = Vec::new();
        for_each_postorder(&t.child("a"), &mut |p, k| {
            order.push((to_lower(&p.to_string_lossy()), format!("{k:?}")));
            true
        });
        let joined = order
            .iter()
            .map(|(p, _)| p.clone())
            .collect::<Vec<_>>()
            .join("|");
        let b_pos = joined.find("\\a\\b|").expect("b emitted");
        let a_pos = joined.rfind("\\a").expect("a emitted");
        assert!(b_pos < a_pos, "children before parents: {joined}");
        // Leaf files inside deeper trees come before their directory.
        assert!(joined.find("leaf.txt").unwrap() < b_pos);
    }

    use crate::util::to_lower;
}
