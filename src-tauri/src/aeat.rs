// The VERI*FACTU web service. Billing records go to the AEAT as soon as they
// are made, oldest first, up to 1,000 a message, and never sooner after the
// previous message than the AEAT's last answer said (60 s to begin with, OM
// art. 16). Without internet or a certificate the till keeps selling: the
// records wait, Ajustes says why, and they go out later in a message marked
// as delayed by an incident. The sender opens its own connection for each
// round and holds nothing while it talks to the AEAT.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local};
use rusqlite::Connection;
use serde::Serialize;

use crate::error::{fail, Error, Result};
use crate::settings;
use crate::verifactu::{self, esc, Answer, Env, Pending};
use crate::{log, tickets};

const NS_SUM: &str = "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroLR.xsd";
const NS_SUM1: &str =
    "https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroInformacion.xsd";

const MAX_RECORDS: usize = 1000;

// RegFactuSistemaFacturacion: who issued the records, and the records as stored
pub fn envelope(name: &str, nif: &str, records: &[Pending], incident: bool) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <soapenv:Envelope xmlns:soapenv=\"http://schemas.xmlsoap.org/soap/envelope/\" xmlns:sum=\"{NS_SUM}\" xmlns:sum1=\"{NS_SUM1}\">\
         <soapenv:Header/><soapenv:Body><sum:RegFactuSistemaFacturacion><sum:Cabecera>\
         <sum1:ObligadoEmision><sum1:NombreRazon>{}</sum1:NombreRazon><sum1:NIF>{}</sum1:NIF></sum1:ObligadoEmision>",
        esc(name),
        esc(nif)
    );
    if incident {
        out.push_str("<sum1:RemisionVoluntaria><sum1:Incidencia>S</sum1:Incidencia></sum1:RemisionVoluntaria>");
    }
    out.push_str("</sum:Cabecera>");
    for r in records {
        out.push_str("<sum:RegistroFactura>");
        out.push_str(&r.xml);
        out.push_str("</sum:RegistroFactura>");
    }
    out.push_str("</sum:RegFactuSistemaFacturacion></soapenv:Body></soapenv:Envelope>");
    out
}

// what the AEAT said about one record
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Line {
    pub number: String,
    pub date: String,
    // Alta | Anulacion
    pub operation: String,
    pub state: String,
    pub code: Option<String>,
    pub message: Option<String>,
    // how the record already there went, when this one was a duplicate (3000)
    pub duplicate: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Reply {
    // a whole message refused: faultcode and faultstring
    pub fault: Option<(String, String)>,
    pub wait: Option<u64>,
    pub state: Option<String>,
    pub csv: Option<String>,
    // the AEAT's clock when it took the message in
    pub received_at: Option<String>,
    pub lines: Vec<Line>,
}

type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

fn find<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.descendants().find(|d| d.tag_name().name() == name)
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|d| d.tag_name().name() == name)
}

fn text(n: Option<Node>) -> Option<String> {
    n.and_then(|n| n.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

// RespuestaRegFactuSistemaFacturacion, or a SOAP Fault, read by element names
// whatever prefixes the AEAT uses
pub fn parse(xml: &str) -> Result<Reply> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error(format!("E_AEAT_REPLY:{e}")))?;
    let root = doc.root();
    if let Some(f) = find(root, "Fault") {
        let code = text(child(f, "faultcode")).unwrap_or_default();
        let msg = text(child(f, "faultstring")).unwrap_or_default();
        return Ok(Reply { fault: Some((code, msg)), ..Reply::default() });
    }
    let Some(body) = find(root, "RespuestaRegFactuSistemaFacturacion") else {
        return fail("E_AEAT_REPLY:unexpected");
    };
    let mut reply = Reply {
        wait: text(child(body, "TiempoEsperaEnvio")).and_then(|t| t.parse().ok()),
        state: text(child(body, "EstadoEnvio")),
        csv: text(child(body, "CSV")),
        received_at: text(find(body, "TimestampPresentacion")),
        ..Reply::default()
    };
    for l in body.children().filter(|n| n.tag_name().name() == "RespuestaLinea") {
        let id = child(l, "IDFactura");
        reply.lines.push(Line {
            number: text(id.and_then(|i| child(i, "NumSerieFactura"))).unwrap_or_default(),
            date: text(id.and_then(|i| child(i, "FechaExpedicionFactura"))).unwrap_or_default(),
            operation: text(child(l, "Operacion").and_then(|o| child(o, "TipoOperacion"))).unwrap_or_default(),
            state: text(child(l, "EstadoRegistro")).unwrap_or_default(),
            code: text(child(l, "CodigoErrorRegistro")),
            message: text(child(l, "DescripcionErrorRegistro")),
            duplicate: text(child(l, "RegistroDuplicado").and_then(|d| child(d, "EstadoRegistroDuplicado"))),
        });
    }
    Ok(reply)
}

// Each line answers the first record of the message with the same invoice
// and operation. A record sent twice (3000) counts as however the first
// copy went; an anulación of an invoice already cancelled (3001) is done.
pub fn answers(batch: &[Pending], reply: &Reply) -> Vec<Answer> {
    let mut used = vec![false; batch.len()];
    let mut out = Vec::new();
    for l in &reply.lines {
        let kind = if l.operation == "Anulacion" { "anulacion" } else { "alta" };
        let Some(i) = (0..batch.len()).find(|&i| !used[i] && batch[i].kind == kind && batch[i].number == l.number && batch[i].date == l.date) else {
            continue;
        };
        let state = match (l.code.as_deref(), l.duplicate.as_deref()) {
            (Some("3000"), Some("AceptadaConErrores")) => "AceptadoConErrores",
            (Some("3000"), Some(_)) => "Correcto",
            (Some("3001"), _) if kind == "anulacion" => "Correcto",
            _ => l.state.as_str(),
        };
        if !["Correcto", "AceptadoConErrores", "Incorrecto"].contains(&state) {
            continue;
        }
        used[i] = true;
        out.push(Answer { record_id: batch[i].id, state: state.into(), code: l.code.clone(), message: l.message.clone() });
    }
    out
}

// The HTTP status and body of a reply read until the server closed it.
fn http_reply(raw: &[u8]) -> Result<(u16, String)> {
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return fail("E_AEAT_HTTP:incomplete");
    };
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or_else(|| Error("E_AEAT_HTTP:status".into()))?;
    let mut body = &raw[split + 4..];
    let chunked = head.lines().any(|l| l.starts_with("transfer-encoding:") && l.contains("chunked"));
    if !chunked {
        return Ok((status, String::from_utf8_lossy(body).into_owned()));
    }
    let mut out = Vec::with_capacity(body.len());
    while let Some(eol) = body.windows(2).position(|w| w == b"\r\n") {
        let size = String::from_utf8_lossy(&body[..eol]);
        let size = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16).map_err(|_| Error("E_AEAT_HTTP:chunk".into()))?;
        body = &body[eol + 2..];
        if size == 0 || size > body.len() {
            out.extend_from_slice(&body[..size.min(body.len())]);
            break;
        }
        out.extend_from_slice(&body[..size]);
        body = body.get(size + 2..).unwrap_or_default();
    }
    Ok((status, String::from_utf8_lossy(&out).into_owned()))
}

// someone a certificate is for, as Ajustes lists them
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Cert {
    pub thumbprint: String,
    pub name: String,
    pub issuer: String,
    // yyyy-mm-dd
    pub expires: String,
    pub valid: bool,
}

// Just enough DER to read whom a certificate is for, who issued it and until
// when: tag, contents and what follows.
fn tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let tag = *b.first()?;
    let first = *b.get(1)? as usize;
    let (len, head) = if first < 0x80 {
        (first, 2)
    } else {
        let n = first & 0x7f;
        if n == 0 || n > 4 {
            return None;
        }
        (b.get(2..2 + n)?.iter().fold(0usize, |l, x| l << 8 | *x as usize), 2 + n)
    };
    Some((tag, b.get(head..head + len)?, b.get(head + len..)?))
}

fn der_text(tag: u8, v: &[u8]) -> String {
    if tag == 0x1E {
        let units: Vec<u16> = v.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(v).into_owned()
}

// the common name, or the organisation when there is none
fn der_name(mut rdns: &[u8]) -> String {
    let (mut cn, mut org) = (None, None);
    while let Some((_, set, rest)) = tlv(rdns) {
        rdns = rest;
        let Some((_, pair, _)) = tlv(set) else { continue };
        let Some((_, oid, value)) = tlv(pair) else { continue };
        let Some((tag, v, _)) = tlv(value) else { continue };
        match oid {
            [0x55, 0x04, 0x03] => cn = Some(der_text(tag, v)),
            [0x55, 0x04, 0x0A] => org = Some(der_text(tag, v)),
            _ => {}
        }
    }
    cn.or(org).unwrap_or_default()
}

fn der_date(tag: u8, v: &[u8]) -> String {
    let s = String::from_utf8_lossy(v);
    match (tag, s.get(..2), s.get(..4)) {
        (0x17, Some(yy), _) => {
            let y: u32 = yy.parse().unwrap_or(0);
            format!("{}-{}-{}", if y < 50 { 2000 + y } else { 1900 + y }, s.get(2..4).unwrap_or("01"), s.get(4..6).unwrap_or("01"))
        }
        (_, _, Some(yyyy)) => format!("{yyyy}-{}-{}", s.get(4..6).unwrap_or("01"), s.get(6..8).unwrap_or("01")),
        _ => String::new(),
    }
}

// (subject, issuer, not after)
pub fn describe(der: &[u8]) -> Option<(String, String, String)> {
    let (_, cert, _) = tlv(der)?;
    let (_, tbs, _) = tlv(cert)?;
    let mut rest = tbs;
    if tlv(rest)?.0 == 0xA0 {
        rest = tlv(rest)?.2;
    }
    rest = tlv(rest)?.2; // serial number
    rest = tlv(rest)?.2; // signature algorithm
    let (_, issuer, after) = tlv(rest)?;
    let (_, validity, after) = tlv(after)?;
    let (_, subject, _) = tlv(after)?;
    let (_, _, until) = tlv(validity)?;
    let (tag, not_after, _) = tlv(until)?;
    Some((der_name(subject), der_name(issuer), der_date(tag, not_after)))
}

#[cfg(windows)]
mod tls {
    use std::io::{self, Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    use schannel::cert_context::{CertContext, HashAlgorithm, ValidUses};
    use schannel::cert_store::CertStore;
    use schannel::schannel_cred::{Direction, SchannelCred};
    use schannel::tls_stream::{Builder, HandshakeError};

    use super::{describe, Cert};
    use crate::error::{fail, Error, Result};

    // extended key usage "TLS client authentication"
    const CLIENT_AUTH: &str = "1.3.6.1.5.5.7.3.2";

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02X}")).collect()
    }

    // the thumbprint Windows shows for it
    fn thumbprint(c: &CertContext) -> Option<String> {
        c.fingerprint(HashAlgorithm::sha1()).ok().map(|h| hex(&h))
    }

    // Personal certificates of this Windows user that may identify a client
    // and whose private key can be used without asking anyone: the ones a
    // till can send with on its own. Server certificates are left out.
    pub fn certificates() -> Vec<Cert> {
        let Ok(store) = CertStore::open_current_user("My") else { return Vec::new() };
        let mut out = Vec::new();
        for c in store.certs() {
            let client = match c.valid_uses() {
                Ok(ValidUses::All) => true,
                Ok(ValidUses::Oids(oids)) => oids.iter().any(|o| o == CLIENT_AUTH),
                Err(_) => false,
            };
            if !client || c.private_key().silent(true).acquire().is_err() {
                continue;
            }
            let Some(print) = thumbprint(&c) else { continue };
            let (name, issuer, expires) = describe(c.to_der()).unwrap_or_default();
            let name = if name.is_empty() { c.friendly_name().unwrap_or_default() } else { name };
            out.push(Cert { thumbprint: print, name, issuer, expires, valid: c.is_time_valid().unwrap_or(false) });
        }
        out.sort_by(|a, b| b.valid.cmp(&a.valid).then(b.expires.cmp(&a.expires)));
        out
    }

    fn find(thumbprint: &str) -> Result<CertContext> {
        let store = CertStore::open_current_user("My").map_err(|e| Error(format!("E_AEAT_CERT:{e}")))?;
        let found = store.certs().find(|c| self::thumbprint(c).is_some_and(|h| h.eq_ignore_ascii_case(thumbprint)));
        match found {
            Some(c) => Ok(c),
            None => fail("E_AEAT_CERT:missing"),
        }
    }

    // One HTTP/1.1 request over TLS, presenting the certificate when there is one.
    pub fn exchange(host: &str, request: &[u8], thumbprint: Option<&str>) -> Result<Vec<u8>> {
        let mut cred = SchannelCred::builder();
        if let Some(t) = thumbprint {
            cred.cert(find(t)?);
        }
        let cred = cred.acquire(Direction::Outbound).map_err(|e| Error(format!("E_AEAT_CERT:{e}")))?;
        let net = |e: io::Error| Error(format!("E_AEAT_NET:{e}"));
        let addr = (host, 443).to_socket_addrs().map_err(net)?.next().ok_or_else(|| Error("E_AEAT_NET:dns".into()))?;
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(15)).map_err(net)?;
        tcp.set_read_timeout(Some(Duration::from_secs(90))).map_err(net)?;
        tcp.set_write_timeout(Some(Duration::from_secs(30))).map_err(net)?;
        let mut tls = Builder::new().domain(host).connect(cred, tcp).map_err(|e| match e {
            HandshakeError::Failure(e) => Error(format!("E_AEAT_TLS:{e}")),
            HandshakeError::Interrupted(_) => Error("E_AEAT_TLS:interrupted".into()),
        })?;
        tls.write_all(request).and_then(|_| tls.flush()).map_err(net)?;
        let mut raw = Vec::new();
        // some servers drop the connection without closing TLS politely
        if let Err(e) = tls.read_to_end(&mut raw) {
            if raw.is_empty() {
                return Err(net(e));
            }
        }
        Ok(raw)
    }
}

#[cfg(not(windows))]
mod tls {
    use super::Cert;
    use crate::error::{fail, Result};

    pub fn certificates() -> Vec<Cert> {
        Vec::new()
    }

    pub fn exchange(_host: &str, _request: &[u8], _thumbprint: Option<&str>) -> Result<Vec<u8>> {
        fail("E_AEAT_CERT:windows")
    }
}

pub use tls::certificates;

fn post(env: Env, body: &str, thumbprint: &str) -> Result<(u16, String)> {
    let host = env.host();
    let mut request = format!(
        "POST {} HTTP/1.1\r\nHost: {host}\r\nContent-Type: text/xml; charset=utf-8\r\nSOAPAction: \"\"\r\n\
         Content-Length: {}\r\nConnection: close\r\nUser-Agent: Tiquet/{}\r\n\r\n",
        verifactu::SOAP_PATH,
        body.len(),
        env!("CARGO_PKG_VERSION")
    )
    .into_bytes();
    request.extend_from_slice(body.as_bytes());
    http_reply(&tls::exchange(host, &request, Some(thumbprint))?)
}

// a GET without a certificate, to see that the PC reaches the AEAT at all
pub fn reach(env: Env) -> Result<u16> {
    let host = match env {
        Env::Test => "prewww2.aeat.es",
        Env::Prod => "www2.agenciatributaria.gob.es",
    };
    let request = format!(
        "GET /static_files/common/internet/dep/aplicaciones/es/aeat/tikeV1.0/cont/ws/SistemaFacturacion.wsdl HTTP/1.1\r\n\
         Host: {host}\r\nConnection: close\r\nUser-Agent: Tiquet/{}\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    Ok(http_reply(&tls::exchange(host, request.as_bytes(), None)?)?.0)
}

struct Wake {
    kicked: Mutex<bool>,
    cv: Condvar,
}

static WAKE: Wake = Wake { kicked: Mutex::new(false), cv: Condvar::new() };
static STARTED: AtomicBool = AtomicBool::new(false);

// a record was just committed: the sender need not wait for its next round
pub fn kick() {
    *WAKE.kicked.lock().unwrap_or_else(|e| e.into_inner()) = true;
    WAKE.cv.notify_all();
}

fn sleep(max: Duration) {
    let guard = WAKE.kicked.lock().unwrap_or_else(|e| e.into_inner());
    let (mut kicked, _) = WAKE.cv.wait_timeout_while(guard, max, |k| !*k).unwrap_or_else(|e| e.into_inner());
    *kicked = false;
}

// once per process, for the database at `db`
pub fn start(db: PathBuf) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new().name("aeat".into()).spawn(move || run(&db));
    if let Err(e) = spawned {
        log::error(format!("VERI*FACTU sender did not start: {e}"));
    }
}

// a failure waits longer each time, up to eight minutes: the
// rule is at least one try an hour
fn backoff(failures: u32) -> Duration {
    Duration::from_secs((60u64 << failures.min(4).saturating_sub(1)).min(900))
}

fn run(db: &Path) {
    let envs = [Env::Prod, Env::Test];
    let mut next = [Instant::now(); 2];
    let mut failures = [0u32; 2];
    loop {
        let soonest = next.iter().min().copied().unwrap_or_else(Instant::now);
        sleep(soonest.saturating_duration_since(Instant::now()).clamp(Duration::from_secs(1), Duration::from_secs(30)));
        for (i, env) in envs.iter().enumerate() {
            if Instant::now() < next[i] {
                continue;
            }
            match round(db, *env, failures[i] > 0) {
                Ok(None) => {}
                Ok(Some(wait)) => {
                    failures[i] = 0;
                    next[i] = Instant::now() + wait;
                }
                Err(e) => {
                    failures[i] += 1;
                    next[i] = Instant::now() + backoff(failures[i]);
                    if failures[i] == 1 || failures[i] % 10 == 0 {
                        log::warn(format!("VERI*FACTU {}: {} (attempt {})", env.code(), e.0, failures[i]));
                    }
                    if let Ok(conn) = open(db) {
                        let _ = settings::put(&conn, "aeatLastError", &format!("{} {}", tickets::now(), e.0));
                    }
                }
            }
        }
    }
}

fn open(db: &Path) -> Result<Connection> {
    let conn = Connection::open(db)?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    Ok(conn)
}

fn older_than(generated_at: &str, secs: i64) -> bool {
    DateTime::parse_from_rfc3339(generated_at).is_ok_and(|t| Local::now().signed_duration_since(t).num_seconds() > secs)
}

// One message for this environment, if there is anything to send. Returns
// how long the AEAT wants before the next one.
fn round(db: &Path, env: Env, retrying: bool) -> Result<Option<Duration>> {
    let conn = open(db)?;
    let has: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name = 'fiscal_records')", [], |r| r.get(0))?;
    if !has {
        return Ok(None);
    }
    let mut batch = verifactu::pending(&conn, env, MAX_RECORDS as i64)?;
    let Some(first) = batch.first().cloned() else { return Ok(None) };
    // one issuer per message, as its header says
    if let Some(cut) = batch.iter().position(|p| p.issuer_nif != first.issuer_nif) {
        batch.truncate(cut);
    }
    let s = settings::load(&conn)?;
    if s.cert.is_empty() {
        return fail("E_AEAT_CERT:none");
    }
    let incident = retrying || older_than(&first.generated_at, 300);
    let name = verifactu::issuer_name(&s.legal_name, &s.shop_name);
    let body = envelope(&name, &first.issuer_nif, &batch, incident);
    drop(conn);

    let (status, text) = post(env, &body, &s.cert)?;
    let reply = match parse(&text) {
        Ok(r) => r,
        Err(_) => return fail(format!("E_AEAT_HTTP:{status}")),
    };
    if let Some((code, msg)) = &reply.fault {
        return fail(format!("E_AEAT_FAULT:{code}: {}", msg.split_whitespace().collect::<Vec<_>>().join(" ")));
    }
    let answers = answers(&batch, &reply);
    let conn = open(db)?;
    verifactu::answered(&conn, &answers, reply.csv.as_deref())?;
    let now = tickets::now();
    settings::put(&conn, "aeatLastSend", &now)?;
    conn.execute("DELETE FROM settings WHERE key = 'aeatLastError'", [])?;
    // the PC clock may be at most a minute off the official time
    match reply.received_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()) {
        Some(t) if Local::now().signed_duration_since(t).num_seconds().abs() > 60 => {
            let drift = Local::now().signed_duration_since(t).num_seconds();
            settings::put(&conn, "aeatClockDrift", &drift.to_string())?;
        }
        Some(_) => {
            conn.execute("DELETE FROM settings WHERE key = 'aeatClockDrift'", [])?;
        }
        None => {}
    }
    let refused = answers.iter().filter(|a| a.state != "Correcto").count();
    log::info(format!(
        "VERI*FACTU {}: {} records sent, {} answered, {refused} with errors or refused{}",
        env.code(),
        batch.len(),
        answers.len(),
        if incident { " (late, incident)" } else { "" }
    ));
    Ok(Some(Duration::from_secs(reply.wait.unwrap_or(60).clamp(1, 3600))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(id: i64, kind: &str, number: &str) -> Pending {
        Pending {
            id,
            issuer_nif: "89890001K".into(),
            kind: kind.into(),
            number: number.into(),
            date: "17-10-2026".into(),
            generated_at: "2026-10-17T10:00:00+02:00".into(),
            xml: format!("<sum1:RegistroAlta><sum1:IDVersion>1.0</sum1:IDVersion>{number}</sum1:RegistroAlta>"),
        }
    }

    // RespuestaRegFactuSistemaFacturacion as RespuestaSuministro.xsd lays it out
    const REPLY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<env:Envelope xmlns:env="http://schemas.xmlsoap.org/soap/envelope/"><env:Header/><env:Body Id="Body">
<tikR:RespuestaRegFactuSistemaFacturacion xmlns:tikR="https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/RespuestaSuministro.xsd" xmlns:tik="https://www2.agenciatributaria.gob.es/static_files/common/internet/dep/aplicaciones/es/aeat/tike/cont/ws/SuministroInformacion.xsd">
<tikR:CSV>A-YDSW8NLFLANWPM</tikR:CSV>
<tikR:DatosPresentacion><tik:NIFPresentador>89890001K</tik:NIFPresentador><tik:TimestampPresentacion>2026-10-17T10:00:05+02:00</tik:TimestampPresentacion></tikR:DatosPresentacion>
<tikR:Cabecera><tik:ObligadoEmision><tik:NombreRazon>Mi Tienda Móvil</tik:NombreRazon><tik:NIF>89890001K</tik:NIF></tik:ObligadoEmision></tikR:Cabecera>
<tikR:TiempoEsperaEnvio>80</tikR:TiempoEsperaEnvio>
<tikR:EstadoEnvio>ParcialmenteCorrecto</tikR:EstadoEnvio>
<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>89890001K</tik:IDEmisorFactura><tik:NumSerieFactura>T2026-000001</tik:NumSerieFactura><tik:FechaExpedicionFactura>17-10-2026</tik:FechaExpedicionFactura></tikR:IDFactura>
<tikR:Operacion><tik:TipoOperacion>Alta</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Correcto</tikR:EstadoRegistro></tikR:RespuestaLinea>
<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>89890001K</tik:IDEmisorFactura><tik:NumSerieFactura>T2026-000002</tik:NumSerieFactura><tik:FechaExpedicionFactura>17-10-2026</tik:FechaExpedicionFactura></tikR:IDFactura>
<tikR:Operacion><tik:TipoOperacion>Alta</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Incorrecto</tikR:EstadoRegistro>
<tikR:CodigoErrorRegistro>1124</tikR:CodigoErrorRegistro><tikR:DescripcionErrorRegistro>El valor del campo TipoImpositivo no está incluido en la lista de valores permitidos.</tikR:DescripcionErrorRegistro></tikR:RespuestaLinea>
<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>89890001K</tik:IDEmisorFactura><tik:NumSerieFactura>T2026-000001</tik:NumSerieFactura><tik:FechaExpedicionFactura>17-10-2026</tik:FechaExpedicionFactura></tikR:IDFactura>
<tikR:Operacion><tik:TipoOperacion>Anulacion</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Incorrecto</tikR:EstadoRegistro>
<tikR:CodigoErrorRegistro>3001</tikR:CodigoErrorRegistro><tikR:DescripcionErrorRegistro>El registro ya ha sido anulado.</tikR:DescripcionErrorRegistro></tikR:RespuestaLinea>
<tikR:RespuestaLinea><tikR:IDFactura><tik:IDEmisorFactura>89890001K</tik:IDEmisorFactura><tik:NumSerieFactura>T2026-000003</tik:NumSerieFactura><tik:FechaExpedicionFactura>17-10-2026</tik:FechaExpedicionFactura></tikR:IDFactura>
<tikR:Operacion><tik:TipoOperacion>Alta</tik:TipoOperacion></tikR:Operacion><tikR:EstadoRegistro>Incorrecto</tikR:EstadoRegistro>
<tikR:CodigoErrorRegistro>3000</tikR:CodigoErrorRegistro><tikR:DescripcionErrorRegistro>Registro de facturación duplicado.</tikR:DescripcionErrorRegistro>
<tikR:RegistroDuplicado><tik:IdPeticionRegistroDuplicado>202610171000</tik:IdPeticionRegistroDuplicado><tik:EstadoRegistroDuplicado>Correcta</tik:EstadoRegistroDuplicado></tikR:RegistroDuplicado></tikR:RespuestaLinea>
</tikR:RespuestaRegFactuSistemaFacturacion></env:Body></env:Envelope>"#;

    #[test]
    fn a_reply_is_matched_record_by_record() {
        let reply = parse(REPLY).unwrap();
        assert_eq!(reply.wait, Some(80));
        assert_eq!(reply.csv.as_deref(), Some("A-YDSW8NLFLANWPM"));
        assert_eq!(reply.state.as_deref(), Some("ParcialmenteCorrecto"));
        assert_eq!(reply.received_at.as_deref(), Some("2026-10-17T10:00:05+02:00"));
        let batch = [
            pending(1, "alta", "T2026-000001"),
            pending(2, "alta", "T2026-000002"),
            pending(3, "anulacion", "T2026-000001"),
            pending(4, "alta", "T2026-000003"),
            pending(5, "alta", "T2026-000004"),
        ];
        let got = answers(&batch, &reply);
        let states: Vec<(i64, &str, Option<&str>)> = got.iter().map(|a| (a.record_id, a.state.as_str(), a.code.as_deref())).collect();
        assert_eq!(
            states,
            vec![(1, "Correcto", None), (2, "Incorrecto", Some("1124")), (3, "Correcto", Some("3001")), (4, "Correcto", Some("3000"))]
        );
        // a record without a line stays waiting
        assert!(!got.iter().any(|a| a.record_id == 5));
    }

    #[test]
    fn a_refused_message_is_a_fault() {
        let fault = r#"<?xml version="1.0" encoding="UTF-8"?><env:Envelope xmlns:env="http://schemas.xmlsoap.org/soap/envelope/"><env:Body><env:Fault>
<faultcode>env:Client</faultcode><faultstring>Codigo[4104].El NIF del titular en la cabecera no está identificado.
NIF:iii. NOMBRE_RAZON:xxx</faultstring><detail><callstack>WSExcepcion</callstack></detail></env:Fault></env:Body></env:Envelope>"#;
        let r = parse(fault).unwrap();
        let (code, msg) = r.fault.unwrap();
        assert_eq!(code, "env:Client");
        assert!(msg.starts_with("Codigo[4104]"));
        assert!(parse("<html>Service Unavailable</html>").is_err());
    }

    #[test]
    fn the_envelope_carries_the_records_as_stored() {
        let batch = [pending(1, "alta", "T2026-000001")];
        let e = envelope("Pérez & Hijos", "89890001K", &batch, true);
        assert!(e.contains("<sum1:NombreRazon>Pérez &amp; Hijos</sum1:NombreRazon><sum1:NIF>89890001K</sum1:NIF>"));
        assert!(e.contains("<sum1:RemisionVoluntaria><sum1:Incidencia>S</sum1:Incidencia></sum1:RemisionVoluntaria></sum:Cabecera>"));
        assert!(e.contains(&format!("<sum:RegistroFactura>{}</sum:RegistroFactura>", batch[0].xml)));
        assert!(!envelope("x", "89890001K", &batch, false).contains("Incidencia"));
        roxmltree::Document::parse(&e).unwrap();
    }

    #[test]
    fn http_replies_plain_and_chunked() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: 5\r\n\r\n<a/>\n";
        assert_eq!(http_reply(plain).unwrap(), (200, "<a/>\n".to_string()));
        let chunked = b"HTTP/1.1 500 Internal Server Error\r\nTransfer-Encoding: chunked\r\n\r\n4\r\n<a>x\r\n5\r\ny</a>\r\n0\r\n\r\n";
        assert_eq!(http_reply(chunked).unwrap(), (500, "<a>xy</a>".to_string()));
        assert!(http_reply(b"HTTP/1.1 200").is_err());
    }

    // a throwaway self-signed certificate shaped like an FNMT one (its key
    // was discarded): UTF-8 names, client authentication only
    const CERT: &str = "MIICTjCCAbegAwIBAgIBBzANBgkqhkiG9w0BAQsFADBRMQswCQYDVQQGEwJFUzEYMBYGA1UEBRMPSURDRVMtMTIzNDU2NzhaMSgwJgYDVQQDDB9Qw4lSRVogR0FSQ8ONQSBKVUFOIC0gMTIzNDU2NzhaMB4XDTI2MTAxNzIwMDEwNFoXDTI2MTAxODIwMDEwNFowUTELMAkGA1UEBhMCRVMxGDAWBgNVBAUTD0lEQ0VTLTEyMzQ1Njc4WjEoMCYGA1UEAwwfUMOJUkVaIEdBUkPDjUEgSlVBTiAtIDEyMzQ1Njc4WjCBnzANBgkqhkiG9w0BAQEFAAOBjQAwgYkCgYEAsjZCLUMXstjlxsua/RxUHXULEWBdI7AQqeSN1nNZqkhcMOPpYY2c+aycp46NULJWNBoYxwVS5lrP5FLGHUym+de1PV0cYssijXY3X20wweHFltrHsLL7aEP6m1iLbL5TCECJe2cprG8DgVhv2L68KhoN7wk/MfINAP/EbMarNoECAwEAAaM2MDQwEwYDVR0lBAwwCgYIKwYBBQUHAwIwHQYDVR0OBBYEFF8NMsMatUsGxEjdMgT5v59u36PDMA0GCSqGSIb3DQEBCwUAA4GBAEy9Jri4Sk2fHAxLUDXLZKUz2XFENbtLXa8N1bRefFzlBoODmlPjWlr/v4U1k+vYixKT6Ib5i8NymSZB8XPZMKQTgki6frHLH4m36lyYoGJdzz1QCiW3U/zOpL8hOMtI7FSnnslykHSquvU7GKcywHD/SuWFVeXCfK47jG8y/j6i";

    #[test]
    fn a_certificate_says_whom_it_is_for_and_until_when() {
        use base64::Engine;
        let der = base64::engine::general_purpose::STANDARD.decode(CERT).unwrap();
        let (name, issuer, until) = describe(&der).unwrap();
        assert_eq!(name, "PÉREZ GARCÍA JUAN - 12345678Z");
        assert_eq!(issuer, name);
        assert_eq!(until, "2026-10-18");
        assert!(describe(&der[..40]).is_none());
        assert_eq!(der_date(0x18, b"20310509120000Z"), "2031-05-09");
        assert_eq!(der_text(0x1E, &[0, b'A', 0, 0xD1]), "AÑ");
    }

    #[test]
    fn waits_grow_but_stay_under_the_hourly_retry() {
        assert_eq!(backoff(1), Duration::from_secs(60));
        assert_eq!(backoff(2), Duration::from_secs(120));
        assert_eq!(backoff(4), Duration::from_secs(480));
        assert_eq!(backoff(50), Duration::from_secs(480));
        assert!(older_than("2020-01-01T00:00:00+01:00", 300));
        assert!(!older_than(&verifactu::stamp(&Local::now()), 300));
    }
}
