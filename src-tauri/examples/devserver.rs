// Serves the real command layer over HTTP so the UI can be driven from a
// browser during development. Prints go to <data dir>/prints as .bin files
// unless TIQUET_PRINTER=spooler.
//
//   cargo run --example devserver --features devserver -- 1430 ../devdata

use std::path::PathBuf;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

use tiquet_lib::api;
use tiquet_lib::state::{AppState, PrintSink};

fn main() {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().and_then(|p| p.parse().ok()).unwrap_or(1430);
    let dir = PathBuf::from(args.next().unwrap_or_else(|| "devdata".into()));
    if std::env::var_os("TIQUET_DOCUMENTS").is_none() {
        std::env::set_var("TIQUET_DOCUMENTS", dir.join("documentos"));
    }
    let sink = match std::env::var("TIQUET_PRINTER").as_deref() {
        Ok("spooler") => PrintSink::Spooler,
        _ => PrintSink::Dir(dir.join("prints")),
    };
    let st = AppState::open(&dir, sink).expect("open data dir");
    tiquet_lib::aeat::start(dir.join("tiquet.db"));
    let server = Server::http(("127.0.0.1", port)).expect("bind");
    eprintln!("devserver http://127.0.0.1:{port} data={}", dir.display());

    for mut req in server.incoming_requests() {
        let Some(cmd) = req.url().strip_prefix("/api/").map(str::to_string) else {
            let _ = req.respond(Response::from_string("not found").with_status_code(404));
            continue;
        };
        // a page on another site must not be able to drive this from the
        // developer's browser: JSON only (which forces a CORS preflight it
        // never gets), and only from localhost
        let header = |name: &'static str| req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
        let local = header("Origin").map_or(true, |o| o.starts_with("http://localhost:") || o.starts_with("http://127.0.0.1:"));
        let json_body = header("Content-Type").is_some_and(|c| c.starts_with("application/json"));
        if !local || !json_body {
            let _ = req.respond(Response::from_string("forbidden").with_status_code(403));
            continue;
        }
        let mut body = String::new();
        let parsed = match req.as_reader().read_to_string(&mut body) {
            Ok(_) if body.trim().is_empty() => Ok(json!({})),
            Ok(_) => serde_json::from_str::<Value>(&body).map_err(|e| format!("E_ARGS:{e}")),
            Err(e) => Err(format!("E_ARGS:{e}")),
        };
        let (status, out) = match parsed.and_then(|args| api::dispatch(&st, &cmd, args)) {
            Ok(v) => (200, v.to_string()),
            Err(e) => (400, json!({ "error": e }).to_string()),
        };
        let ct = Header::from_bytes("Content-Type", "application/json").unwrap();
        let _ = req.respond(Response::from_string(out).with_status_code(status).with_header(ct));
    }
}
