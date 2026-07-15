//! Minimal rotating file log for YumeDock. Writes to
//! `%LOCALAPPDATA%\YumeDock\yumedock.log`, capped at 512 KB (rotated once
//! to `.1`). No external dependency.
//!
//! Intended for rare error paths only — `write()` opens and closes the file
//! per call. The path is resolved once under a `Mutex`, but the writes
//! themselves are unsynchronized, so this is safe for the current callers
//! (both on the single UI thread) and must not be called from worker threads
//! without adding write-side locking.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

use crate::config::app_data_dir;

const MAX_BYTES: u64 = 512 * 1024;

static LOG: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn log_path() -> Option<std::path::PathBuf> {
    let mut slot = LOG.lock().ok()?;
    if let Some(path) = slot.as_ref() {
        return Some(path.clone());
    }
    let dir = app_data_dir().ok()?;
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("yumedock.log");
    *slot = Some(path.clone());
    Some(path)
}

pub fn write(level: &str, message: &str) {
    let Some(path) = log_path() else {
        return;
    };
    // Rotate if too large. Best-effort: ignore errors.
    if let Ok(meta) = fs::metadata(&path) {
        if meta.len() > MAX_BYTES {
            let _ = fs::rename(&path, path.with_extension("log.1"));
        }
    }
    let line = format!("[{}] {}\n", level, message);
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

#[macro_export]
macro_rules! yume_warn {
    ($($arg:tt)*) => {
        $crate::log::write("WARN", &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! yume_err {
    ($($arg:tt)*) => {
        $crate::log::write("ERROR", &format!($($arg)*))
    };
}
