use crate::error::JanitorError;
use std::fs;
use std::path::{Path, PathBuf};

pub fn find_kernel_dir(module_dir: &Path) -> Result<PathBuf, JanitorError> {
    if !module_dir.exists() {
        return Err(JanitorError::NoKernelDir(module_dir.to_path_buf()));
    }
    let mut entries = fs::read_dir(module_dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect::<Vec<_>>();

    // Sort to get a deterministic order (e.g., latest version).
    entries.sort();

    // In the Live ISO there should be just one kernel installed, but if there are more,
    // we take the last one, which is likely the newest version.
    entries
        .pop()
        .ok_or_else(|| JanitorError::NoKernelDir(module_dir.to_path_buf()))
}

/// Recursively walk a directory and return a list of all files.
pub fn walk_dir(dir: &Path) -> Result<Vec<PathBuf>, JanitorError> {
    let mut files = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                files.extend(walk_dir(&path)?);
            } else {
                files.push(path);
            }
        }
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::JanitorError;
    use std::fs;

    #[test]
    fn test_find_kernel_dir_success() {
        let temp_dir = tempfile::tempdir().unwrap();
        let modules_dir = temp_dir.path();
        let kernel_dir_name = "6.1.0-test";
        let kernel_dir = modules_dir.join(kernel_dir_name);
        fs::create_dir(&kernel_dir).unwrap();

        let found_dir = find_kernel_dir(modules_dir).unwrap();
        assert_eq!(found_dir, kernel_dir);
    }

    #[test]
    fn test_find_kernel_dir_no_subdirectories() {
        let temp_dir = tempfile::tempdir().unwrap();
        let modules_dir = temp_dir.path();

        let result = find_kernel_dir(modules_dir);
        assert!(matches!(result, Err(JanitorError::NoKernelDir(_))));
    }

    #[test]
    fn test_find_kernel_dir_non_existent_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let modules_dir = temp_dir.path().join("non_existent");

        let result = find_kernel_dir(&modules_dir);
        assert!(matches!(result, Err(JanitorError::NoKernelDir(_))));
    }

    #[test]
    fn test_find_kernel_dir_prefers_last_entry() {
        let temp_dir = tempfile::tempdir().unwrap();
        let modules_dir = temp_dir.path();
        fs::create_dir(modules_dir.join("6.0.0-test")).unwrap();
        fs::create_dir(modules_dir.join("6.1.0-test")).unwrap(); // This should be picked due to sorting

        assert!(find_kernel_dir(modules_dir)
            .unwrap()
            .ends_with("6.1.0-test"));
    }
}
