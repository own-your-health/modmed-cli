//! Owner-only files, written whole: the directory is created with mode
//! `0700`, and the file goes to a temporary name with mode `0600` and is
//! renamed over the target (`pk_cli_core::private_file`, the family's one
//! owner-only writer), so a reader never sees half a file.

use std::path::Path;

use pk_cli_core::private_file::{self, Replace};
use pk_cli_core::CliError;

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), CliError> {
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    private_file::create_dir_all(dir)
        .map_err(|e| CliError::Other(format!("creating {}: {e}", dir.display())))?;
    private_file::write(path, bytes, Replace::Always)
        .map_err(|e| CliError::Other(format!("writing {}: {e}", path.display())))
}

/// A directory only the owner can read (`0700`), and its parents.
pub fn create_private_dir(dir: &Path) -> Result<(), CliError> {
    private_file::create_dir_all(dir)
        .map_err(|e| CliError::Other(format!("creating {}: {e}", dir.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_whole_and_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("state.json");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_at_the_target_is_replaced_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let elsewhere = dir.path().join("elsewhere.txt");
        std::fs::write(&elsewhere, b"untouched").unwrap();
        let path = dir.path().join("state.json");
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"untouched");
        assert!(!std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}
