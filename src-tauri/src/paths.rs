use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::db;
use crate::error::{fail, Result};
use crate::log;

// Not %LOCALAPPDATA%\es.tiquet.caja: the uninstaller's "delete application
// data" box wipes that folder, and the sales records have to outlive it.
pub fn data_dir() -> Result<PathBuf> {
    match dirs::data_local_dir() {
        Some(d) => Ok(d.join("TiquetData")),
        None => fail("E_DATA_DIR"),
    }
}

// TIQUET_DOCUMENTS moves exports and default backups elsewhere; the dev
// server and the tests use it to keep out of the real Documents folder
pub fn documents() -> PathBuf {
    if let Some(d) = std::env::var_os("TIQUET_DOCUMENTS").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    match dirs::document_dir() {
        Some(d) => d.join("Tiquet"),
        None => data_dir().unwrap_or_else(|_| PathBuf::from(".")).join("Documentos"),
    }
}

// 1.0 kept the database where the uninstaller can delete it. The first start
// of a newer version copies it out, checks the copy, and only then renames
// the old file, so anything going wrong leaves the old one where it was.
pub fn adopt(old_dir: &Path, new_dir: &Path) -> Result<bool> {
    let (old, new) = (old_dir.join("tiquet.db"), new_dir.join("tiquet.db"));
    if old_dir == new_dir || new.exists() || !old.exists() {
        return Ok(false);
    }
    fs::create_dir_all(new_dir)?;
    let part = new_dir.join("tiquet.db.part");
    let _ = fs::remove_file(&part);
    db::copy_to(&Connection::open(&old)?, &part)?;
    db::check(&Connection::open(&part)?)?;
    fs::rename(&part, &new)?;
    let kept = old_dir.join("tiquet.db.moved");
    if let Err(e) = fs::rename(&old, &kept) {
        log::warn(format!("old database left in place at {}: {e}", old.display()));
    }
    for f in ["tiquet.db-wal", "tiquet.db-shm"] {
        let _ = fs::remove_file(old_dir.join(f));
    }
    log::info(format!("database moved from {} to {}", old.display(), new.display()));
    Ok(true)
}

// Highest number handed out per series, kept next to the database but outside
// it. A restored backup knows nothing of the tickets printed after it was
// taken; this file does, so their numbers are never issued a second time.
const MARKS: &str = "numeracion.json";

pub fn read_marks(dir: &Path) -> BTreeMap<String, i64> {
    fs::read_to_string(dir.join(MARKS))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn write_marks(dir: &Path, marks: &BTreeMap<String, i64>) {
    let mut all = read_marks(dir);
    let mut changed = false;
    for (series, seq) in marks {
        let e = all.entry(series.clone()).or_insert(0);
        if *seq > *e {
            *e = *seq;
            changed = true;
        }
    }
    if !changed {
        return;
    }
    let tmp = dir.join(format!("{MARKS}.tmp"));
    let wrote = serde_json::to_string_pretty(&all)
        .map_err(|e| e.to_string())
        .and_then(|s| fs::write(&tmp, s).map_err(|e| e.to_string()))
        .and_then(|_| fs::rename(&tmp, dir.join(MARKS)).map_err(|e| e.to_string()));
    if let Err(e) = wrote {
        log::error(format!("numbering marks not saved: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("tiquet-paths-{tag}-{n}"))
    }

    #[test]
    fn an_old_database_is_copied_out_checked_and_then_retired() {
        let (old, new) = (tmp("old"), tmp("new"));
        {
            let c = db::open(&old.join("tiquet.db")).unwrap();
            c.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        }
        assert!(adopt(&old, &new).unwrap());
        assert!(old.join("tiquet.db.moved").exists() && !old.join("tiquet.db").exists());
        let c = db::open(&new.join("tiquet.db")).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // second start: nothing to do
        assert!(!adopt(&old, &new).unwrap());
    }

    #[test]
    fn marks_only_ever_go_up() {
        let dir = tmp("marks");
        fs::create_dir_all(&dir).unwrap();
        write_marks(&dir, &BTreeMap::from([("T2026".to_string(), 40), ("TR2026".to_string(), 2)]));
        write_marks(&dir, &BTreeMap::from([("T2026".to_string(), 12)]));
        let m = read_marks(&dir);
        assert_eq!((m["T2026"], m["TR2026"]), (40, 2));
    }
}
