//! Push-based output notification for the selected tmux session.
//!
//! `tmux pipe-pane` tees the selected pane's output into a FIFO. A
//! background thread drains the FIFO (discarding the bytes) and sets a
//! dirty flag. The main loop captures the pane only when the flag is set,
//! instead of spawning a capture subprocess on every frame.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct PipeWatch {
    dirty: Arc<AtomicBool>,
    fifo_path: PathBuf,
}

impl PipeWatch {
    /// Create the FIFO and start the drain thread. Returns None when the
    /// FIFO cannot be set up; callers fall back to per-frame capture.
    pub fn start() -> Option<Self> {
        let dir = crate::hooks::state_dir();
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("output.fifo");

        if !create_fifo(&path) {
            return None;
        }

        // Open read+write: the open never blocks waiting for a writer, and
        // the FIFO never reaches EOF while pipe commands come and go.
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .ok()?;

        let dirty = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&dirty);
        std::thread::Builder::new()
            .name("pipe-watch".into())
            .spawn(move || {
                use std::io::Read;
                let mut buf = [0u8; 4096];
                loop {
                    match file.read(&mut buf) {
                        Ok(0) => break,
                        Ok(_) => flag.store(true, Ordering::Release),
                        Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(_) => break,
                    }
                }
            })
            .ok()?;

        Some(Self {
            dirty,
            fifo_path: path,
        })
    }

    pub fn fifo_path(&self) -> &Path {
        &self.fifo_path
    }

    /// Return and reset the dirty flag.
    pub fn take_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }
}

/// Ensure a FIFO exists at `path`, replacing a stale regular file if needed.
fn create_fifo(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::FileTypeExt;

    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    for _ in 0..2 {
        if unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) } == 0 {
            return true;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return false;
        }
        // Path exists: reuse it if it is already a FIFO, otherwise remove
        // the stale file and retry once.
        match std::fs::metadata(path) {
            Ok(meta) if meta.file_type().is_fifo() => return true,
            Ok(_) => {
                if std::fs::remove_file(path).is_err() {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
    false
}
