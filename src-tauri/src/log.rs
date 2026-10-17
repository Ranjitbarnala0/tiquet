// A plain text log next to the database: what was sold, printed, backed up
// and what failed, so a support call can be answered from the file instead
// of from memory. Rotates at 1 MB, keeping one previous file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_BYTES: u64 = 1_000_000;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init(dir: &Path) {
    let _ = fs::create_dir_all(dir);
    *FILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir.join("tiquet.log"));
}

pub fn path() -> Option<PathBuf> {
    FILE.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn write(level: &str, msg: &str) {
    let guard = FILE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(path) = guard.as_ref() else { return };
    if fs::metadata(path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
    let msg: String = msg.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).take(2000).collect();
    let line = format!("{} {level} {msg}\r\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

pub fn info(msg: impl AsRef<str>) {
    write("INFO ", msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    write("WARN ", msg.as_ref());
}

pub fn error(msg: impl AsRef<str>) {
    write("ERROR", msg.as_ref());
}
