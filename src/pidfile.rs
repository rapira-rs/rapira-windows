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
