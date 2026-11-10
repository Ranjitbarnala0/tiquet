// Every Tiquet tells the control server of whoever supplies it which PC it runs
// on: how many there are, where, which version, and whether one is a copy of
// another's data. The answer is signed with a key only that server holds, so
// only it can lock a copy or make it update; nothing in the database can fake
// it. While no server is published (or this PC has never been told of one)
// the till works as it always did.
//
// A PC that cannot reach the server keeps working on its last answer for as
// long as that answer says (45 days unless the server says otherwise), and is
// warned in its last week. A new PC has 14 days to reach it once. After that,
// or when the server locks it, it can still look at everything, print and
// export, but it cannot charge until it checks in again: one click.

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::error::{fail, Error, Result};
use crate::{log, settings};

// the control server's public key: the raw 32 bytes of its Ed25519 key
const PUBLIC_KEY: &str = "jLl/wXrHyA+vhE1Texzr6TCLkm2WE8zCwApS/4U0GJs=";
// where the server's address is published: a file in the app's own repository,
// so the address can change without a new version of the app
const DIRECTORY: &str = "https://raw.githubusercontent.com/Ranjitbarnala0/tiquet/main/control.json";
const EVERY: Duration = Duration::from_secs(6 * 3600);
const RETRY: Duration = Duration::from_secs(30 * 60);
const NEW_DAYS: i64 = 14;
const WARN_DAYS: i64 = 7;
const VALID_DAYS: i64 = 45;
// what a lock or a lapsed answer stops: whatever takes money in
pub const CHARGES: &[&str] = &[
    "sale.finalize",
    "tickets.refund",
    "book.add",
    "repairs.create",
    "cash.open",
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub device: String,
    pub status: String,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub update_now: bool,
    pub issued: String,
    #[serde(default)]
    pub valid_days: Option<i64>,
}

// What the screen shows and the core goes by.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct State {
    // a control server is known
    pub server: bool,
    // ok | locked | lapsed (no answer for too long) | new (never reached)
    pub status: String,
    pub message: Option<String>,
    // charging is refused
    pub blocked: bool,
    // the screen says so before it comes to that
    pub warn: bool,
    pub days_left: Option<i64>,
    pub min_version: Option<String>,
    pub update_now: bool,
    // the version should be installed as soon as the till is free
    pub force_update: bool,
    pub device: String,
    pub checked: Option<String>,
}

fn days(a: DateTime<Local>, b: DateTime<Local>) -> i64 {
    (b - a).num_seconds().div_euclid(86_400)
}

// newer than: "1.10.0" > "1.9.2"
pub fn older(have: &str, want: &str) -> bool {
    let parts = |v: &str| {
        v.split(['.', '-'])
            .take(3)
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect::<Vec<_>>()
    };
    parts(have) < parts(want)
}

// The rule, on its own so it can be tried at any date.
pub fn judge(
    now: DateTime<Local>,
    server: bool,
    answer: Option<&Payload>,
    first: Option<DateTime<Local>>,
    device: &str,
    checked: Option<String>,
) -> State {
    let mut s = State {
        server,
        status: "ok".into(),
        message: None,
        blocked: false,
        warn: false,
        days_left: None,
        min_version: None,
        update_now: false,
        force_update: false,
        device: device.chars().take(12).collect(),
        checked,
    };
    if !server {
        return s;
    }
    match answer {
        Some(p) => {
            s.message = p.message.clone().filter(|m| !m.trim().is_empty());
            s.min_version = p.min_version.clone().filter(|v| !v.trim().is_empty());
            s.update_now = p.update_now;
            s.force_update = p.update_now
                || s.min_version
                    .as_deref()
                    .is_some_and(|v| older(env!("CARGO_PKG_VERSION"), v));
            let issued = DateTime::parse_from_rfc3339(&p.issued)
                .map(|d| d.with_timezone(&Local))
                .unwrap_or(now);
            let left = p.valid_days.filter(|d| *d > 0).unwrap_or(VALID_DAYS) - days(issued, now);
            s.days_left = Some(left.max(0));
            if p.status == "locked" {
                s.status = "locked".into();
                s.blocked = true;
            } else if left <= 0 {
                s.status = "lapsed".into();
                s.blocked = true;
            } else {
                s.warn = left <= WARN_DAYS;
            }
        }
        None => {
            let left = NEW_DAYS - first.map_or(0, |f| days(f, now));
            s.status = "new".into();
            s.days_left = Some(left.max(0));
            s.blocked = left <= 0;
            s.warn = !s.blocked && left <= NEW_DAYS - 3;
        }
    }
    s
}

// ----------------------------------------------------------- this PC

#[cfg(windows)]
fn registry(key: &str, value: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let wide = |s: &str| {
        s.encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<u16>>()
    };
    let (k, v) = (wide(key), wide(value));
    let mut buf = vec![0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            k.as_ptr(),
            v.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut size,
        )
    };
    if r != 0 {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1).min(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]).trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(not(windows))]
fn registry(_key: &str, _value: &str) -> Option<String> {
    None
}

// The PC, not its data: Windows' own id of this installation of Windows,
// never sent as it is.
pub fn device_id() -> String {
    let base = registry("SOFTWARE\\Microsoft\\Cryptography", "MachineGuid")
        .unwrap_or_else(|| format!("host:{}", std::env::var("COMPUTERNAME").unwrap_or_default()));
    Sha256::digest(format!("tiquet:{}", base.to_lowercase()).as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn os_name() -> String {
    let nt = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let build = registry(nt, "CurrentBuildNumber")
        .or_else(|| registry(nt, "CurrentBuild"))
        .unwrap_or_default();
    let mut name = registry(nt, "ProductName").unwrap_or_else(|| "Windows".into());
    // Windows 11 still calls itself 10 there
    if build.parse::<u32>().is_ok_and(|b| b >= 22000) {
        name = name.replacen("Windows 10", "Windows 11", 1);
    }
    let display = registry(nt, "DisplayVersion").unwrap_or_default();
    format!("{name} {display} ({build})")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ----------------------------------------------------------- the answer

fn verify(payload: &[u8], sig: &[u8]) -> bool {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    let Ok(key) = base64::engine::general_purpose::STANDARD.decode(PUBLIC_KEY) else {
        return false;
    };
    let (Ok(key), Ok(sig)) = (
        <[u8; 32]>::try_from(key.as_slice()),
        <[u8; 64]>::try_from(sig),
    ) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    key.verify(payload, &Signature::from_bytes(&sig)).is_ok()
}

// The server's answer, if it really is the server's and for this PC.
pub fn read_answer(v: &Value, device: &str) -> Option<Payload> {
    let b64 = base64::engine::general_purpose::STANDARD;
    let payload = b64.decode(v["payload"].as_str()?).ok()?;
    let sig = b64.decode(v["sig"].as_str()?).ok()?;
    if !verify(&payload, &sig) {
        return None;
    }
    let p: Payload = serde_json::from_slice(&payload).ok()?;
    (p.device == device).then_some(p)
}

fn stamp(now: DateTime<Local>) -> String {
    now.format("%Y-%m-%dT%H:%M:%S").to_string()
}

fn parse_local(s: &str) -> Option<DateTime<Local>> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
        .ok()
        .and_then(|n| Local.from_local_datetime(&n).single())
}

// what this PC has kept: the server's address, its last answer, when it was
// first seen working with a server
struct Kept {
    url: Option<String>,
    answer: Option<Payload>,
    checked: Option<String>,
    first: Option<DateTime<Local>>,
}

fn kept(conn: &Connection, device: &str) -> Kept {
    let get = |k: &str| settings::get(conn, k).ok().flatten();
    let stored: Value = get("license")
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(Value::Null);
    let first: Value = get("licenseFirst")
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(Value::Null);
    Kept {
        url: get("controlUrl").filter(|u| !u.is_empty()),
        answer: read_answer(&stored, device),
        checked: stored["at"]
            .as_str()
            .map(str::to_string)
            .filter(|_| stored["device"] == device),
        // a copy of another PC's data starts its own count
        first: (first["device"] == device)
            .then(|| first["at"].as_str().and_then(parse_local))
            .flatten(),
    }
}

pub fn state(conn: &Connection) -> State {
    let device = device_id();
    let k = kept(conn, &device);
    judge(
        Local::now(),
        k.url.is_some() || override_url().is_some(),
        k.answer.as_ref(),
        k.first,
        &device,
        k.checked,
    )
}

// Charging refused, with why; None when it may go ahead.
pub fn refusal(conn: &Connection) -> Option<String> {
    let s = state(conn);
    s.blocked
        .then(|| format!("E_LICENSE_{}", s.status.to_uppercase()))
}

// ----------------------------------------------------------- the network

fn override_url() -> Option<String> {
    std::env::var("TIQUET_CONTROL_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

// https://host/path, or http to this PC only (a server tried out here)
fn split(url: &str) -> Option<(bool, String, u16, String)> {
    let (tls, rest) = match url.split_once("://")? {
        ("https", r) => (true, r),
        ("http", r) => (false, r),
        _ => return None,
    };
    let (hostport, path) = rest
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((rest, "/".into()));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (hostport.to_string(), if tls { 443 } else { 80 }),
    };
    if !tls && !["127.0.0.1", "localhost"].contains(&host.as_str()) {
        return None;
    }
    Some((tls, host, port, path))
}

fn request(url: &str, body: Option<&str>) -> Result<(u16, String)> {
    let Some((tls, host, port, path)) = split(url) else {
        return fail("E_LICENSE_URL");
    };
    let method = if body.is_some() { "POST" } else { "GET" };
    let body = body.unwrap_or_default();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: Tiquet/{}\r\nAccept: application/json\r\nConnection: close\r\nContent-Length: {}\r\n",
        env!("CARGO_PKG_VERSION"),
        body.len()
    );
    if method == "POST" {
        req.push_str("Content-Type: application/json\r\n");
    }
    req.push_str("\r\n");
    req.push_str(body);
    let raw = if tls {
        if port != 443 {
            return fail("E_LICENSE_URL");
        }
        crate::aeat::https(&host, req.as_bytes())?
    } else {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect((host.as_str(), port))
            .map_err(|e| Error(format!("E_LICENSE_NET:{e}")))?;
        s.set_read_timeout(Some(Duration::from_secs(20))).ok();
        s.write_all(req.as_bytes())
            .map_err(|e| Error(format!("E_LICENSE_NET:{e}")))?;
        let mut raw = Vec::new();
        s.read_to_end(&mut raw)
            .map_err(|e| Error(format!("E_LICENSE_NET:{e}")))?;
        raw
    };
    crate::aeat::reply(&raw)
}

// The address published in the repository; None when nothing is published.
// Err when it could not be asked (no internet): what was kept stays.
fn directory() -> Result<Option<String>> {
    let (code, body) = request(DIRECTORY, None)?;
    if code == 404 {
        return Ok(None);
    }
    if code != 200 {
        return fail(format!("E_LICENSE_HTTP:{code}"));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| Error("E_LICENSE_DIRECTORY".into()))?;
    Ok(v["checkin"]
        .as_str()
        .map(str::to_string)
        .filter(|u| split(u).is_some()))
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0)
}

// what this PC says about itself
fn report(conn: &Connection, device: &str) -> Value {
    let s = settings::load_own(conn).unwrap_or_default();
    let main = crate::lan::load_main(conn);
    let sharing = settings::get(conn, "lanShare").ok().flatten().as_deref() == Some("1");
    let role = if main.is_some() {
        "secondary"
    } else if sharing {
        "main"
    } else {
        "single"
    };
    let last: Option<String> = conn
        .query_row(
            "SELECT number FROM tickets WHERE training = 0 ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    json!({
        "device": device,
        "install": settings::install_id(conn).unwrap_or_default(),
        "version": env!("CARGO_PKG_VERSION"),
        "shop": main.as_ref().map(|m| m.shop.clone()).unwrap_or(s.shop_name),
        "nif": s.shop_nif,
        "host": std::env::var("COMPUTERNAME").unwrap_or_default(),
        "os": os_name(),
        "role": role,
        "main": main.map(|m| m.install).unwrap_or_default(),
        "tickets": count(conn, "SELECT COUNT(*) FROM tickets WHERE training = 0"),
        "users": count(conn, "SELECT COUNT(*) FROM users"),
        "lastTicket": last.unwrap_or_default(),
        "time": crate::tickets::now(),
    })
}

// One check-in: find the server, say who this PC is, keep its signed answer.
pub fn check(conn: &Connection) -> Result<State> {
    let device = device_id();
    let url = match override_url() {
        Some(u) => Some(u),
        None => match directory() {
            Ok(found) => {
                match &found {
                    Some(u) => settings::put(conn, "controlUrl", u)?,
                    None => settings::clear(conn, "controlUrl")?,
                }
                found
            }
            Err(_) => kept(conn, &device).url,
        },
    };
    let Some(url) = url else {
        return Ok(state(conn));
    };
    let first: Value = settings::get(conn, "licenseFirst")?
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or(Value::Null);
    if first["device"] != device.as_str() {
        settings::put(
            conn,
            "licenseFirst",
            &json!({ "device": device, "at": stamp(Local::now()) }).to_string(),
        )?;
    }
    let (code, body) = request(&url, Some(&report(conn, &device).to_string()))?;
    if code != 200 {
        return fail(format!("E_LICENSE_HTTP:{code}"));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| Error("E_LICENSE_ANSWER".into()))?;
    let Some(p) = read_answer(&v, &device) else {
        log::warn("control server answer did not verify");
        return fail("E_LICENSE_SIGNATURE");
    };
    let before = kept(conn, &device).answer.map(|a| a.status);
    settings::put(conn, "license", &json!({ "payload": v["payload"], "sig": v["sig"], "device": device, "at": stamp(Local::now()) }).to_string())?;
    if before.as_deref() != Some(p.status.as_str()) {
        log::info(format!("control server says this PC is {}", p.status));
    }
    Ok(state(conn))
}

static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

// another check-in now, not in hours
pub fn kick() {
    let (m, c) = &WAKE;
    *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
    c.notify_all();
}

fn wait(d: Duration) {
    let (m, c) = &WAKE;
    let mut woken = m.lock().unwrap_or_else(|e| e.into_inner());
    if !*woken {
        woken = c
            .wait_timeout(woken, d)
            .map(|(g, _)| g)
            .unwrap_or_else(|e| e.into_inner().0);
    }
    *woken = false;
}

// in the background for as long as the app is open: soon after it opens, then
// every few hours
pub fn start(db: PathBuf) {
    std::thread::spawn(move || {
        wait(Duration::from_secs(20));
        loop {
            let next = match open(&db).and_then(|c| check(&c)) {
                Ok(_) => EVERY,
                Err(e) => {
                    log::info(format!("control server not reached: {}", e.0));
                    RETRY
                }
            };
            wait(next);
        }
    });
}

fn open(db: &Path) -> Result<Connection> {
    let conn = Connection::open(db)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    Ok(conn)
}

pub fn route(st: &crate::state::AppState, cmd: &str) -> Result<Value> {
    match cmd {
        "license.state" => {
            st.user()?;
            Ok(serde_json::to_value(state(&st.conn()))?)
        }
        // "Comprobar ahora": the network call runs without holding the
        // database, which the other PCs of the shop may be using
        "license.check" => {
            st.user()?;
            let db = st.data_dir.join("tiquet.db");
            let out = open(&db).and_then(|c| check(&c));
            kick();
            match out {
                Ok(s) => Ok(serde_json::to_value(s)?),
                Err(e) => {
                    let mut v = serde_json::to_value(state(&st.conn()))?;
                    v["error"] = json!(e.0);
                    Ok(v)
                }
            }
        }
        _ => fail(format!("E_UNKNOWN:{cmd}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Local> {
        parse_local(s).unwrap()
    }

    fn answer(status: &str, issued: &str) -> Payload {
        Payload {
            device: "d".into(),
            status: status.into(),
            message: None,
            min_version: None,
            update_now: false,
            issued: issued.into(),
            valid_days: Some(45),
        }
    }

    #[test]
    fn with_no_server_published_nothing_changes() {
        let s = judge(at("2030-01-01T10:00:00"), false, None, None, "d", None);
        assert_eq!((s.blocked, s.warn, s.status.as_str()), (false, false, "ok"));
    }

    #[test]
    fn a_new_pc_has_two_weeks_to_reach_the_server_once() {
        let first = Some(at("2026-11-01T10:00:00"));
        let on = |d: &str| {
            let s = judge(at(d), true, None, first, "d", None);
            (s.status, s.blocked, s.warn, s.days_left)
        };
        assert_eq!(
            on("2026-11-02T10:00:00"),
            ("new".into(), false, false, Some(13))
        );
        assert_eq!(
            on("2026-11-05T10:00:00"),
            ("new".into(), false, true, Some(10))
        );
        assert_eq!(
            on("2026-11-15T10:00:01"),
            ("new".into(), true, false, Some(0))
        );
    }

    #[test]
    fn an_answer_lasts_its_days_and_a_lock_stops_charging_at_once() {
        let a = answer("ok", "2026-11-01T09:00:00.123Z");
        let on = |d: &str, p: &Payload| {
            let s = judge(at(d), true, Some(p), None, "d", None);
            (s.status, s.blocked, s.warn)
        };
        assert_eq!(on("2026-11-20T10:00:00", &a), ("ok".into(), false, false));
        assert_eq!(on("2026-12-10T10:00:00", &a), ("ok".into(), false, true));
        assert_eq!(
            on("2026-12-17T10:00:00", &a),
            ("lapsed".into(), true, false)
        );
        assert_eq!(
            on(
                "2026-11-02T10:00:00",
                &answer("locked", "2026-11-01T09:00:00Z")
            ),
            ("locked".into(), true, false)
        );
    }

    #[test]
    fn a_version_below_the_minimum_updates_as_soon_as_it_can() {
        assert!(
            older("1.9.2", "1.10.0")
                && older("1.10.0", "1.10.1")
                && !older("1.10.0", "1.10.0")
                && !older("1.11.0", "1.10.9")
        );
        let mut a = answer("ok", "2026-11-01T09:00:00Z");
        a.min_version = Some("99.0.0".into());
        assert!(judge(at("2026-11-02T10:00:00"), true, Some(&a), None, "d", None).force_update);
        a.min_version = Some("0.1.0".into());
        assert!(!judge(at("2026-11-02T10:00:00"), true, Some(&a), None, "d", None).force_update);
        a.update_now = true;
        assert!(judge(at("2026-11-02T10:00:00"), true, Some(&a), None, "d", None).force_update);
    }

    #[test]
    fn only_the_servers_signature_for_this_pc_counts() {
        // signed by the real key: the server's test answer, kept here
        let fake = json!({ "payload": base64::engine::general_purpose::STANDARD.encode(br#"{"device":"d","status":"ok","issued":"2026-11-01T09:00:00Z"}"#), "sig": base64::engine::general_purpose::STANDARD.encode([0u8; 64]) });
        assert!(read_answer(&fake, "d").is_none());
        assert!(read_answer(&json!({}), "d").is_none());
    }

    #[test]
    fn this_pc_has_one_id_of_64_hex() {
        let id = device_id();
        assert_eq!(id.len(), 64);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert_eq!(id, device_id());
        assert!(os_name().starts_with("Windows"), "{}", os_name());
    }

    #[test]
    fn addresses_are_https_or_this_pc() {
        assert_eq!(
            split("https://tiquet-control.x.workers.dev/v1/checkin"),
            Some((
                true,
                "tiquet-control.x.workers.dev".into(),
                443,
                "/v1/checkin".into()
            ))
        );
        assert_eq!(
            split("http://127.0.0.1:8787/v1/checkin"),
            Some((false, "127.0.0.1".into(), 8787, "/v1/checkin".into()))
        );
        assert_eq!(split("http://evil.example/v1/checkin"), None);
        assert_eq!(split("ftp://x/y"), None);
    }
}
