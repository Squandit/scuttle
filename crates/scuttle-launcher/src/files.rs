//! A simple in-memory index of the user's files for the launcher.
//!
//! The index is a flat list of paths under a few chosen folders. It is
//! built breadth-first so that, when a folder tree is larger than the
//! limit, the files nearest the top are the ones kept.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};

/// Folders that are never worth indexing.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__pycache__",
    "venv",
    "vendor",
    "build",
    "dist",
];

/// How deep below each root the index goes.
const MAX_DEPTH: usize = 6;

/// One indexed file or folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: PathBuf,
    /// The final path component, which is what queries match against.
    pub name: String,
    pub is_dir: bool,
}

/// Turns the configured roots into absolute, existing directories.
/// Relative roots are taken as relative to `home`.
pub fn resolve_roots(roots: &[PathBuf], home: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = roots
        .iter()
        .map(|r| {
            if r.is_absolute() {
                r.clone()
            } else {
                home.join(r)
            }
        })
        .filter(|p| p.is_dir())
        .collect();
    out.dedup();
    out
}

/// Walks `roots` and returns up to `limit` entries. Hidden files, and
/// build and dependency folders, are skipped. Symlinks are listed but
/// never followed, so a link cycle cannot hang the walk.
pub fn build_index(roots: &[PathBuf], limit: usize) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    let mut queue: VecDeque<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();

    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<_> = read.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        for child in children {
            if entries.len() >= limit {
                return entries;
            }
            let name = child.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                continue;
            }
            let Ok(file_type) = child.file_type() else {
                continue;
            };
            let is_dir = file_type.is_dir();
            if is_dir && SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let path = child.path();
            if is_dir && depth < MAX_DEPTH {
                queue.push_back((path.clone(), depth + 1));
            }
            entries.push(FileEntry { path, name, is_dir });
        }
    }
    entries
}

/// Shortens a path for display by replacing the home directory with `~`.
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("scuttle-files-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Documents/work/deep")).unwrap();
        fs::create_dir_all(root.join("Documents/.secret")).unwrap();
        fs::create_dir_all(root.join("Documents/project/node_modules/pkg")).unwrap();
        fs::write(root.join("Documents/report.pdf"), "").unwrap();
        fs::write(root.join("Documents/.hidden"), "").unwrap();
        fs::write(root.join("Documents/.secret/key"), "").unwrap();
        fs::write(root.join("Documents/work/notes.md"), "").unwrap();
        fs::write(root.join("Documents/work/deep/buried.txt"), "").unwrap();
        fs::write(root.join("Documents/project/node_modules/pkg/index.js"), "").unwrap();
        fs::write(root.join("Documents/project/main.rs"), "").unwrap();
        root
    }

    fn names(entries: &[FileEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn indexes_files_and_folders_but_skips_hidden_and_dependency_dirs() {
        let root = tree("skip");
        let index = build_index(&[root.join("Documents")], 1000);
        let found = names(&index);
        for expected in [
            "report.pdf",
            "work",
            "notes.md",
            "buried.txt",
            "project",
            "main.rs",
        ] {
            assert!(found.contains(&expected), "missing {expected} in {found:?}");
        }
        for unwanted in [".hidden", ".secret", "key", "node_modules", "index.js"] {
            assert!(!found.contains(&unwanted), "should skip {unwanted}");
        }
        assert!(index.iter().any(|e| e.name == "work" && e.is_dir));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn shallow_entries_come_first_when_the_limit_bites() {
        let root = tree("limit");
        let index = build_index(&[root.join("Documents")], 3);
        assert_eq!(names(&index), vec!["project", "report.pdf", "work"]);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn symlink_cycles_do_not_hang() {
        let root = tree("cycle");
        std::os::unix::fs::symlink(root.join("Documents"), root.join("Documents/work/loop"))
            .unwrap();
        let index = build_index(&[root.join("Documents")], 10_000);
        assert!(index.iter().any(|e| e.name == "loop" && !e.is_dir));
        assert!(index.len() < 50);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn roots_resolve_against_home_and_missing_ones_are_dropped() {
        let root = tree("roots");
        let roots = vec![
            PathBuf::from("Documents"),
            PathBuf::from("Nope"),
            root.join("Documents/work"),
        ];
        assert_eq!(
            resolve_roots(&roots, &root),
            vec![root.join("Documents"), root.join("Documents/work")]
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn missing_root_gives_an_empty_index() {
        assert!(build_index(&[PathBuf::from("/nonexistent/scuttle")], 10).is_empty());
    }

    #[test]
    fn display_path_uses_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(
            display_path(Path::new("/home/me/Documents/a.txt"), home),
            "~/Documents/a.txt"
        );
        assert_eq!(display_path(Path::new("/home/me"), home), "~");
        assert_eq!(display_path(Path::new("/etc/hosts"), home), "/etc/hosts");
    }
}
