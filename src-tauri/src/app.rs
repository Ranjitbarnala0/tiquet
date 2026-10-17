use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};

use crate::api;
use crate::state::{AppState, PrintSink};

struct Shell {
    core: Result<AppState, String>,
    selftest: Option<PathBuf>,
    shown: AtomicBool,
}

// The window starts hidden and is shown once the UI has rendered, so there
// is no white flash. `--selftest <file>` never shows it: the UI boots, talks
// to the core, the result is written to <file> and the process exits.
#[tauri::command]
async fn call(app: AppHandle, shell: State<'_, Shell>, cmd: String, args: Value) -> Result<Value, String> {
    if cmd == "app.ready" {
        if let Some(out) = &shell.selftest {
            let report = json!({ "ok": shell.core.is_ok(), "ui": args, "core": shell.core.as_ref().err() });
            let _ = std::fs::write(out, report.to_string());
            app.exit(0);
        } else {
            show(&app, &shell);
        }
        return Ok(Value::Null);
    }
    match &shell.core {
        Ok(core) => api::dispatch(core, &cmd, args),
        Err(e) => Err(format!("E_DB:{e}")),
    }
}

fn show(app: &AppHandle, shell: &Shell) {
    if shell.shown.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn selftest_arg() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let i = args.iter().position(|a| a == "--selftest")?;
    args.get(i + 1).map(PathBuf::from)
}

pub fn run() {
    let selftest = selftest_arg();
    tauri::Builder::default()
        .setup(move |app| {
            let data_dir = match &selftest {
                Some(_) => std::env::temp_dir().join(format!("tiquet-selftest-{}", std::process::id())),
                None => app.path().app_local_data_dir().unwrap_or_else(|_| std::env::temp_dir().join("tiquet")),
            };
            let core = AppState::open(&data_dir, PrintSink::Spooler).map_err(|e| e.0);
            app.manage(Shell { core, selftest: selftest.clone(), shown: AtomicBool::new(false) });

            let handle = app.handle().clone();
            let testing = selftest.is_some();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(if testing { 45 } else { 4 }));
                let shell = handle.state::<Shell>();
                if let Some(out) = &shell.selftest {
                    let _ = std::fs::write(out, r#"{"ok":false,"error":"ui never reported ready"}"#);
                    handle.exit(3);
                } else {
                    show(&handle, shell.inner());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("error while running Tiquet");
}
