// Several PCs in one shop, one set of data. One till is the main one: it keeps
// the database and, once Ajustes says so, answers the others over the shop's
// own network. The others keep their own printer, paper and screen and send
// everything else there, so the numbering, the day, the drawer and the
// products are one. Whoever is on the same network and knows a user's
// password can work; nothing else is asked.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::path::{Component, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::{fail, Error, Result};
use crate::printer::{self, Job};
use crate::state::{self, AppState, Caller, SessionUser, Slot};
use crate::{api, backup, log, os, settings, tickets, users};

pub const PORT: u16 = 47713;
// an answer that takes longer is a till that is gone; a backup or a send to
// the AEAT can take a while
const ANSWER: Duration = Duration::from_secs(120);
const CONNECT: Duration = Duration::from_secs(3);
// a logo is the biggest thing ever sent
const MOST: u64 = 8 * 1024 * 1024;
// a PC that has not been back for this long signs in again
const KEEP_DAYS: i64 = 30;
// how often the PCs signed in are written down, besides each sign-in
const SAVE_EVERY: Duration = Duration::from_secs(600);
// a main till that moved to another address is looked for at most this often
const RESCAN: Duration = Duration::from_secs(30);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// What lets the network reach the shop's data: the app hands over its core,
// the dev server and the tests their own.
pub trait Host: Send + Sync {
    fn with(&self, f: &mut dyn FnMut(&AppState)) -> bool;
}

// -------------------------------------------------------------- main till

struct Client {
    slot: Arc<Slot>,
    pc: String,
    ip: String,
    seen: String,
}

struct Shared {
    host: Arc<dyn Host>,
    clients: Mutex<HashMap<String, Client>>,
    saved: Mutex<Instant>,
    stopping: AtomicBool,
}

pub struct Running {
    server: Arc<tiny_http::Server>,
    threads: Vec<JoinHandle<()>>,
    pub addr: SocketAddr,
    shared: Arc<Shared>,
}

impl Running {
    pub fn stop(self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        for _ in &self.threads {
            self.server.unblock();
        }
        for t in self.threads {
            let _ = t.join();
        }
        save_tokens(&self.shared);
    }
}

static HOST: OnceLock<Arc<dyn Host>> = OnceLock::new();
static RUNNING: Mutex<Option<Running>> = Mutex::new(None);

pub fn set_host(h: Arc<dyn Host>) {
    let _ = HOST.set(h);
}

// 0.0.0.0 in a shop; TIQUET_LAN_BIND=127.0.0.1 for tests on a PC whose
// firewall must not ask anything
fn bind_addr() -> String {
    let ip = std::env::var("TIQUET_LAN_BIND")
        .ok()
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "0.0.0.0".into());
    format!("{ip}:{PORT}")
}

// Starts or stops answering the other PCs, and remembers the choice.
pub fn share(st: &AppState, on: bool) -> Result<()> {
    if on && st.remote().is_some() {
        return fail("E_LAN_ROLE");
    }
    let mut r = lock(&RUNNING);
    if on && r.is_none() {
        let host = HOST
            .get()
            .cloned()
            .ok_or_else(|| Error("E_LAN_HOST".into()))?;
        *r = Some(serve(host, &bind_addr())?);
    } else if !on {
        if let Some(old) = r.take() {
            old.stop();
        }
    }
    drop(r);
    settings::put(&st.conn(), "lanShare", if on { "1" } else { "0" })?;
    log::info(format!("sharing this till with the shop's other PCs: {on}"));
    Ok(())
}

// as the app opens: answer again if it did before
pub fn resume_sharing(st: &AppState) {
    let on = settings::get(&st.conn(), "lanShare")
        .ok()
        .flatten()
        .as_deref()
        == Some("1");
    if on && st.remote().is_none() && lock(&RUNNING).is_none() {
        if let Err(e) = share(st, true) {
            log::error(format!(
                "could not share this till with the shop's other PCs: {}",
                e.0
            ));
        }
    }
}

pub fn serve(host: Arc<dyn Host>, addr: &str) -> Result<Running> {
    let server =
        Arc::new(tiny_http::Server::http(addr).map_err(|e| Error(format!("E_LAN_PORT:{e}")))?);
    let bound = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| Error("E_LAN_PORT".into()))?;
    let shared = Arc::new(Shared {
        host,
        clients: Mutex::new(HashMap::new()),
        saved: Mutex::new(Instant::now()),
        stopping: AtomicBool::new(false),
    });
    restore_tokens(&shared);
    let threads = (0..4)
        .map(|_| {
            let (server, shared) = (server.clone(), shared.clone());
            std::thread::spawn(move || loop {
                match server.recv() {
                    Ok(req) => answer(&shared, req),
                    Err(_) if shared.stopping.load(Ordering::SeqCst) => break,
                    // a connection that went wrong is that connection's problem
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            })
        })
        .collect();
    log::info(format!("answering the shop's other PCs on {bound}"));
    Ok(Running {
        server,
        threads,
        addr: bound,
        shared,
    })
}

// only the shop's own network, never something forwarded from outside it
pub fn private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_loopback() || v.is_link_local(),
        IpAddr::V6(v) => {
            let first = v.segments()[0];
            v.is_loopback() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80
        }
    }
}

fn header(req: &tiny_http::Request, name: &'static str) -> Option<String> {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_string())
}

fn reply(req: tiny_http::Request, code: u16, body: Value) {
    let ct = tiny_http::Header::from_bytes("Content-Type", "application/json").expect("header");
    let _ = req.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(code)
            .with_header(ct),
    );
}

fn shop_hello(st: &AppState) -> Value {
    let conn = st.conn();
    let shop = settings::load_own(&conn)
        .map(|s| s.shop_name)
        .unwrap_or_default();
    json!({ "app": "tiquet", "shop": shop, "install": settings::install_id(&conn).unwrap_or_default(), "version": env!("CARGO_PKG_VERSION"), "port": PORT })
}

fn answer(shared: &Shared, mut req: tiny_http::Request) {
    let ip = req.remote_addr().map(|a| a.ip());
    if !ip.is_some_and(private) {
        return reply(req, 403, json!({ "error": "E_FORBIDDEN" }));
    }
    let ip = ip.map(|i| i.to_string()).unwrap_or_default();
    let url = req.url().to_string();
    if url == "/tiquet" {
        let mut out = Value::Null;
        shared.host.with(&mut |st| out = shop_hello(st));
        return reply(req, if out.is_null() { 503 } else { 200 }, out);
    }
    // A page open in a browser of the shop cannot drive the till: only Tiquet
    // sends this header, which a page cannot add without asking first (and
    // nothing here answers that), and only to an address, never to a name.
    let by_address = header(&req, "Host")
        .is_some_and(|h| h.parse::<SocketAddr>().is_ok() || h.parse::<IpAddr>().is_ok());
    let tiquet = header(&req, "X-Tiquet").as_deref() == Some("1");
    if *req.method() != tiny_http::Method::Post || !by_address || !tiquet {
        return reply(req, 403, json!({ "error": "E_FORBIDDEN" }));
    }
    let Some(cmd) = url.strip_prefix("/api/").map(str::to_string) else {
        return reply(req, 404, json!({ "error": "E_NOT_FOUND" }));
    };
    let mut body = String::new();
    if req
        .as_reader()
        .take(MOST)
        .read_to_string(&mut body)
        .is_err()
    {
        return reply(req, 400, json!({ "error": "E_ARGS" }));
    }
    let args = if body.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&body).unwrap_or(Value::Null)
    };
    if !args.is_object() {
        return reply(req, 400, json!({ "error": "E_ARGS" }));
    }
    let token = header(&req, "X-Tiquet-Token").filter(|t| !t.is_empty());
    let pc: String = header(&req, "X-Tiquet-Pc")
        .unwrap_or_default()
        .chars()
        .take(60)
        .collect();
    let device = header(&req, "X-Tiquet-Device")
        .and_then(|d| base64::engine::general_purpose::STANDARD.decode(d).ok())
        .and_then(|d| serde_json::from_slice::<Map<String, Value>>(&d).ok());
    let known = token
        .as_ref()
        .and_then(|t| lock(&shared.clients).get(t).map(|c| c.slot.clone()));
    let slot = known.clone().unwrap_or_else(|| Arc::new(Slot::new()));
    let caller = Rc::new(Caller {
        slot: slot.clone(),
        device,
        jobs: Default::default(),
    });
    let mut out: Option<std::result::Result<Value, String>> = None;
    shared.host.with(&mut |st| {
        out = Some(state::as_caller(caller.clone(), || {
            api::dispatch(st, &cmd, args.clone())
        }));
    });
    let Some(mut out) = out else {
        return reply(req, 503, json!({ "error": "E_LAN_DOWN" }));
    };

    // who is signed in from where
    let mut clients = lock(&shared.clients);
    let mut changed = false;
    let signed_in = slot.user().is_some();
    if let (Ok(v), true) = (
        out.as_mut(),
        signed_in && (known.is_none() || cmd == "login"),
    ) {
        // a new sign-in gets a new key; the one it had is no longer good
        if let Some(t) = &token {
            clients.remove(t);
        }
        let t = new_token();
        clients.insert(
            t.clone(),
            Client {
                slot: slot.clone(),
                pc: pc.clone(),
                ip: ip.clone(),
                seen: tickets::now(),
            },
        );
        v["_token"] = json!(t);
        changed = true;
    } else if let Some(t) = &token {
        if !signed_in {
            changed = clients.remove(t).is_some();
        } else if let Some(c) = clients.get_mut(t) {
            c.seen = tickets::now();
            c.ip = ip.clone();
            if !pc.is_empty() {
                c.pc = pc.clone();
            }
        }
    }
    drop(clients);
    if changed || lock(&shared.saved).elapsed() > SAVE_EVERY {
        save_tokens(shared);
    }

    if let Ok(v) = out.as_mut() {
        let jobs = caller.jobs.take();
        if !jobs.is_empty() {
            v["_jobs"] = serde_json::to_value(jobs).unwrap_or_default();
        }
    }
    match out {
        Ok(v) => reply(req, 200, v),
        Err(e) => reply(req, 400, json!({ "error": e })),
    }
}

fn new_token() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut b = [0u8; 24];
    OsRng.fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

// The PCs signed in survive the main till closing to update itself: they
// carry on with no password to type when it is back.
fn save_tokens(shared: &Shared) {
    let list: Vec<Value> = lock(&shared.clients)
        .iter()
        .filter_map(|(t, c)| {
            c.slot.user().map(
                |u| json!({ "token": t, "user": u.id, "pc": c.pc, "ip": c.ip, "seen": c.seen }),
            )
        })
        .collect();
    shared.host.with(&mut |st| {
        if let Err(e) = settings::put(
            &st.conn(),
            "lanTokens",
            &Value::Array(list.clone()).to_string(),
        ) {
            log::warn(format!("could not keep the PCs signed in: {}", e.0));
        }
    });
    *lock(&shared.saved) = Instant::now();
}

fn restore_tokens(shared: &Shared) {
    let mut found = Vec::new();
    shared.host.with(&mut |st| {
        let conn = st.conn();
        let stored: Vec<Value> = settings::get(&conn, "lanTokens")
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str(&j).ok())
            .unwrap_or_default();
        let oldest = (chrono::Local::now() - chrono::Duration::days(KEEP_DAYS))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        for v in stored {
            let (Some(t), Some(id)) = (v["token"].as_str(), v["user"].as_i64()) else {
                continue;
            };
            let seen = v["seen"].as_str().unwrap_or_default().to_string();
            if seen < oldest {
                continue;
            }
            // a user since switched off signs in no more
            if let Ok(Some((u, _, true))) = users::credentials(&conn, id) {
                let slot = Arc::new(Slot::new());
                slot.sign_in(u);
                let (pc, ip) = (
                    v["pc"].as_str().unwrap_or_default().into(),
                    v["ip"].as_str().unwrap_or_default().into(),
                );
                found.push((t.to_string(), Client { slot, pc, ip, seen }));
            }
        }
    });
    lock(&shared.clients).extend(found);
}

// ------------------------------------------------------------- other PCs

// The main till this PC works with, kept in its own small database.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Main {
    pub addr: String,
    pub install: String,
    pub shop: String,
    #[serde(default)]
    pub token: Option<String>,
}

pub fn load_main(conn: &rusqlite::Connection) -> Option<Main> {
    settings::get(conn, "lanMain")
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
}

fn save_main(st: &AppState, m: Option<&Main>) -> Result<()> {
    {
        let conn = st.conn();
        match m {
            Some(m) => settings::put(&conn, "lanMain", &serde_json::to_string(m)?)?,
            None => settings::clear(&conn, "lanMain")?,
        }
    }
    st.set_remote(m.cloned());
    Ok(())
}

// what never leaves this PC: its printer, its folders, its own log, and the
// network itself
const HERE: &[&str] = &[
    "log.client",
    "diagnostics.save",
    "app.reveal",
    "session.keep",
];

pub fn local(cmd: &str) -> bool {
    HERE.contains(&cmd)
        || cmd.starts_with("printers.")
        || cmd.starts_with("lan.")
        || cmd.starts_with("license.")
}

fn pc_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_default()
}

enum Sent {
    // nothing reached the other till
    Unreached,
    // it may have been done there: never sent again on its own
    Lost,
}

// A small HTTP/1.1 client: JSON over the shop's network, one request a
// connection.
fn http(
    addr: &str,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
    wait: Duration,
) -> std::result::Result<(u16, Vec<u8>), Sent> {
    let to = addr
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or(Sent::Unreached)?;
    let mut s = TcpStream::connect_timeout(&to, wait.min(CONNECT)).map_err(|_| Sent::Unreached)?;
    s.set_read_timeout(Some(wait))
        .map_err(|_| Sent::Unreached)?;
    s.set_write_timeout(Some(CONNECT * 4))
        .map_err(|_| Sent::Unreached)?;
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).map_err(|_| Sent::Unreached)?;
    s.write_all(body).map_err(|_| Sent::Lost)?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|_| Sent::Lost)?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(Sent::Lost)?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let code = head
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or(Sent::Lost)?;
    let mut rest = raw[split + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        rest = unchunk(&rest);
    }
    Ok((code, rest))
}

fn unchunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = b.windows(2).position(|w| w == b"\r\n") {
        let n = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).trim(), 16).unwrap_or(0);
        if n == 0 || i + 2 + n > b.len() {
            break;
        }
        out.extend_from_slice(&b[i + 2..i + 2 + n]);
        b = &b[(i + 4 + n).min(b.len())..];
    }
    out
}

// what a Tiquet that shares its till says about itself
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub addr: String,
    pub shop: String,
    pub install: String,
    pub version: String,
}

fn hello(addr: &str, wait: Duration) -> Option<Found> {
    let (code, body) = http(addr, "GET", "/tiquet", &[], &[], wait).ok()?;
    let v: Value = serde_json::from_slice(&body).ok()?;
    (code == 200 && v["app"] == "tiquet").then(|| Found {
        addr: addr.to_string(),
        shop: v["shop"].as_str().unwrap_or_default().into(),
        install: v["install"].as_str().unwrap_or_default().into(),
        version: v["version"].as_str().unwrap_or_default().into(),
    })
}

// this PC's addresses on its networks, found without sending anything
pub fn own_addresses() -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for probe in [
        "192.168.1.1:9",
        "192.168.0.1:9",
        "10.0.0.1:9",
        "172.16.0.1:9",
        "8.8.8.8:53",
    ] {
        let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else {
            continue;
        };
        if sock.connect(probe).is_err() {
            continue;
        }
        if let Ok(SocketAddr::V4(a)) = sock.local_addr() {
            let ip = *a.ip();
            if !ip.is_unspecified() && !ip.is_loopback() && !out.contains(&ip) {
                out.push(ip);
            }
        }
    }
    out
}

// Every Tiquet that shares its till on this PC's networks: each address of
// the /24 is asked at once, a couple of seconds in all.
pub fn find() -> Vec<Found> {
    let mut targets = vec![format!("127.0.0.1:{PORT}")];
    for ip in own_addresses() {
        let [a, b, c, _] = ip.octets();
        targets.extend((1..=254u8).map(|d| format!("{a}.{b}.{c}.{d}:{PORT}")));
    }
    targets.dedup();
    let found = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for chunk in targets.chunks(4) {
            let found = &found;
            sc.spawn(move || {
                for t in chunk {
                    if let Some(f) = hello(t, Duration::from_millis(700)) {
                        lock(found).push(f);
                    }
                }
            });
        }
    });
    let mut all = found.into_inner().unwrap_or_else(|e| e.into_inner());
    // the same till seen on this PC and on its address: once
    all.sort_by(|a, b| a.install.cmp(&b.install).then(a.addr.cmp(&b.addr)));
    all.dedup_by(|a, b| a.install == b.install);
    all
}

pub fn connect(st: &AppState, addr: &str) -> Result<Main> {
    let addr = addr.trim();
    let addr = if addr.contains(':') {
        addr.to_string()
    } else {
        format!("{addr}:{PORT}")
    };
    let Some(f) = hello(&addr, Duration::from_secs(3)) else {
        return fail("E_LAN_NOT_FOUND");
    };
    if f.install == settings::install_id(&st.conn())? {
        return fail("E_LAN_SELF");
    }
    // this till stops answering others: it works with that one now
    if let Some(old) = lock(&RUNNING).take() {
        old.stop();
    }
    settings::put(&st.conn(), "lanShare", "0")?;
    let m = Main {
        addr: f.addr,
        install: f.install,
        shop: f.shop,
        token: None,
    };
    save_main(st, Some(&m))?;
    st.set_session(None);
    log::info(format!(
        "this PC now works with the main till {} at {}",
        m.shop, m.addr
    ));
    Ok(m)
}

pub fn disconnect(st: &AppState) -> Result<()> {
    if let Some(m) = st.remote() {
        if m.token.is_some() {
            let _ = call(st, &m, "logout", &json!({}));
        }
        log::info(format!(
            "this PC no longer works with the main till {}",
            m.shop
        ));
    }
    save_main(st, None)?;
    st.set_session(None);
    Ok(())
}

static SCANNED: Mutex<Option<Instant>> = Mutex::new(None);

// the main till at another address, when it moved (the router gave it
// another one); looked for at most every half minute
fn moved(st: &AppState, m: &Main) -> Option<Main> {
    {
        let mut last = lock(&SCANNED);
        if last.is_some_and(|t| t.elapsed() < RESCAN) {
            return None;
        }
        *last = Some(Instant::now());
    }
    let f = find()
        .into_iter()
        .find(|f| f.install == m.install && f.addr != m.addr)?;
    let moved = Main {
        addr: f.addr,
        ..m.clone()
    };
    save_main(st, Some(&moved)).ok()?;
    log::info(format!("the main till {} is now at {}", m.shop, moved.addr));
    Some(moved)
}

// One request to the main till as this PC, with its sign-in and its printer
// settings.
fn call(st: &AppState, m: &Main, cmd: &str, args: &Value) -> Result<Value> {
    let device = settings::device(&settings::load_own(&st.conn())?);
    let device =
        base64::engine::general_purpose::STANDARD.encode(Value::Object(device).to_string());
    let mut headers = vec![
        ("X-Tiquet", "1".to_string()),
        ("X-Tiquet-Pc", pc_name()),
        ("X-Tiquet-Device", device),
    ];
    if let Some(t) = &m.token {
        headers.push(("X-Tiquet-Token", t.clone()));
    }
    let body = args.to_string();
    let path = format!("/api/{cmd}");
    let sent = match http(&m.addr, "POST", &path, &headers, body.as_bytes(), ANSWER) {
        Err(Sent::Unreached) => match moved(st, m) {
            Some(m) => http(&m.addr, "POST", &path, &headers, body.as_bytes(), ANSWER),
            None => return fail("E_LAN_DOWN"),
        },
        other => other,
    };
    let (code, body) = sent.map_err(|_| Error("E_LAN_DOWN".into()))?;
    let v: Value = serde_json::from_slice(&body).map_err(|_| Error("E_LAN_DOWN".into()))?;
    if code != 200 {
        let e = v["error"].as_str().unwrap_or("E_LAN_DOWN").to_string();
        // the main till no longer knows this sign-in
        if e == "E_SESSION" || e == "E_SESSION_IDLE" {
            forget_token(st);
        }
        return Err(Error(e));
    }
    Ok(v)
}

fn forget_token(st: &AppState) {
    if let Some(m) = st.remote().filter(|m| m.token.is_some()) {
        let _ = save_main(st, Some(&Main { token: None, ..m }));
    }
    st.set_session(None);
}

fn user_of(v: &Value) -> Option<SessionUser> {
    serde_json::from_value(v.clone()).ok()
}

// Everything that is not this PC's own goes to the main till. What it
// printed for this PC is printed here, a file it made is saved here and a link
// opens here.
pub fn forward(st: &AppState, cmd: &str, args: Value) -> Result<Value> {
    let Some(m) = st.remote() else {
        return fail("E_LAN_ROLE");
    };
    if cmd == "settings.save" {
        return save_settings(st, &m, args);
    }
    let mut v = match call(st, &m, cmd, &args) {
        Ok(v) => v,
        // with the main till gone the screen still needs to know what it is
        Err(e) if cmd == "bootstrap" && e.0 == "E_LAN_DOWN" => return Ok(down(st, &m)),
        Err(e) => return Err(e),
    };
    if let Some(t) = v
        .as_object_mut()
        .and_then(|o| o.remove("_token"))
        .and_then(|t| t.as_str().map(str::to_string))
    {
        let m = st.remote().unwrap_or(m.clone());
        save_main(
            st,
            Some(&Main {
                token: Some(t),
                ..m
            }),
        )?;
    }
    match cmd {
        "login" | "setup" | "recover" => st.set_session(user_of(&v["user"])),
        "logout" => forget_token(st),
        "bootstrap" => {
            st.set_session(user_of(&v["session"]));
            v["version"] = json!(env!("CARGO_PKG_VERSION"));
            v["lan"] = role(st);
        }
        _ => {}
    }
    if let Some(jobs) = v.as_object_mut().and_then(|o| o.remove("_jobs")) {
        let jobs: Vec<(String, Job)> = serde_json::from_value(jobs).unwrap_or_default();
        print_here(st, &m, cmd, &mut v, jobs)?;
    }
    if let Some(file) = v.as_object_mut().and_then(|o| o.remove("_file")) {
        let path = save_here(&file)?;
        // with no mail account the PDF goes on the clipboard to paste in
        if args["copy"].as_bool().unwrap_or(false) || cmd == "tickets.email" {
            v["copied"] = json!(st.desktop() && os::copy_file(&path));
        }
        if args["reveal"].as_bool().unwrap_or(false) && st.desktop() {
            os::reveal(&path);
        }
        v["path"] = json!(path.to_string_lossy());
    }
    // a WhatsApp chat or an email to write: on this PC
    if v["opened"] == json!(false) && st.desktop() {
        if let Some(url) = v["url"].as_str().map(str::to_string) {
            let can = !url.starts_with("mailto:") || os::handles("mailto");
            v["opened"] = json!(can && os::open(&url));
        }
    }
    Ok(v)
}

fn down(st: &AppState, m: &Main) -> Value {
    let conn = st.conn();
    let lang = settings::load_own(&conn)
        .map(|s| s.lang)
        .unwrap_or_else(|_| "es".into());
    json!({
        "needsSetup": false,
        "canRecover": false,
        "installId": settings::install_id(&conn).unwrap_or_default(),
        "users": [],
        "lang": lang,
        "shopName": m.shop,
        "session": null,
        "version": env!("CARGO_PKG_VERSION"),
        "autoUpdate": true,
        "lan": { "role": "secondary", "shop": m.shop, "addr": m.addr, "down": true },
    })
}

// The prints the main till made for this PC, on this PC's printer. The main
// till recorded them as printed: what did not reach the paper is told back.
fn print_here(
    st: &AppState,
    m: &Main,
    cmd: &str,
    v: &mut Value,
    jobs: Vec<(String, Job)>,
) -> Result<()> {
    let s = settings::load_own(&st.conn())?;
    let mut failed: Option<String> = None;
    for (doc, job) in jobs {
        // a drawer to open never holds the sale up
        if doc == "cajon" {
            printer::send_detached(&st.sink, &s, &doc, &job.raw);
            continue;
        }
        let out = if job.sheets.is_empty() {
            printer::send_job(&st.sink, &s, &doc, &job)
        } else {
            printer::send_sheets(&st.sink, &s, &doc, &job.sheets)
        };
        let (e, w) = match out {
            Ok(w) => (None, w),
            Err(e) => (Some(e.0), None),
        };
        if let Some(e) = &e {
            log::warn(format!("print {doc} here: {e}"));
            let _ = call(
                st,
                m,
                "tickets.printFailed",
                &json!({ "number": doc, "detail": e }),
            );
        }
        // a repair's deposit ticket answers for itself
        let place = if v["deposit"]["ticket"]["number"].as_str() == Some(doc.as_str()) {
            &mut v["deposit"]
        } else {
            &mut *v
        };
        if place.get("printed").is_some() || place.get("printError").is_some() {
            if let Some(e) = e {
                // a recharge slip with no printer set up is no error
                if !(cmd == "book.add" && e == "E_NO_PRINTER") {
                    if place.get("printed").is_some() {
                        place["printed"] = json!(false);
                    }
                    if place["printError"].is_null() {
                        place["printError"] = json!(e);
                    }
                }
            } else if let (Some(w), true) = (w, place.get("printWarning").is_some()) {
                place["printWarning"] = json!(w);
            }
        } else if let Some(e) = e {
            failed = failed.or(Some(e));
        } else if let Some(w) = w {
            place["warning"] = json!(w);
        }
    }
    match failed {
        Some(e) => Err(Error(e)),
        None => Ok(()),
    }
}

// A file made there for this PC, in this PC's own Documents\Tiquet. Only a
// plain relative path is taken.
fn save_here(file: &Value) -> Result<PathBuf> {
    let sub = PathBuf::from(file["sub"].as_str().unwrap_or_default());
    if sub.as_os_str().is_empty() || !sub.components().all(|c| matches!(c, Component::Normal(_))) {
        return fail("E_ARGS:file");
    }
    let data = base64::engine::general_purpose::STANDARD
        .decode(file["data"].as_str().unwrap_or_default())
        .map_err(|_| Error("E_ARGS:file".into()))?;
    let path = backup::documents().join(sub);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, data)?;
    Ok(path)
}

// Ajustes saved here: the printer and the rest of this PC's own settings stay
// on this PC, the shop's go to the main till, which also says whether this
// person may change settings at all.
fn save_settings(st: &AppState, m: &Main, args: Value) -> Result<Value> {
    let Some(all) = args.as_object() else {
        return fail("E_ARGS");
    };
    let pick = |own: bool| -> Map<String, Value> {
        all.iter()
            .filter(|(k, _)| settings::DEVICE_KEYS.contains(&k.as_str()) == own)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    };
    let (mine, shop) = (pick(true), pick(false));
    call(st, m, "settings.save", &Value::Object(shop))?;
    if !mine.is_empty() {
        let old = settings::load_own(&st.conn())?;
        let mut new = settings::with_device(old.clone(), &mine);
        // a width given without Automático is this PC's own choice
        if mine.contains_key("columns") && !mine.contains_key("paperAuto") {
            new.paper_auto = false;
        }
        if new.paper_auto {
            new.columns = printer::auto_columns(&st.sink, &new).unwrap_or(new.columns);
        }
        let new = new.clean();
        let conn = st.conn();
        settings::put(&conn, "settings", &serde_json::to_string(&new)?)?;
        // "None" chosen on purpose: no printer is then picked on its own
        if new.printer_name.trim().is_empty() && !old.printer_name.trim().is_empty() {
            settings::put(&conn, "printerOff", "1")?;
        } else if !new.printer_name.trim().is_empty() {
            settings::clear(&conn, "printerOff")?;
        }
    }
    call(st, m, "settings.get", &json!({}))
}

// what Ajustes and the screen show about this PC and the network
pub fn role(st: &AppState) -> Value {
    if let Some(m) = st.remote() {
        return json!({ "role": "secondary", "shop": m.shop, "addr": m.addr, "signedIn": m.token.is_some() });
    }
    let r = lock(&RUNNING);
    match r.as_ref() {
        Some(run) => {
            let mut clients: Vec<Value> = lock(&run.shared.clients)
                .iter()
                .map(|(t, c)| json!({ "id": &t[..8], "pc": c.pc, "ip": c.ip, "seen": c.seen, "user": c.slot.user().map(|u| u.name) }))
                .collect();
            clients.sort_by(|a, b| b["seen"].as_str().cmp(&a["seen"].as_str()));
            let addresses: Vec<String> = own_addresses().iter().map(|a| a.to_string()).collect();
            json!({ "role": "main", "port": run.addr.port(), "addresses": addresses, "clients": clients })
        }
        None => json!({ "role": "single" }),
    }
}

// a PC the owner no longer wants working with this till
fn kick(id: &str) -> Result<()> {
    let r = lock(&RUNNING);
    let Some(run) = r.as_ref() else {
        return fail("E_LAN_ROLE");
    };
    if id.len() < 8 {
        return fail("E_ARGS:id");
    }
    lock(&run.shared.clients).retain(|t, _| !t.starts_with(id));
    save_tokens(&run.shared);
    Ok(())
}

pub fn route(st: &AppState, cmd: &str, a: &Value) -> Result<Value> {
    match cmd {
        "lan.status" => Ok(role(st)),
        // the notice on a PC whose main till stopped answering asks this
        "lan.ping" => Ok(
            json!({ "up": st.remote().map_or(true, |m| hello(&m.addr, Duration::from_secs(2)).is_some() || moved(st, &m).is_some()) }),
        ),
        "lan.share" => {
            let me = st.admin()?;
            let on = a["on"].as_bool().unwrap_or(false);
            share(st, on)?;
            log::info(format!(
                "sharing with the shop's other PCs turned {} by {}",
                if on { "on" } else { "off" },
                me.name
            ));
            Ok(role(st))
        }
        "lan.kick" => {
            st.admin()?;
            kick(a["id"].as_str().unwrap_or_default())?;
            Ok(role(st))
        }
        // finding and joining a main till asks for no sign-in on a new PC:
        // it has nobody to sign in yet
        "lan.find" => {
            let own = settings::install_id(&st.conn())?;
            Ok(json!(find()
                .into_iter()
                .filter(|f| f.install != own)
                .collect::<Vec<_>>()))
        }
        "lan.connect" => {
            // a till with its own sales works with another one only when its
            // owner says so
            if st.remote().is_none() && users::count(&st.conn())? > 0 {
                st.admin()?;
            }
            Ok(json!(connect(st, a["addr"].as_str().unwrap_or_default())?))
        }
        "lan.disconnect" => {
            disconnect(st)?;
            Ok(role(st))
        }
        _ => fail(format!("E_UNKNOWN:{cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PrintSink;
    use std::path::Path;

    struct TestHost(Arc<AppState>);

    impl Host for TestHost {
        fn with(&self, f: &mut dyn FnMut(&AppState)) -> bool {
            f(&self.0);
            true
        }
    }

    fn till(name: &str) -> (Arc<AppState>, PathBuf) {
        static DOCS: std::sync::Once = std::sync::Once::new();
        DOCS.call_once(|| {
            std::env::set_var(
                "TIQUET_DOCUMENTS",
                std::env::temp_dir().join("tiquet-test-documents"),
            )
        });
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("tiquet-lan-{name}-{n}"));
        (
            Arc::new(AppState::open(&d, PrintSink::Dir(d.join("prints"))).unwrap()),
            d,
        )
    }

    fn prints(d: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<(String, Vec<u8>)> = d
            .join("prints")
            .read_dir()
            .map(|r| {
                r.filter_map(|f| f.ok())
                    .map(|f| {
                        (
                            f.file_name().to_string_lossy().into_owned(),
                            std::fs::read(f.path()).unwrap(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    fn run(bytes: &[u8], c: u8, n: usize) -> bool {
        bytes.windows(n).any(|w| w.iter().all(|b| *b == c))
    }

    #[test]
    fn a_second_pc_sells_into_the_main_tills_data_and_prints_on_its_own_printer() {
        let (main, main_dir) = till("main");
        let setup = json!({ "shopName": "Mi Tienda Móvil", "shopNif": "B12345674", "adminName": "Ana", "password": "123456", "samples": true, "lang": "es" });
        api::dispatch(&main, "setup", setup).unwrap();
        api::dispatch(
            &main,
            "settings.save",
            json!({ "printerName": "POS-MAIN", "columns": 48, "paperAuto": false }),
        )
        .unwrap();
        let eva = api::dispatch(
            &main,
            "users.save",
            json!({ "name": "Eva", "role": "cashier", "password": "4321" }),
        )
        .unwrap();
        api::dispatch(&main, "logout", json!({})).unwrap();
        let server = serve(Arc::new(TestHost(main.clone())), "127.0.0.1:0").unwrap();
        let addr = server.addr.to_string();

        // a new PC finds the shop and joins it
        let (pc, pc_dir) = till("pc2");
        assert_eq!(
            hello(&addr, Duration::from_secs(2)).unwrap().shop,
            "Mi Tienda Móvil"
        );
        assert_eq!(
            api::dispatch(&pc, "lan.connect", json!({ "addr": addr })).unwrap()["shop"],
            "Mi Tienda Móvil"
        );
        let boot = api::dispatch(&pc, "bootstrap", json!({})).unwrap();
        assert_eq!(
            (boot["needsSetup"].as_bool(), boot["lan"]["role"].as_str()),
            (Some(false), Some("secondary"))
        );
        assert!(boot["users"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| u["name"] == "Eva"));
        assert!(boot["session"].is_null());

        // a wrong password is refused there; the right one signs in here
        assert_eq!(
            api::dispatch(
                &pc,
                "login",
                json!({ "userId": eva["id"], "password": "nope" })
            )
            .unwrap_err(),
            "E_AUTH"
        );
        api::dispatch(
            &pc,
            "login",
            json!({ "userId": eva["id"], "password": "4321" }),
        )
        .unwrap();
        assert_eq!(pc.current().unwrap().name, "Eva");
        assert!(
            main.current().is_none(),
            "the main till's own screen stays signed out"
        );

        // this PC's printer and paper are its own
        let mine = json!({ "printerName": "POS-PC2", "columns": 32, "paperAuto": false });
        let local = settings::with_device(
            settings::load_own(&pc.conn()).unwrap(),
            mine.as_object().unwrap(),
        );
        settings::put(
            &pc.conn(),
            "settings",
            &serde_json::to_string(&local).unwrap(),
        )
        .unwrap();
        let s = api::dispatch(&pc, "settings.get", json!({})).unwrap();
        assert_eq!(
            (
                s["printerName"].as_str(),
                s["columns"].as_i64(),
                s["shopNif"].as_str()
            ),
            (Some("POS-PC2"), Some(32), Some("B12345674"))
        );

        // a sale made here is in the main till's data, and printed here, on
        // this PC's 58 mm paper
        let before_main = prints(&main_dir).len();
        let sale = json!({ "lines": [{ "name": "Funda", "qty": 1, "unitCents": 1250, "vatBp": 2100 }], "payment": { "method": "cash" }, "saleId": "pc2-1" });
        let r = api::dispatch(&pc, "sale.finalize", sale.clone()).unwrap();
        assert_eq!(
            (r["printed"].as_bool(), r["ticket"]["userName"].as_str()),
            (Some(true), Some("Eva"))
        );
        assert_eq!(
            prints(&main_dir).len(),
            before_main,
            "nothing printed on the main till"
        );
        let number = r["ticket"]["number"].as_str().unwrap().to_string();
        let here = prints(&pc_dir);
        let (_, bytes) = here
            .iter()
            .find(|(n, _)| n.contains(&number))
            .expect("printed on this PC");
        assert!(bytes.windows(5).any(|w| w == b"Funda"));
        assert!(
            !run(bytes, b'-', 33) && !run(bytes, b'=', 33),
            "laid out for 32 columns"
        );
        let n: i64 = main
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM tickets WHERE user_name = 'Eva'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
        // the same sale sent again is not charged twice
        assert_eq!(
            api::dispatch(&pc, "sale.finalize", sale).unwrap()["duplicate"],
            true
        );

        // a cashier changes no settings, from any PC
        assert_eq!(
            api::dispatch(&pc, "settings.save", json!({ "footer": "Gracias" })).unwrap_err(),
            "E_FORBIDDEN"
        );
        // nor can a PC touch the main till's printer, folders or network
        for cmd in ["printers.auto", "lan.share", "app.reveal", "backup.restore"] {
            let c = Rc::new(Caller {
                slot: Arc::new(Slot::new()),
                device: None,
                jobs: Default::default(),
            });
            assert_eq!(
                state::as_caller(c, || api::dispatch(&main, cmd, json!({}))).unwrap_err(),
                "E_LAN_HERE",
                "{cmd}"
            );
        }

        // a PDF made there is saved here
        let pdf = api::dispatch(&pc, "tickets.pdf", json!({ "id": r["ticket"]["id"] })).unwrap();
        let path = PathBuf::from(pdf["path"].as_str().unwrap());
        assert!(
            path.starts_with(backup::documents())
                && std::fs::read(&path).unwrap().starts_with(b"%PDF")
        );

        // the owner signs in on this PC too: the shop's settings go to the main
        // till, the printer stays here
        api::dispatch(&pc, "logout", json!({})).unwrap();
        assert!(pc.current().is_none() && pc.remote().unwrap().token.is_none());
        api::dispatch(&pc, "login", json!({ "userId": 1, "password": "123456" })).unwrap();
        let s = api::dispatch(
            &pc,
            "settings.save",
            json!({ "footer": "Hasta pronto", "printerName": "OTRA-PC2" }),
        )
        .unwrap();
        assert_eq!(
            (s["footer"].as_str(), s["printerName"].as_str()),
            (Some("Hasta pronto"), Some("OTRA-PC2"))
        );
        let own = settings::load_own(&main.conn()).unwrap();
        assert_eq!(
            (own.footer.as_str(), own.printer_name.as_str(), own.columns),
            ("Hasta pronto", "POS-MAIN", 48)
        );
        assert_eq!(
            settings::load_own(&pc.conn()).unwrap().printer_name,
            "OTRA-PC2"
        );

        // a full invoice made here comes out on this PC's own A4 printer
        api::dispatch(&pc, "settings.save", json!({ "invoicePrinter": "A4-PC2" })).unwrap();
        let before_main = prints(&main_dir).len();
        let inv = json!({ "lines": [{ "name": "iPhone 15", "qty": 1, "unitCents": 79_900, "vatBp": 2100 }], "payment": { "method": "card" },
                          "customer": { "name": "Acme Telecom SL", "nif": "B12345674", "address": "" }, "saleId": "pc2-inv" });
        assert_eq!(
            api::dispatch(&pc, "sale.finalize", inv).unwrap()["printed"].as_bool(),
            Some(true)
        );
        assert!(prints(&pc_dir)
            .iter()
            .any(|(n, b)| n.ends_with("-A4.pdf") && b.starts_with(b"%PDF")));
        assert_eq!(
            prints(&main_dir).len(),
            before_main,
            "nothing printed on the main till"
        );
        assert_eq!(
            settings::load_own(&main.conn()).unwrap().invoice_printer,
            ""
        );

        // the main till closes to update itself and opens again: still signed in
        server.stop();
        assert_eq!(
            api::dispatch(&pc, "day.totals", json!({})).unwrap_err(),
            "E_LAN_DOWN"
        );
        let gone = api::dispatch(&pc, "bootstrap", json!({})).unwrap();
        assert_eq!(gone["lan"]["down"].as_bool(), Some(true));
        let server = serve(Arc::new(TestHost(main.clone())), &addr).unwrap();
        let back = api::dispatch(&pc, "bootstrap", json!({})).unwrap();
        assert_eq!(back["session"]["name"], "Ana");
        api::dispatch(&pc, "day.totals", json!({})).unwrap();
        server.stop();

        // and the PC can go back to working on its own
        api::dispatch(&pc, "lan.disconnect", json!({})).unwrap();
        assert_eq!(
            api::dispatch(&pc, "bootstrap", json!({})).unwrap()["needsSetup"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn only_tiquet_on_the_shops_own_network_is_answered() {
        for (ip, ok) in [
            ("192.168.1.20", true),
            ("10.0.0.5", true),
            ("172.20.1.1", true),
            ("127.0.0.1", true),
            ("169.254.3.3", true),
            ("8.8.8.8", false),
            ("80.58.61.250", false),
            ("::1", true),
            ("fe80::1", true),
            ("2a02:9130::1", false),
        ] {
            assert_eq!(private(ip.parse().unwrap()), ok, "{ip}");
        }
        let (main, _) = till("guard");
        let server = serve(Arc::new(TestHost(main)), "127.0.0.1:0").unwrap();
        let addr = server.addr.to_string();
        let ask = |headers: &[(&str, String)], host: Option<&str>| {
            let mut s = TcpStream::connect(&addr).unwrap();
            let mut req = format!("POST /api/bootstrap HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: 2\r\n", host.unwrap_or(&addr));
            for (k, v) in headers {
                req.push_str(&format!("{k}: {v}\r\n"));
            }
            req.push_str("\r\n{}");
            s.write_all(req.as_bytes()).unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).unwrap();
            out.split_whitespace().nth(1).unwrap().to_string()
        };
        // a page in a browser sends no X-Tiquet, and a rebound name is no address
        assert_eq!(ask(&[], None), "403");
        assert_eq!(
            ask(&[("X-Tiquet", "1".into())], Some("evil.example:47713")),
            "403"
        );
        assert_eq!(ask(&[("X-Tiquet", "1".into())], None), "200");
        server.stop();
    }

    #[test]
    fn a_chunked_answer_is_read_whole() {
        assert_eq!(
            unchunk(b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"),
            b"hello world"
        );
    }
}
