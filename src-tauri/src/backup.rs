use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use rusqlite::Connection;

use crate::error::Result;
use crate::settings::{self, Settings};

const KEEP: usize = 60;

pub fn documents() -> PathBuf {
    dirs::document_dir().unwrap_or_else(std::env::temp_dir).join("Tiquet")
}

pub fn dir(s: &Settings) -> PathBuf {
    match s.backup_dir.trim() {
        "" => documents().join("Copias de seguridad"),
        d => PathBuf::from(d),
    }
}

// VACUUM INTO writes a consistent copy even while the app keeps running
pub fn run(conn: &Connection, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("tiquet-{}.db", Local::now().format("%Y%m%d-%H%M%S")));
    let quoted = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{quoted}'"))?;
    settings::put(conn, "lastBackupAt", &crate::tickets::now())?;
    settings::put(conn, "lastBackupPath", &path.to_string_lossy())?;
    prune(dir);
    Ok(path)
}

fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("tiquet-") && n.ends_with(".db"))
        })
        .collect();
    files.sort();
    let extra = files.len().saturating_sub(KEEP);
    for old in &files[..extra] {
        let _ = fs::remove_file(old);
    }
}

// first login of the day takes a copy; a failure is recorded, never fatal
pub fn auto(conn: &Connection) {
    let Ok(s) = settings::load(conn) else { return };
    if !s.auto_backup {
        return;
    }
    let today = Local::now().format("%Y-%m-%d").to_string();
    let last = settings::get(conn, "lastBackupAt").ok().flatten().unwrap_or_default();
    if last.starts_with(&today) {
        return;
    }
    match run(conn, &dir(&s)) {
        Ok(_) => {
            let _ = conn.execute("DELETE FROM settings WHERE key = 'lastBackupError'", []);
        }
        Err(e) => {
            let _ = settings::put(conn, "lastBackupError", &e.0);
        }
    }
}
