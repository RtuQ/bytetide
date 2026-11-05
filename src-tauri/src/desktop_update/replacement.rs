use std::{fs, path::Path};

pub(super) fn replace(target: &Path, staged: &Path, backup: &Path) -> Result<(), String> {
    fs::rename(target, backup).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(staged, target) {
        fs::rename(backup, target).map_err(|rollback| {
            format!(
                "replace: {e}; rollback: {rollback}; backup: {}",
                backup.display()
            )
        })?;
        return Err(e.to_string());
    }
    Ok(())
}
pub(super) fn rollback(target: &Path, backup: &Path) -> Result<(), String> {
    fs::remove_file(target).map_err(|e| format!("{e}; backup: {}", backup.display()))?;
    fs::rename(backup, target).map_err(|e| format!("{e}; backup: {}", backup.display()))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_replacement_restores_original() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        let backup = dir.path().join("backup");
        fs::write(&target, b"original").unwrap();
        assert!(replace(&target, &dir.path().join("missing"), &backup).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert!(!backup.exists());
    }
    #[test]
    fn failed_startup_rolls_back_installed_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        let staged = dir.path().join("staged");
        let backup = dir.path().join("backup");
        fs::write(&target, b"original").unwrap();
        fs::write(&staged, b"update").unwrap();
        replace(&target, &staged, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"update");
        rollback(&target, &backup).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"original");
    }
    #[test]
    fn backup_is_preserved_when_rollback_cannot_remove_target() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("app");
        let backup = dir.path().join("backup");
        fs::create_dir(&target).unwrap();
        fs::write(&backup, b"original").unwrap();
        assert!(rollback(&target, &backup).is_err());
        assert_eq!(fs::read(&backup).unwrap(), b"original");
    }
}
