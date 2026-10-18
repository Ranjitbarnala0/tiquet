// Tiquet updates itself from its GitHub releases. latest.json names the newest
// version and carries the installer's signature, which has to match the
// public key built into this copy. On Windows the installer then runs silently
// (/S), closes Tiquet and starts the new version with the same arguments.

use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Updater, UpdaterExt};

use crate::log;

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

// Null when there is nothing newer. Otherwise it only comes back with an
// error: an update that installs ends this process.
pub async fn install(app: &AppHandle) -> Result<Value, String> {
    let Some(u) = updater(app, Duration::from_secs(600))?.check().await.map_err(fail)? else {
        return Ok(Value::Null);
    };
    log::info(format!("downloading Tiquet {}", u.version));
    let (h, mut got) = (app.clone(), 0u64);
    let bytes = u
        .download(
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
        })?;
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
