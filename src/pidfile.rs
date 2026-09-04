use std::io;
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const FILE_SHARE_READ: u32 = 0x00000001;

pub struct PidFile {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl PidFile {
    pub fn write(path: &Path) -> io::Result<PidFile> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            // Read sharing lets tools inspect the PID while the owner blocks replacement and deletion.
            // https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew#parameters
            .share_mode(FILE_SHARE_READ)
            .open(path)?;
        if let Err(error) = writeln!(file, "{}", std::process::id()) {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
        Ok(PidFile {
            path: path.to_path_buf(),
            file: Some(file),
        })
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn test_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rapira-pidfile-test-{}-{unique}-{sequence}.pid",
            std::process::id()
        ))
    }

    #[test]
    fn a_second_owner_cannot_replace_or_remove_the_first_pidfile() {
        let path = test_path();
        let first = PidFile::write(&path).unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents, format!("{}\n", std::process::id()));

        let error = PidFile::write(&path).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);

        drop(first);
        assert!(!path.exists());
    }
}
