// What a shop sends when something goes wrong, in one zip in Documents\Tiquet:
// the activity log, the settings, the printers, the Windows and Tiquet
// versions, the database's health and where the AEAT's answers stand. Never
// the database itself; the certificate thumbprint and the backup folder are
// taken out of the settings.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::Local;
use serde_json::{json, Map, Value};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use crate::error::Result;
use crate::state::AppState;
use crate::{log, os, paths, printer, settings, tickets};

// the log rotates at 1 MB; a file that grew past this only sends its end
const LOG_MAX: u64 = 2_000_000;

pub fn save(st: &AppState) -> Result<PathBuf> {
    let health = st.health();
    let (s, last, sends) = {
        let conn = st.conn();
        let mut stmt = conn.prepare("SELECT state, COUNT(*) FROM fiscal_sends GROUP BY state")?;
        let sends: Map<String, Value> = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, json!(r.get::<_, i64>(1)?))))?
            .collect::<rusqlite::Result<_>>()?;
        (settings::load(&conn)?, tickets::last_print(&conn)?, sends)
    };
    let mut shown = serde_json::to_value(&s)?;
    if let Some(o) = shown.as_object_mut() {
        o.remove("cert");
        o.remove("backupDir");
    }
    let state = json!({
        "tiquet": env!("CARGO_PKG_VERSION"),
        "windows": os::windows_version(),
        "at": tickets::now(),
        "dataDir": st.data_dir,
        "db": health,
        "fiscalSends": sends,
    });
    let printers = json!({
        "printers": printer::list(),
        "status": printer::status(&st.sink, &s),
        "lastPrint": last,
    });

    let dir = paths::documents();
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("diagnostico-{}.zip", Local::now().format("%Y%m%d-%H%M")));
    let logs: Vec<PathBuf> = log::path().map(|l| vec![l.with_extension("log.1"), l]).unwrap_or_default();
    write(&path, &logs, &[("estado.json", state), ("ajustes.json", shown), ("impresoras.json", printers)])?;
    Ok(path)
}

fn write(path: &Path, logs: &[PathBuf], files: &[(&str, Value)]) -> Result<()> {
    let mut zip = ZipWriter::new(File::create(path)?);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for f in logs {
        if let Some(bytes) = tail(f) {
            zip.start_file(f.file_name().unwrap_or_default().to_string_lossy(), opts)?;
            zip.write_all(&bytes)?;
        }
    }
    for (name, v) in files {
        zip.start_file(*name, opts)?;
        zip.write_all(serde_json::to_string_pretty(v)?.as_bytes())?;
    }
    zip.finish()?;
    Ok(())
}

fn tail(path: &Path) -> Option<Vec<u8>> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(LOG_MAX))).ok()?;
    let mut out = Vec::new();
    f.read_to_end(&mut out).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PrintSink;

    fn tmp(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("tiquet-diag-{tag}-{}-{n}", std::process::id()))
    }

    fn entry(zip: &mut zip::ZipArchive<File>, name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
        out
    }

    #[test]
    fn both_logs_go_in_and_an_oversized_one_only_by_its_end() {
        let dir = tmp("logs");
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("tiquet.log");
        fs::write(&log, "2026-10-18 09:00:00 INFO  Tiquet starting\r\n").unwrap();
        let mut old = vec![b'x'; 3_000_000];
        old.extend_from_slice(b"last line\r\n");
        fs::write(log.with_extension("log.1"), &old).unwrap();

        let path = dir.join("d.zip");
        write(&path, &[log.with_extension("log.1"), log.clone(), dir.join("missing.log")], &[("estado.json", json!({ "a": 1 }))]).unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        assert_eq!(zip.len(), 3);
        assert_eq!(entry(&mut zip, "tiquet.log"), fs::read(&log).unwrap());
        let kept = entry(&mut zip, "tiquet.log.1");
        assert_eq!(kept.len() as u64, LOG_MAX);
        assert!(kept.ends_with(b"last line\r\n"));
        assert!(fs::metadata(&path).unwrap().len() < 100_000);
    }

    #[test]
    fn the_settings_go_without_the_certificate_or_the_backup_folder() {
        static DOCS: std::sync::Once = std::sync::Once::new();
        DOCS.call_once(|| std::env::set_var("TIQUET_DOCUMENTS", std::env::temp_dir().join("tiquet-test-documents")));
        let dir = tmp("save");
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        let s = settings::Settings {
            shop_name: "Mi Tienda Móvil".into(),
            cert: "AB".repeat(20),
            backup_dir: "E:\\Copias".into(),
            ..settings::Settings::default()
        };
        settings::put(&st.conn(), "settings", &serde_json::to_string(&s).unwrap()).unwrap();

        let path = save(&st).unwrap();
        assert!(path.starts_with(paths::documents()));
        let mut zip = zip::ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut read = |name: &str| serde_json::from_slice::<Value>(&entry(&mut zip, name)).unwrap();
        let shown = read("ajustes.json");
        assert_eq!(shown["shopName"], "Mi Tienda Móvil");
        assert!(shown.get("cert").is_none() && shown.get("backupDir").is_none());
        let state = read("estado.json");
        assert_eq!(state["tiquet"], env!("CARGO_PKG_VERSION"));
        assert_eq!(state["db"]["integrity"], "ok");
        assert!(state["fiscalSends"].is_object());
        assert!(state["windows"].as_str().is_some_and(|w| cfg!(not(windows)) || w.starts_with("Windows")));
        assert!(read("impresoras.json")["printers"].is_array());
    }
}
