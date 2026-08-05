use std::path::{Path, PathBuf};

const FILE_NAME: &str = "direct-capture-verification";
const CONTENT: &[u8] = b"permissions_grant\n";

fn state_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(crate::bundle::user_home_subdirectory())
            .join(FILE_NAME),
    )
}

pub fn record() -> Result<(), String> {
    let path = state_path().ok_or_else(|| "user home directory is unavailable".to_owned())?;
    record_to_path(&path)
}

pub fn was_recorded() -> bool {
    state_path().is_some_and(|path| load_from_path(&path))
}

pub fn clear() -> Result<(), String> {
    let Some(path) = state_path() else {
        return Ok(());
    };
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {}: {error}", path.display())),
    }
}

fn load_from_path(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|content| content == CONTENT)
}

fn record_to_path(path: &Path) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "direct-capture verification path has no parent".to_owned())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    std::fs::write(path, CONTENT).map_err(|error| format!("write {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_round_trips() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        record_to_path(&path).expect("record verification");
        assert!(load_from_path(&path));
    }

    #[test]
    fn unexpected_content_is_not_a_verification() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join(FILE_NAME);
        std::fs::write(&path, b"unexpected").expect("write unexpected content");
        assert!(!load_from_path(&path));
    }
}
