use std::{fs, io, path::{Path, PathBuf}};

use crate::BACKUP_SUFFIX;

/// Lists regular backup files in deterministic path order. Symlinks and non-files are omitted;
/// actual backup parsing remains bounded and validated by `read_backup`.
pub fn list_backup_files(directory: &Path) -> io::Result<Vec<PathBuf>> {
    if !directory.exists() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(BACKUP_SUFFIX))
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use super::list_backup_files;

    #[test]
    fn catalog_is_sorted_and_ignores_non_backup_and_symlink_entries() {
        let directory = tempdir().expect("tempdir");
        let a = directory.path().join("a.vfdlantern-backup.json");
        let b = directory.path().join("b.vfdlantern-backup.json");
        fs::write(&b, b"b").expect("b");
        fs::write(&a, b"a").expect("a");
        fs::write(directory.path().join("notes.txt"), b"ignore").expect("notes");
        symlink(&a, directory.path().join("linked.vfdlantern-backup.json")).expect("symlink");

        assert_eq!(list_backup_files(directory.path()).expect("catalog"), vec![a, b]);
    }
}
