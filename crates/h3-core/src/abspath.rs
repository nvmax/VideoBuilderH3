//! `os.path.abspath`: a full path with `.` and `..` resolved by the text of the path alone. The standard `std::path::absolute` resolves `..` on
//! Windows and keeps it on Linux and macOS (a `..` can only be resolved there by looking at the disk, because of symbolic links). Every "is this
//! inside that folder" check of the app compares path components, so `folder/../other` has to become `other` first or it passes as inside `folder`.

use std::io;
use std::path::{Component, Path, PathBuf};

/// The path with `.` removed and each `..` resolved against the folder before it. A `..` at the root stays at the root, and a relative path keeps
/// the `..` it cannot resolve. A trailing separator is dropped.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match out.components().next_back() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                _ => out.push(".."),
            },
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `std::path::absolute` with `..` resolved on every system. Fails the way the standard function does, for an empty path.
pub fn absolute(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    std::path::absolute(path).map(|full| normalize(&full))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(text: &str) -> PathBuf {
        normalize(Path::new(text))
    }

    #[test]
    fn dots_are_resolved_against_the_folders_before_them() {
        assert_eq!(normalized("a/b/../c"), Path::new("a").join("c"));
        assert_eq!(normalized("a/./b/"), Path::new("a").join("b"));
        assert_eq!(normalized("a/b/../../c"), Path::new("c"));
    }

    #[test]
    fn a_relative_path_keeps_the_dots_it_cannot_resolve() {
        assert_eq!(normalized("../a"), Path::new("..").join("a"));
        assert_eq!(normalized("a/../../b"), Path::new("..").join("b"));
    }

    #[test]
    fn an_absolute_path_cannot_climb_above_its_root() {
        let here = std::env::current_dir().unwrap();
        let root: PathBuf = here.components().take_while(|c| matches!(c, Component::Prefix(_) | Component::RootDir)).collect();
        assert_eq!(normalize(&root.join("..").join("..").join("tmp")), root.join("tmp"));
    }

    #[test]
    fn a_folder_that_climbs_out_is_not_inside_its_parent() {
        let parent = absolute(Path::new("out")).unwrap();
        let child = absolute(parent.join("..")).unwrap();
        assert!(!child.starts_with(&parent));
        assert_eq!(absolute(parent.join("sub").join("..").join("x")).unwrap(), parent.join("x"));
    }

    #[test]
    fn an_empty_path_is_an_error() {
        assert!(absolute(Path::new("")).is_err());
    }
}
