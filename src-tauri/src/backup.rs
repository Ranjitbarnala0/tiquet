use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::{DateTime, Local};
use rusqlite::{Connection, DatabaseName};
use serde::Serialize;

use crate::error::{fail, Result};
use crate::settings::{self, Settings};
use crate::{db, log, paths};

const KEEP: usize = 60;

pub fn documents() -> PathBuf {
    paths::documents()
}

pub fn dir(s: &Settings) -> PathBuf {
    match s.backup_dir.trim() {
        "" => default_dir(),
        d => PathBuf::from(d),
    }
}

pub fn default_dir() -> PathBuf {
    documents().join("Copias de seguridad")
}

// every copy, daily or by hand, leaves its result where Ajustes and the
// admin's sign-in can see it
pub fn run(conn: &Connection, dir: &Path) -> Result<PathBuf> {
    let done = fs::create_dir_all(dir).map_err(Into::into).and_then(|_| {
        let path = db::unique_path(dir, "tiquet");
        db::copy_to(conn, &path).map(|_| path)
    });
    match done {
        Ok(path) => {
            settings::put(conn, "lastBackupAt", &crate::tickets::now())?;
            settings::put(conn, "lastBackupPath", &path.to_string_lossy())?;
            conn.execute("DELETE FROM settings WHERE key = 'lastBackupError'", [])?;
            log::info(format!("backup written to {}", path.display()));
            prune(dir);
            Ok(path)
        }
        Err(e) => {
            log::error(format!("backup to {} failed: {}", dir.display(), e.0));
            let _ = settings::put(conn, "lastBackupError", &format!("{} ({})", e.0, dir.display()));
            Err(e)
        }
    }
}

fn is_backup(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("tiquet-") && n.ends_with(".db"))
}

fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_backup(p)).collect();
    files.sort();
    let extra = files.len().saturating_sub(KEEP);
    for old in &files[..extra] {
        let _ = fs::remove_file(old);
    }
}

// once a day: at the first sign-in and, for a till left open overnight, at
// the first sale. A failure is recorded, never fatal.
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
    let _ = run(conn, &dir(&s));
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct BackupFile {
    pub path: String,
    pub name: String,
    pub bytes: u64,
    pub modified: String,
}

pub fn list(dirs: &[PathBuf]) -> Vec<BackupFile> {
    let mut out: Vec<(SystemTime, BackupFile)> = Vec::new();
    for d in dirs {
        let Ok(entries) = fs::read_dir(d) else { continue };
        for p in entries.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| is_backup(p)) {
            let Ok(meta) = fs::metadata(&p) else { continue };
            let at = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if out.iter().any(|(_, f)| Path::new(&f.path) == p) {
                continue;
            }
            out.push((
                at,
                BackupFile {
                    path: p.to_string_lossy().into(),
                    name: p.file_name().unwrap_or_default().to_string_lossy().into(),
                    bytes: meta.len(),
                    modified: DateTime::<Local>::from(at).format("%Y-%m-%dT%H:%M:%S").to_string(),
                },
            ));
        }
    }
    out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    out.into_iter().map(|(_, f)| f).take(100).collect()
}

// Only files the app itself lists can be restored, so a path typed into the
// request cannot point anywhere else.
pub fn allowed(file: &Path, dirs: &[PathBuf]) -> Result<PathBuf> {
    let listed = list(dirs);
    match listed.iter().find(|f| Path::new(&f.path) == file) {
        Some(f) => Ok(PathBuf::from(&f.path)),
        None => fail("E_BACKUP_INVALID"),
    }
}

// Replaces the live database with a backup, through the open connection.
// The current data is copied aside first, so a wrong pick can be undone by
// restoring that copy.
pub fn restore(conn: &mut Connection, file: &Path, data_dir: &Path) -> Result<PathBuf> {
    db::inspect(file)?;
    let safety = db::unique_path(&data_dir.join("restauraciones"), "tiquet-antes-de-restaurar");
    db::copy_to(conn, &safety)?;
    conn.restore(DatabaseName::Main, file, None::<fn(rusqlite::backup::Progress)>)?;
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    db::check(conn)?;
    db::migrate(conn, None)?;
    log::warn(format!("database restored from {}; previous data kept in {}", file.display(), safety.display()));
    Ok(safety)
}

// The same for when the database cannot even be opened: the damaged file is
// renamed, never deleted, and the backup is copied in its place.
pub fn restore_offline(file: &Path, data_dir: &Path) -> Result<PathBuf> {
    db::inspect(file)?;
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let kept = data_dir.join(format!("tiquet.db.danada-{stamp}"));
    let live = data_dir.join("tiquet.db");
    if live.exists() {
        fs::rename(&live, &kept)?;
    }
    for f in ["tiquet.db-wal", "tiquet.db-shm"] {
        let p = data_dir.join(f);
        if p.exists() {
            let _ = fs::rename(&p, data_dir.join(format!("{f}.danada-{stamp}")));
        }
    }
    fs::copy(file, &live)?;
    log::warn(format!("database replaced by {}; damaged file kept as {}", file.display(), kept.display()));
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let n = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("tiquet-bk-{tag}-{n}"))
    }

    #[test]
    fn backups_in_the_same_second_get_their_own_names_and_failures_are_recorded() {
        let data = tmp("data");
        let conn = db::open(&data.join("tiquet.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        let out = data.join("copias");
        let a = run(&conn, &out).unwrap();
        let b = run(&conn, &out).unwrap();
        assert_ne!(a, b);
        assert_eq!(list(&[out.clone(), out.clone()]).len(), 2);

        // a "folder" that is really a file cannot hold backups
        let blocked = data.join("not-a-dir");
        fs::write(&blocked, "x").unwrap();
        assert!(run(&conn, &blocked).is_err());
        assert!(settings::get(&conn, "lastBackupError").unwrap().is_some());
        run(&conn, &out).unwrap();
        assert!(settings::get(&conn, "lastBackupError").unwrap().is_none());
    }

    #[test]
    fn restore_swaps_the_data_and_keeps_the_old_copy() {
        let data = tmp("restore");
        let mut conn = db::open(&data.join("tiquet.db")).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x')", []).unwrap();
        let out = data.join("copias");
        let file = run(&conn, &out).unwrap();
        conn.execute("INSERT INTO users (name, role, pass_hash) VALUES ('Jordi', 'cashier', 'x')", []).unwrap();

        let ok = allowed(&file, std::slice::from_ref(&out)).unwrap();
        assert!(allowed(&data.join("tiquet.db"), std::slice::from_ref(&out)).is_err());
        let safety = restore(&mut conn, &ok, &data).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
        // the data from before the restore is in the safety copy
        let old = Connection::open(&safety).unwrap();
        let n: i64 = old.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);

        // a file that is not a Tiquet database is refused
        let junk = out.join("tiquet-junk.db");
        fs::write(&junk, "not sqlite").unwrap();
        assert_eq!(restore(&mut conn, &junk, &data).unwrap_err().0, "E_BACKUP_INVALID");
    }
}
