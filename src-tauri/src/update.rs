// Tiquet updates itself from its GitHub releases. latest.json names the newest
// version and carries the installer's signature, which has to match the
// public key built into this copy. The installer comes down in the background
// and waits here until the till is free; on Windows it then runs silently
// (/S), closes Tiquet and starts the new version with the same arguments.

use std::sync::Mutex;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, Updater, UpdaterExt};

use crate::log;

// an installer already downloaded and checked, ready to run
static READY: Mutex<Option<(Update, Vec<u8>)>> = Mutex::new(None);

fn fail(e: impl std::fmt::Display) -> String {
    format!("E_UPDATE:{e}")
}

// TIQUET_UPDATE_URL asks another server instead, for testing; what it offers
// still has to carry this copy's signature
fn updater(app: &AppHandle, wait: Duration) -> Result<Updater, String> {
    let mut b = app.updater_builder().timeout(wait);
    if let Some(url) = std::env::var("TIQUET_UPDATE_URL").ok().filter(|u| !u.is_empty()) {
        b = b.endpoints(vec![url.parse().map_err(fail)?]).map_err(fail)?;
    }
    b.build().map_err(fail)
}

pub async fn check(app: &AppHandle) -> Result<Value, String> {
    let found = updater(app, Duration::from_secs(30))?.check().await.map_err(fail)?;
    Ok(match found {
        Some(u) => json!({ "version": u.version, "notes": u.body }),
        None => Value::Null,
    })
}

async fn download(app: &AppHandle, u: &Update) -> Result<Vec<u8>, String> {
    log::info(format!("downloading Tiquet {}", u.version));
    let (h, mut got) = (app.clone(), 0u64);
    u.download(
        move |n, total| {
            got += n as u64;
            let _ = h.emit("tiquet://update", json!({ "got": got, "total": total }));
        },
        || {},
    )
    .await
    .map_err(|e| {
        log::error(format!("update {} not downloaded: {e}", u.version));
        fail(e)
    })
}

fn ready() -> std::sync::MutexGuard<'static, Option<(Update, Vec<u8>)>> {
    READY.lock().unwrap_or_else(|e| e.into_inner())
}

// Brings the newest version down, signature checked, without touching the
// till: `install` then only has to run it. Null when there is nothing newer.
pub async fn fetch(app: &AppHandle) -> Result<Value, String> {
    let Some(u) = updater(app, Duration::from_secs(600))?.check().await.map_err(fail)? else {
        return Ok(Value::Null);
    };
    let have = ready().as_ref().is_some_and(|(r, _)| r.version == u.version);
    if !have {
        let bytes = download(app, &u).await?;
        *ready() = Some((u.clone(), bytes));
    }
    Ok(json!({ "version": u.version }))
}

// Null when there is nothing newer. Otherwise it only comes back with an
// error: an update that installs ends this process.
pub async fn install(app: &AppHandle) -> Result<Value, String> {
    let waiting = ready().take();
    let (u, bytes) = match waiting {
        Some(r) => r,
        None => {
            let Some(u) = updater(app, Duration::from_secs(600))?.check().await.map_err(fail)? else {
                return Ok(Value::Null);
            };
            let bytes = download(app, &u).await?;
            (u, bytes)
        }
    };
    log::info(format!("installing Tiquet {}", u.version));
    u.install(bytes).map_err(|e| {
        log::error(format!("update {} not installed: {e}", u.version));
        fail(e)
    })?;
    // Windows never gets here: the installer has closed Tiquet and starts it again
    app.restart()
}

// --selftest with TIQUET_UPDATE_URL installs what that server offers before
// it reports, so the whole update can be tried on a PC without a window
pub fn testing() -> bool {
    std::env::args().any(|a| a == "--selftest") && std::env::var_os("TIQUET_UPDATE_URL").is_some()
}
