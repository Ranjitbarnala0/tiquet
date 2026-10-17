use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{RwLock, RwLockReadGuard};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::{AppState, PrintSink};
use crate::{aeat, api, backup, log, os, paths};

// set once the window is on screen; a failure before that gets a message box
// instead of a till that silently never opens
static UP: AtomicBool = AtomicBool::new(false);

struct Shell {
    // replaced when the failure screen retries or restores a backup
    core: RwLock<Result<AppState, String>>,
    data_dir: Option<PathBuf>,
    // --selftest or --check: where to write the report before exiting
    report: Option<PathBuf>,
    shown: AtomicBool,
    // Ajustes has changes not saved yet: closing the window asks first
    dirty: AtomicBool,
}

impl Shell {
    fn core(&self) -> RwLockReadGuard<'_, Result<AppState, String>> {
        self.core.read().unwrap_or_else(|e| e.into_inner())
    }

    fn reopen(&self) -> Result<(), String> {
        let dir = self.data_dir.as_deref().ok_or("E_DATA_DIR")?;
        let core = open(dir);
        if core.is_ok() && self.report.is_none() {
            aeat::start(dir.join("tiquet.db"));
        }
        let res = core.as_ref().map(|_| ()).map_err(|e| e.clone());
        *self.core.write().unwrap_or_else(|e| e.into_inner()) = core;
        res
    }
}

fn open(dir: &Path) -> Result<AppState, String> {
    AppState::open(dir, PrintSink::Spooler)
        .map(|mut st| {
            st.desktop = true;
            st
        })
        .map_err(|e| e.0)
}

// The window starts hidden and is shown once the UI has rendered, so there
// is no white flash. `--selftest <file>` and `--check <file>` never show it:
// the UI boots, talks to the core, a report is written to <file> and the
// process exits. --selftest uses a throwaway database; --check opens the real
// one (moving and upgrading it like a normal start) and says what it found.
// Every call runs on a blocking thread: database and printer work must not
// stall the async runtime that drives the window.
#[tauri::command]
async fn call(app: AppHandle, cmd: String, args: Value) -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(move || handle(&app, &cmd, args))
        .await
        .map_err(|e| format!("E_INTERNAL:{e}"))?
}

fn handle(app: &AppHandle, cmd: &str, args: Value) -> Result<Value, String> {
    let shell = app.state::<Shell>();
    match cmd {
        "app.ready" => {
            if let Some(out) = &shell.report {
                let core = shell.core();
                let db = core.as_ref().ok().map(|c| c.health());
                let report = json!({
                    "ok": core.is_ok() && args.get("ok").and_then(Value::as_bool).unwrap_or(false),
                    "ui": args,
                    "core": core.as_ref().err(),
                    "version": env!("CARGO_PKG_VERSION"),
                    "dataDir": shell.data_dir,
                    "db": db,
                });
                let _ = std::fs::write(out, report.to_string());
                app.exit(0);
            } else {
                show(app, &shell);
            }
            Ok(Value::Null)
        }
        // the UI must be able to report its own crash even when the database
        // could not be opened
        "log.client" => {
            api::client_log(&args);
            Ok(Value::Null)
        }
        // what the failure screen needs when the database will not open
        "app.info" => {
            let dir = shell.data_dir.as_ref().map(|d| d.to_string_lossy().into_owned());
            Ok(json!({
                "dataDir": dir,
                "logPath": log::path().map(|p| p.to_string_lossy().into_owned()),
                "version": env!("CARGO_PKG_VERSION"),
                "backups": backup::list(&[backup::default_dir()]),
            }))
        }
        "app.retry" => shell.reopen().map(|_| Value::Null),
        "app.dirty" => {
            shell.dirty.store(args.get("dirty").and_then(Value::as_bool).unwrap_or(false), Ordering::SeqCst);
            Ok(Value::Null)
        }
        // the cashier chose to close without saving
        "app.exit" => {
            app.exit(0);
            Ok(Value::Null)
        }
        "app.restore" if shell.core().is_err() => {
            let dir = shell.data_dir.clone().ok_or("E_DATA_DIR")?;
            let path = PathBuf::from(args.get("path").and_then(Value::as_str).unwrap_or_default());
            let file = backup::allowed(&path, &[backup::default_dir()]).map_err(|e| e.0)?;
            backup::restore_offline(&file, &dir).map_err(|e| e.0)?;
            shell.reopen().map(|_| Value::Null)
        }
        "app.reveal" if shell.core().is_err() => {
            if let Some(dir) = &shell.data_dir {
                os::reveal(dir);
            }
            Ok(Value::Null)
        }
        _ => match &*shell.core() {
            Ok(core) => api::dispatch(core, cmd, args),
            Err(e) => Err(if e.starts_with("E_") { e.clone() } else { format!("E_DB:{e}") }),
        },
    }
}

fn show(app: &AppHandle, shell: &Shell) {
    if shell.shown.swap(true, Ordering::SeqCst) {
        return;
    }
    UP.store(true, Ordering::SeqCst);
    focus(app);
}

fn focus(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn arg(name: &str) -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(PathBuf::from)
}

// F5, Ctrl+P, Ctrl+F, Ctrl+S, Alt+Left and the other browser shortcuts would
// reload, print or save the till's page; WebView2 can switch them all off
#[cfg(windows)]
fn disable_browser_keys(app: &tauri::App) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
    use windows_core::Interface;

    if let Some(w) = app.get_webview_window("main") {
        let _ = w.with_webview(|wv| unsafe {
            let settings = wv.controller().CoreWebView2().and_then(|c| c.Settings());
            if let Ok(s) = settings.and_then(|s| s.cast::<ICoreWebView2Settings3>()) {
                let _ = s.SetAreBrowserAcceleratorKeysEnabled(false);
            }
        });
    }
}

pub fn run() {
    let selftest = arg("--selftest");
    let report = selftest.clone().or_else(|| arg("--check"));
    let testing = report.is_some();
    // the test modes get a browser profile of their own, so they never clash
    // with a Tiquet that is already open on this PC
    if testing {
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", std::env::temp_dir().join("tiquet-test-webview"));
    }
    let data_dir = match &selftest {
        Some(_) => {
            let dir = std::env::temp_dir().join("tiquet-selftest");
            let _ = std::fs::remove_dir_all(&dir);
            Ok(dir)
        }
        None => paths::data_dir().map_err(|e| e.0),
    };
    // logging starts before the window, so a window that fails to open is logged
    if let Ok(dir) = &data_dir {
        log::init(dir);
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error(format!("panic: {info}"));
        if !testing && !UP.load(Ordering::SeqCst) {
            let what = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_default();
            let log = log::path().map(|p| p.display().to_string()).unwrap_or_default();
            os::alert(
                "Tiquet",
                &format!(
                    "Tiquet no ha podido abrir su ventana. Las ventas guardadas no se han tocado.\n\n{what}\n\n\
                     Si falta o falla Microsoft Edge WebView2, instálalo desde https://go.microsoft.com/fwlink/p/?LinkId=2124703 \
                     o vuelve a instalar Tiquet.\n\nRegistro: {log}"
                ),
            );
        }
        previous(info);
    }));

    let mut builder = tauri::Builder::default();
    // a second double-click on the shortcut brings the open till to the front
    // instead of starting another copy against the same database
    if report.is_none() {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| focus(app)));
    }
    builder
        .setup(move |app| {
            let core = match &data_dir {
                Ok(dir) => {
                    if selftest.is_none() {
                        if let Ok(old) = app.path().app_local_data_dir() {
                            if let Err(e) = paths::adopt(&old, dir) {
                                log::error(format!("could not move the database from {}: {}", old.display(), e.0));
                            }
                        }
                    }
                    let core = open(dir);
                    // the test modes never talk to the AEAT
                    if core.is_ok() && !testing {
                        aeat::start(dir.join("tiquet.db"));
                    }
                    core
                }
                Err(e) => Err(e.clone()),
            };
            app.manage(Shell {
                core: RwLock::new(core),
                data_dir: data_dir.ok(),
                report: report.clone(),
                shown: AtomicBool::new(false),
                dirty: AtomicBool::new(false),
            });
            #[cfg(windows)]
            disable_browser_keys(app);

            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(if testing { 45 } else { 4 }));
                let shell = handle.state::<Shell>();
                if let Some(out) = &shell.report {
                    let _ = std::fs::write(out, r#"{"ok":false,"error":"ui never reported ready"}"#);
                    handle.exit(3);
                } else {
                    show(&handle, shell.inner());
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.state::<Shell>().dirty.load(Ordering::SeqCst) {
                    api.prevent_close();
                    let _ = window.emit("tiquet://close-requested", ());
                }
            }
        })
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("error while running Tiquet");
}
