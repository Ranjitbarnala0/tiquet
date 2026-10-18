use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{fail, Error, Result};
use crate::settings::Settings;
use crate::state::PrintSink;

// A job the queue has not started printing in this long is cancelled: the
// printer is off, unplugged or waiting for someone. Cancelling means nothing
// comes out hours later on its own, drawer kick included.
const STALL: Duration = Duration::from_secs(10);
// a job still printing after this is left to finish and reported as slow
const SLOW: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_millis(150);
// the spooler's status lags a little when a job starts
const GRACE: Duration = Duration::from_millis(1500);
// a network printer: getting a connection, then taking the whole job
const CONNECT: Duration = Duration::from_secs(3);
const WRITE: Duration = Duration::from_secs(10);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub port: String,
    pub driver: String,
    #[serde(rename = "virtual")]
    pub is_virtual: bool,
    pub default: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    // none | missing | virtual | ready | offline | paper | paused | error | unknown
    pub state: &'static str,
    pub jobs: u32,
}

impl Status {
    fn of(state: &'static str) -> Status {
        Status { state, jobs: 0 }
    }
}

// Where tickets go: a queue installed in Windows, or a printer on the network
// that takes raw ESC/POS on a TCP port (9100 on nearly all of them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Queue(String),
    Net(String, u16),
}

// None: no printer set up yet
pub fn target(s: &Settings) -> Option<Target> {
    let (name, host) = (s.printer_name.trim(), s.printer_host.trim());
    match s.printer_kind.as_str() {
        "network" if host.is_empty() => None,
        "network" => Some(Target::Net(host.into(), s.printer_port)),
        _ if name.is_empty() => None,
        _ => Some(Target::Queue(name.into())),
    }
}

// what the screen and the test page call it
pub fn label(s: &Settings) -> String {
    match target(s) {
        Some(Target::Queue(name)) => name,
        Some(Target::Net(host, port)) => format!("{host}:{port}"),
        None => String::new(),
    }
}

// PDF and XPS writers, OneNote and fax queues accept a job and then ask
// where to save it or render garbage: they can never print an ESC/POS ticket
pub fn is_virtual(name: &str, port: &str, driver: &str) -> bool {
    let all = format!("{name} {port} {driver}").to_lowercase();
    ["portprompt:", "file:", "nul:", "xpsport", "pdf", "xps", "onenote", "fax"]
        .iter()
        .any(|k| all.contains(k))
}

const ATTR_WORK_OFFLINE: u32 = 0x400;
const ST_PAUSED: u32 = 0x1;
const ST_ERROR: u32 = 0x2;
const ST_PAPER_JAM: u32 = 0x8;
const ST_PAPER_OUT: u32 = 0x10;
const ST_PAPER_PROBLEM: u32 = 0x40;
const ST_OFFLINE: u32 = 0x80;
const ST_NOT_AVAILABLE: u32 = 0x1000;
const ST_USER_INTERVENTION: u32 = 0x10_0000;
const ST_DOOR_OPEN: u32 = 0x40_0000;
const ST_SERVER_OFFLINE: u32 = 0x200_0000;

fn classify(attributes: u32, status: u32) -> &'static str {
    if attributes & ATTR_WORK_OFFLINE != 0 || status & (ST_OFFLINE | ST_SERVER_OFFLINE | ST_NOT_AVAILABLE) != 0 {
        "offline"
    } else if status & (ST_PAPER_OUT | ST_PAPER_JAM | ST_PAPER_PROBLEM) != 0 {
        "paper"
    } else if status & ST_PAUSED != 0 {
        "paused"
    } else if status & (ST_ERROR | ST_DOOR_OPEN | ST_USER_INTERVENTION) != 0 {
        "error"
    } else {
        "ready"
    }
}

const JOB_PAUSED: u32 = 0x1;
const JOB_ERROR: u32 = 0x2;
const JOB_DELETING: u32 = 0x4;
const JOB_PRINTING: u32 = 0x10;
const JOB_OFFLINE: u32 = 0x20;
const JOB_PAPEROUT: u32 = 0x40;
const JOB_PRINTED: u32 = 0x80;
const JOB_DELETED: u32 = 0x100;
const JOB_BLOCKED: u32 = 0x200;
const JOB_INTERVENTION: u32 = 0x400;
const JOB_COMPLETE: u32 = 0x1000;

fn job_problem(st: u32) -> Option<&'static str> {
    if st & (JOB_DELETING | JOB_DELETED) != 0 {
        Some("cancelled")
    } else if st & JOB_OFFLINE != 0 {
        Some("offline")
    } else if st & JOB_PAPEROUT != 0 {
        Some("paper")
    } else if st & JOB_PAUSED != 0 {
        Some("paused")
    } else if st & (JOB_ERROR | JOB_BLOCKED | JOB_INTERVENTION) != 0 {
        Some("error")
    } else {
        None
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Watch {
    // the job left the queue or says printed
    Done,
    // still printing when we stopped waiting
    Slow,
    // stopped before any of it printed: cancelled, nothing will come out
    Failed(&'static str),
    // stopped after it had started printing: part of it may be on paper
    Partial(&'static str),
}

// Follows one job in the Windows queue. `look` gives the job's status bits,
// or None once it has left the queue; `printer` the queue's own state.
fn watch(mut look: impl FnMut() -> Option<u32>, mut printer: impl FnMut() -> &'static str, stall: Duration, slow: Duration, tick: Duration) -> Watch {
    let start = Instant::now();
    let mut printing = false;
    loop {
        let Some(st) = look() else { return Watch::Done };
        if st & (JOB_PRINTED | JOB_COMPLETE) != 0 {
            return Watch::Done;
        }
        printing |= st & JOB_PRINTING != 0;
        let queue = if start.elapsed() >= GRACE.min(stall) { printer() } else { "ready" };
        let problem = job_problem(st).or(match queue {
            "offline" | "paper" | "paused" | "error" => Some(queue),
            _ => None,
        });
        let waited = start.elapsed();
        match problem {
            Some(kind) if printing => return Watch::Partial(kind),
            Some(kind) => return Watch::Failed(kind),
            None if !printing && waited >= stall => return Watch::Failed("stalled"),
            None if printing && waited >= slow => return Watch::Slow,
            None => std::thread::sleep(tick),
        }
    }
}

// One thread per kind of spooler work, for the life of the app. A spooler
// that hangs holds that one thread; later requests time out instead of
// piling up more threads, and a print that could not start in time is
// dropped rather than printed after the cashier was told it failed.
type Task = (Instant, Box<dyn FnOnce() + Send>);

struct Lane(Mutex<mpsc::Sender<Task>>);

impl Lane {
    fn new() -> Lane {
        let (tx, rx) = mpsc::channel::<Task>();
        std::thread::spawn(move || {
            for (deadline, task) in rx {
                if Instant::now() <= deadline {
                    task();
                }
            }
        });
        Lane(Mutex::new(tx))
    }

    // Some(answer), or None with whether the work had started
    fn run<T: Send + 'static>(&self, start: Duration, total: Duration, f: impl FnOnce() -> T + Send + 'static) -> std::result::Result<T, bool> {
        let (tx, rx) = mpsc::channel();
        let began = Arc::new(AtomicBool::new(false));
        let flag = began.clone();
        let task = Box::new(move || {
            flag.store(true, Ordering::SeqCst);
            let _ = tx.send(f());
        });
        let sent = self.0.lock().unwrap_or_else(|e| e.into_inner()).send((Instant::now() + start, task));
        if sent.is_err() {
            return Err(false);
        }
        rx.recv_timeout(total).map_err(|_| began.load(Ordering::SeqCst))
    }
}

fn print_lane() -> &'static Lane {
    static L: OnceLock<Lane> = OnceLock::new();
    L.get_or_init(Lane::new)
}

fn status_lane() -> &'static Lane {
    static L: OnceLock<Lane> = OnceLock::new();
    L.get_or_init(Lane::new)
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub best: Option<String>,
    pub columns: u32,
    pub candidates: Vec<String>,
}

// Which installed printer is the receipt printer, for a new PC or the
// Detectar button. PDF, XPS and fax queues never qualify; a name, driver or
// port that says POS, receipt, thermal, a receipt brand, 58/80 mm or USB
// counts for it, an office printer brand against it. Only a clear winner is
// picked; a tie is left for the person to choose.
pub fn detect(entries: &[Entry]) -> Detected {
    const RECEIPT: &[&str] = &[
        "pos", "receipt", "ticket", "tickets", "thermal", "termica", "tm", "escpos", "xprinter", "premier", "itp", "bixolon",
        "citizen", "sewoo", "rongta", "hprt", "zjiang", "gprinter", "posbank", "custom", "metapace", "sam4s", "star",
    ];
    const OFFICE: &[&str] = &[
        "laserjet", "deskjet", "officejet", "envy", "pixma", "ecotank", "workforce", "brother", "canon", "samsung", "lexmark",
        "ricoh", "kyocera", "xerox", "sharp", "konica", "oki", "mfp", "pcl", "postscript",
    ];
    let mut scored: Vec<(i32, &Entry, bool)> = Vec::new();
    for e in entries.iter().filter(|e| !e.is_virtual) {
        let all = format!("{} {} {}", e.name, e.driver, e.port).to_lowercase();
        // name and driver often repeat each other: each word counts once
        let mut words: Vec<&str> = all.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect();
        words.sort_unstable();
        words.dedup();
        let mut score = 0;
        for w in &words {
            let digits = |p: &str| w.strip_prefix(p).is_some_and(|r| !r.is_empty() && r.chars().all(|c| c.is_ascii_digit()));
            if RECEIPT.contains(w) || digits("pos") || digits("itp") || digits("tm") || matches!(*w, "58mm" | "80mm" | "58" | "80") {
                score += 10;
            }
            if OFFICE.contains(w) {
                score -= 10;
            }
        }
        if all.contains("generic / text only") {
            score += 5;
        }
        if e.port.to_lowercase().starts_with("usb") {
            score += 3;
        }
        let narrow = words.iter().any(|w| w.contains("58"));
        if score > 0 {
            scored.push((score, e, narrow));
        }
    }
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let clear = match scored.as_slice() {
        [(top, ..), (next, ..), ..] => top > next,
        [_] => true,
        [] => false,
    };
    Detected {
        best: clear.then(|| scored[0].1.name.clone()),
        columns: if clear && scored[0].2 { 32 } else { 48 },
        candidates: scored.iter().map(|(_, e, _)| e.name.clone()).collect(),
    }
}

pub fn list() -> Vec<Entry> {
    let (rows, default) = win::list();
    let mut out: Vec<Entry> = rows
        .into_iter()
        .map(|(name, port, driver)| Entry {
            is_virtual: is_virtual(&name, &port, &driver),
            default: default.as_deref() == Some(name.as_str()),
            name,
            port,
            driver,
        })
        .collect();
    out.sort_by(|a, b| a.is_virtual.cmp(&b.is_virtual).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

// What the printer says. An idle queue's state can be stale (Windows
// refreshes it while a job prints), so it informs the cashier but never
// blocks a print. A network printer is ready when it takes a connection.
pub fn status(sink: &PrintSink, s: &Settings) -> Status {
    let Some(to) = target(s) else {
        return Status::of("none");
    };
    match (sink, to) {
        (PrintSink::Dir(_), _) => Status::of("ready"),
        (PrintSink::Spooler, Target::Queue(name)) => {
            let n = name.clone();
            match status_lane().run(Duration::from_secs(2), Duration::from_secs(3), move || win::info(&n)) {
                Err(_) => Status::of("unknown"),
                Ok(Err(_)) => Status::of("missing"),
                Ok(Ok(i)) if is_virtual(&name, &i.port, &i.driver) => Status { state: "virtual", jobs: i.jobs },
                Ok(Ok(i)) => Status { state: classify(i.attributes, i.status), jobs: i.jobs },
            }
        }
        // many network printers take one connection at a time: a probe made
        // while a ticket prints would find it offline, so none is made then,
        // and a probe waits its turn behind prints
        (PrintSink::Spooler, Target::Net(_, _)) if NET_BUSY.load(Ordering::SeqCst) => Status::of("ready"),
        (PrintSink::Spooler, Target::Net(host, port)) => {
            match print_lane().run(Duration::from_secs(2), CONNECT + Duration::from_secs(1), move || connect(&host, port, CONNECT).is_ok()) {
                Ok(true) => Status::of("ready"),
                Ok(false) => Status::of("offline"),
                Err(_) => Status::of("unknown"),
            }
        }
    }
}

// Ok(None): printed. Ok(Some("W_PRINT_SLOW")): still printing when we
// stopped waiting. E_PRINT:<why>: nothing printed and nothing is left in the
// queue. E_PRINT:partial-<why> and E_PRINT:timeout: part or all of it may be
// on paper, so the next print of that ticket must be marked as a copy.
pub fn send(sink: &PrintSink, s: &Settings, doc: &str, data: &[u8]) -> Result<Option<String>> {
    let Some(to) = target(s) else {
        return fail("E_NO_PRINTER");
    };
    if let Target::Queue(name) = &to {
        match status(sink, s).state {
            "missing" => return fail(format!("E_PRINTER_MISSING:{name}")),
            "virtual" => return fail(format!("E_PRINTER_VIRTUAL:{name}")),
            _ => {}
        }
    }
    match (sink, to) {
        (PrintSink::Dir(dir), _) => {
            std::fs::create_dir_all(dir)?;
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
            let name: String = doc
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            std::fs::write(dir.join(format!("{stamp}-{name}.bin")), data)?;
            Ok(None)
        }
        (PrintSink::Spooler, Target::Queue(p)) => {
            let (d, b) = (doc.to_string(), data.to_vec());
            on_lane(STALL + SLOW + Duration::from_secs(5), move || spool(&p, &d, &b, None))
        }
        (PrintSink::Spooler, Target::Net(host, port)) => {
            let b = data.to_vec();
            on_lane(CONNECT + WRITE + Duration::from_secs(5), move || net(&host, port, &b, CONNECT, WRITE).map(|_| None))
        }
    }
}

// a print that could not start in time never went out; one that started and
// did not answer in time may be on paper
fn on_lane(total: Duration, f: impl FnOnce() -> std::result::Result<Option<String>, String> + Send + 'static) -> Result<Option<String>> {
    match print_lane().run(Duration::from_secs(4), total, f) {
        Ok(r) => r.map_err(Error),
        Err(true) => fail("E_PRINT:timeout"),
        Err(false) => fail("E_PRINT:busy"),
    }
}

// For a drawer kick after a sale saved without printing: the sale does not
// wait for it, and if the printer is off the job is cancelled, never left to
// open the drawer later on its own.
pub fn send_detached(sink: &PrintSink, s: &Settings, doc: &str, data: &[u8]) {
    let Some(to) = target(s) else { return };
    match sink {
        PrintSink::Dir(_) => {
            let _ = send(sink, s, doc, data);
        }
        PrintSink::Spooler => {
            let (name, d, b) = (label(s), doc.to_string(), data.to_vec());
            let task = Box::new(move || {
                let out = match &to {
                    Target::Queue(p) => spool(p, &d, &b, None).map(|_| ()),
                    Target::Net(host, port) => net(host, *port, &b, CONNECT, WRITE),
                };
                if let Err(e) = out {
                    crate::log::warn(format!("{d} on {name}: {e}"));
                }
            });
            let _ = print_lane().0.lock().unwrap_or_else(|e| e.into_inner()).send((Instant::now() + Duration::from_secs(4), task));
        }
    }
}

static NET_BUSY: AtomicBool = AtomicBool::new(false);

fn connect(host: &str, port: u16, wait: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::from(io::ErrorKind::NotFound);
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, wait) {
            Ok(sock) => return Ok(sock),
            Err(e) => last = e,
        }
    }
    Err(last)
}

// A network print is one connection: connect, write it all, close. Without a
// connection nothing went out; once it is open, a failure may have left part
// of the ticket on paper.
fn net(host: &str, port: u16, data: &[u8], wait: Duration, limit: Duration) -> std::result::Result<(), String> {
    NET_BUSY.store(true, Ordering::SeqCst);
    let out = match connect(host, port, wait) {
        Err(_) => Err("E_PRINT:offline".to_string()),
        Ok(mut sock) => put(&mut sock, data, limit).map_err(|_| "E_PRINT:partial-error".to_string()),
    };
    NET_BUSY.store(false, Ordering::SeqCst);
    out
}

// The whole job within `limit`, however slowly the printer takes it. Written
// a piece at a time, so a printer that hangs up halfway is noticed: Windows
// would otherwise take the whole job into its own buffers at once.
fn put(sock: &mut TcpStream, mut data: &[u8], limit: Duration) -> io::Result<()> {
    let end = Instant::now() + limit;
    while !data.is_empty() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        sock.set_write_timeout(Some(left))?;
        match sock.write(&data[..data.len().min(4096)]) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => data = &data[n..],
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn spool(printer: &str, doc: &str, data: &[u8], out_file: Option<&Path>) -> std::result::Result<Option<String>, String> {
    let job = win::submit(printer, doc, data, out_file).map_err(|e| format!("E_PRINT:{e}"))?;
    let q = win::Queue::open(printer).map_err(|e| format!("E_PRINT:{e}"))?;
    match watch(|| q.job(job), || q.state(), STALL, SLOW, TICK) {
        Watch::Done => Ok(None),
        Watch::Slow => Ok(Some("W_PRINT_SLOW".into())),
        Watch::Failed(why) => {
            q.cancel(job);
            Err(format!("E_PRINT:{why}"))
        }
        Watch::Partial(why) => {
            q.cancel(job);
            Err(format!("E_PRINT:partial-{why}"))
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Graphics::Printing::{
        AbortPrinter, ClosePrinter, EndDocPrinter, EndPagePrinter, EnumPrintersW, GetDefaultPrinterW, GetJobW,
        GetPrinterDriverW, GetPrinterW, OpenPrinterW, SetJobW, StartDocPrinterW, StartPagePrinter, WritePrinter,
        DOC_INFO_1W, DRIVER_INFO_2W, JOB_CONTROL_DELETE, JOB_INFO_1W, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL,
        PRINTER_INFO_2W,
    };

    pub struct Info {
        pub port: String,
        pub driver: String,
        pub attributes: u32,
        pub status: u32,
        pub jobs: u32,
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }

    fn last_error() -> String {
        std::io::Error::last_os_error().to_string()
    }

    pub fn list() -> (Vec<(String, String, String)>, Option<String>) {
        let mut rows = Vec::new();
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        unsafe {
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, null(), 2, null_mut(), 0, &mut needed, &mut count);
            if needed > 0 {
                // u64 backing keeps the PRINTER_INFO_2W array pointer-aligned
                let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
                if EnumPrintersW(flags, null(), 2, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) != 0 {
                    let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_2W, count as usize);
                    for i in infos {
                        rows.push((from_wide(i.pPrinterName), from_wide(i.pPortName), from_wide(i.pDriverName)));
                    }
                }
            }
        }
        (rows, default_printer())
    }

    fn default_printer() -> Option<String> {
        unsafe {
            let mut len = 0u32;
            GetDefaultPrinterW(null_mut(), &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize];
            if GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) == 0 {
                return None;
            }
            Some(from_wide(buf.as_ptr()))
        }
    }

    // an open queue handle, closed on drop
    pub struct Queue(HANDLE);

    impl Queue {
        pub fn open(name: &str) -> Result<Queue, String> {
            let w = wide(name);
            let mut h: HANDLE = null_mut();
            unsafe {
                if OpenPrinterW(w.as_ptr(), &mut h, null()) == 0 {
                    return Err(last_error());
                }
            }
            Ok(Queue(h))
        }

        pub fn info(&self) -> Result<Info, String> {
            unsafe {
                let mut needed = 0u32;
                GetPrinterW(self.0, 2, null_mut(), 0, &mut needed);
                if needed == 0 {
                    return Err(last_error());
                }
                let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
                if GetPrinterW(self.0, 2, buf.as_mut_ptr() as *mut u8, needed, &mut needed) == 0 {
                    return Err(last_error());
                }
                let p = &*(buf.as_ptr() as *const PRINTER_INFO_2W);
                Ok(Info {
                    port: from_wide(p.pPortName),
                    driver: from_wide(p.pDriverName),
                    attributes: p.Attributes,
                    status: p.Status,
                    jobs: p.cJobs,
                })
            }
        }

        pub fn state(&self) -> &'static str {
            self.info().map(|i| super::classify(i.attributes, i.status)).unwrap_or("error")
        }

        // the job's status bits, or None once the queue no longer has it
        pub fn job(&self, id: u32) -> Option<u32> {
            unsafe {
                let mut needed = 0u32;
                GetJobW(self.0, id, 1, null_mut(), 0, &mut needed);
                if needed == 0 {
                    return None;
                }
                let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
                if GetJobW(self.0, id, 1, buf.as_mut_ptr() as *mut u8, needed, &mut needed) == 0 {
                    return None;
                }
                Some((*(buf.as_ptr() as *const JOB_INFO_1W)).Status)
            }
        }

        pub fn cancel(&self, id: u32) {
            unsafe {
                SetJobW(self.0, id, 0, null(), JOB_CONTROL_DELETE);
            }
        }
    }

    impl Drop for Queue {
        fn drop(&mut self) {
            unsafe {
                ClosePrinter(self.0);
            }
        }
    }

    pub fn info(name: &str) -> Result<Info, String> {
        Queue::open(name)?.info()
    }

    unsafe fn driver_version(h: HANDLE) -> u32 {
        let mut needed = 0u32;
        GetPrinterDriverW(h, null(), 2, null_mut(), 0, &mut needed);
        if needed == 0 {
            return 3;
        }
        let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
        if GetPrinterDriverW(h, null(), 2, buf.as_mut_ptr() as *mut u8, needed, &mut needed) == 0 {
            return 3;
        }
        (*(buf.as_ptr() as *const DRIVER_INFO_2W)).cVersion
    }

    // RAW hands the bytes straight to the port on classic (v3) drivers. A v4
    // (XPS) driver accepts a RAW job and then silently drops it, so those get
    // XPS_PASS, their pass-through datatype. Returns the job id.
    pub fn submit(printer: &str, doc: &str, data: &[u8], out_file: Option<&Path>) -> Result<u32, String> {
        let q = Queue::open(printer)?;
        let doc_w = wide(doc);
        let out_w = out_file.map(|p| wide(&p.to_string_lossy()));
        unsafe {
            let mut first_err = None;
            let order = if driver_version(q.0) >= 4 { ["XPS_PASS", "RAW"] } else { ["RAW", "XPS_PASS"] };
            for dtype in order {
                let dtype_w = wide(dtype);
                let info = DOC_INFO_1W {
                    pDocName: doc_w.as_ptr() as *mut u16,
                    pOutputFile: out_w.as_ref().map_or(null_mut(), |o| o.as_ptr() as *mut u16),
                    pDatatype: dtype_w.as_ptr() as *mut u16,
                };
                let job = StartDocPrinterW(q.0, 1, &info);
                if job == 0 {
                    first_err.get_or_insert_with(last_error);
                    continue;
                }
                // a half-written job must never reach the paper: abort it
                // instead of ending it
                return match write_job(q.0, data) {
                    Ok(()) => {
                        EndDocPrinter(q.0);
                        Ok(job)
                    }
                    Err(e) => {
                        AbortPrinter(q.0);
                        Err(e)
                    }
                };
            }
            Err(first_err.unwrap_or_default())
        }
    }

    unsafe fn write_job(h: HANDLE, data: &[u8]) -> Result<(), String> {
        if StartPagePrinter(h) == 0 {
            return Err(last_error());
        }
        let mut written = 0u32;
        let ok = WritePrinter(h, data.as_ptr() as *const c_void, data.len() as u32, &mut written);
        let err = (ok == 0).then(last_error);
        EndPagePrinter(h);
        match err {
            Some(e) => Err(e),
            None if written as usize != data.len() => Err(format!("{written}/{} bytes", data.len())),
            None => Ok(()),
        }
    }
}

// the network tests share the print lane, and a refused connection takes
// Windows two seconds: one at a time keeps them from timing each other out
#[cfg(test)]
pub static NET_TESTS: Mutex<()> = Mutex::new(());

#[cfg(not(windows))]
mod win {
    use std::path::Path;

    pub struct Info {
        pub port: String,
        pub driver: String,
        pub attributes: u32,
        pub status: u32,
        pub jobs: u32,
    }

    pub struct Queue;

    impl Queue {
        pub fn open(_: &str) -> Result<Queue, String> {
            Err("printing needs Windows".into())
        }
        pub fn state(&self) -> &'static str {
            "error"
        }
        pub fn job(&self, _: u32) -> Option<u32> {
            None
        }
        pub fn cancel(&self, _: u32) {}
    }

    pub fn list() -> (Vec<(String, String, String)>, Option<String>) {
        (Vec::new(), None)
    }

    pub fn info(_: &str) -> Result<Info, String> {
        Err("printing needs Windows".into())
    }

    pub fn submit(_: &str, _: &str, _: &[u8], _: Option<&Path>) -> Result<u32, String> {
        Err("printing needs Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    #[test]
    fn virtual_queues_are_recognised() {
        assert!(is_virtual("Microsoft Print to PDF", "PORTPROMPT:", "Microsoft Print To PDF"));
        assert!(is_virtual("Microsoft XPS Document Writer", "PORTPROMPT:", "Microsoft XPS Document Writer v4"));
        assert!(is_virtual("OneNote (Desktop)", "nul:", "Send to Microsoft OneNote 16 Driver"));
        assert!(is_virtual("Fax", "SHRFAX:", "Microsoft Shared Fax Driver"));
        assert!(!is_virtual("POS-80", "USB001", "POS-80C"));
        assert!(!is_virtual("Premier ITP-73", "USB002", "Generic / Text Only"));
    }

    #[test]
    fn the_receipt_printer_is_found_among_the_others() {
        let e = |name: &str, port: &str, driver: &str| Entry {
            name: name.into(),
            port: port.into(),
            driver: driver.into(),
            is_virtual: is_virtual(name, port, driver),
            default: false,
        };
        let office = e("HP LaserJet Pro M404", "WSD-1234", "HP Universal Printing PCL 6");
        let pdf = e("Microsoft Print to PDF", "PORTPROMPT:", "Microsoft Print To PDF");
        let pos = e("POS-80C", "USB001", "POS-80C");
        let found = detect(&[office.clone(), pdf.clone(), pos]);
        assert_eq!((found.best.as_deref(), found.columns), (Some("POS-80C"), 48));
        let small = e("Impresora tickets 58mm", "USB002", "Generic / Text Only");
        assert_eq!(detect(&[office.clone(), small]).columns, 32);
        // nothing that looks like a receipt printer: nothing is picked
        assert_eq!(detect(&[office.clone(), pdf]).best, None);
        // two equally likely: the person chooses
        let two = detect(&[e("POS-80", "USB001", "POS-80"), e("XP-80", "USB002", "Xprinter XP-80")]);
        assert_eq!((two.best, two.candidates.len()), (None, 2));
        assert_eq!(detect(&[e("Premier ITP-73", "USB001", "ITP-73")]).best.as_deref(), Some("Premier ITP-73"));
    }

    #[test]
    fn spooler_state_maps_to_what_the_cashier_needs_to_know() {
        assert_eq!(classify(0, 0), "ready");
        assert_eq!(classify(ATTR_WORK_OFFLINE, 0), "offline");
        assert_eq!(classify(0, ST_OFFLINE | ST_PAPER_OUT), "offline");
        assert_eq!(classify(0, ST_PAPER_OUT), "paper");
        assert_eq!(classify(0, ST_PAUSED), "paused");
        assert_eq!(classify(0, ST_DOOR_OPEN), "error");
    }

    // a job is followed through a scripted series of queue states
    fn run(states: &[Option<u32>], queue: &'static str) -> Watch {
        let mut it = states.iter().copied();
        let last = *states.last().unwrap();
        let ms = Duration::from_millis;
        watch(move || it.next().unwrap_or(last), move || queue, ms(60), ms(90), ms(5))
    }

    #[test]
    fn a_job_is_only_printed_when_the_queue_lets_go_of_it() {
        assert_eq!(run(&[Some(0x8), Some(JOB_PRINTING), None], "ready"), Watch::Done);
        assert_eq!(run(&[Some(JOB_PRINTED)], "ready"), Watch::Done);
        // unplugged or switched off: nothing printed, the job is cancelled
        assert_eq!(run(&[Some(0), Some(JOB_OFFLINE)], "ready"), Watch::Failed("offline"));
        assert_eq!(run(&[Some(JOB_PAPEROUT)], "ready"), Watch::Failed("paper"));
        assert_eq!(run(&[Some(0)], "ready"), Watch::Failed("stalled"));
        // the paper ran out halfway: some of it is out there
        assert_eq!(run(&[Some(JOB_PRINTING), Some(JOB_PRINTING | JOB_ERROR)], "ready"), Watch::Partial("error"));
        assert_eq!(run(&[Some(JOB_PRINTING)], "ready"), Watch::Slow);
        assert_eq!(run(&[Some(JOB_DELETING)], "ready"), Watch::Failed("cancelled"));
    }

    #[test]
    fn a_hung_lane_answers_without_running_late_work() {
        let lane = Lane::new();
        let ms = Duration::from_millis;
        let ran = Arc::new(AtomicBool::new(false));
        let busy = Arc::new(AtomicBool::new(false));
        // the lane is stuck on the first job; the second times out before
        // it can start and must never run afterwards
        let b = busy.clone();
        let _ = lane.run(ms(5000), ms(1), move || {
            b.store(true, Ordering::SeqCst);
            std::thread::sleep(ms(300));
        });
        let t = Instant::now();
        while !busy.load(Ordering::SeqCst) && t.elapsed() < ms(2000) {
            std::thread::sleep(ms(1));
        }
        let flag = ran.clone();
        assert_eq!(lane.run(ms(50), ms(80), move || flag.store(true, Ordering::SeqCst)), Err(false));
        std::thread::sleep(ms(400));
        assert!(!ran.load(Ordering::SeqCst));
        assert_eq!(lane.run(ms(50), ms(500), || 7), Ok(7));
    }

    fn queue(name: &str) -> Settings {
        Settings { printer_name: name.into(), ..Settings::default() }
    }

    fn network(port: u16) -> Settings {
        Settings { printer_kind: "network".into(), printer_host: "127.0.0.1".into(), printer_port: port, ..Settings::default() }
    }

    // a printer on the network: what each connection brought, in order. A
    // connection left open fails the read instead of hanging the test.
    fn printer(connections: usize) -> (u16, std::thread::JoinHandle<Vec<Vec<u8>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let got = std::thread::spawn(move || {
            (0..connections)
                .map(|_| {
                    let (mut c, _) = l.accept().unwrap();
                    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut buf = Vec::new();
                    c.read_to_end(&mut buf).unwrap();
                    buf
                })
                .collect()
        });
        (port, got)
    }

    // nobody listens there: bound once for a free number, then let go
    fn closed_port() -> u16 {
        TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }

    #[test]
    fn a_missing_queue_is_refused_before_anything_is_spooled() {
        let err = send(&PrintSink::Spooler, &queue("Tiquet no existe"), "x", b"x").unwrap_err().0;
        assert_eq!(err, "E_PRINTER_MISSING:Tiquet no existe");
        assert_eq!(send(&PrintSink::Spooler, &queue("  "), "x", b"x").unwrap_err().0, "E_NO_PRINTER");
        let no_address = Settings { printer_host: " ".into(), ..network(9100) };
        assert_eq!(send(&PrintSink::Spooler, &no_address, "x", b"x").unwrap_err().0, "E_NO_PRINTER");
        assert_eq!(status(&PrintSink::Spooler, &no_address).state, "none");
    }

    #[test]
    fn virtual_queue_is_refused() {
        let pdf = list().into_iter().find(|p| p.is_virtual);
        if let Some(p) = pdf {
            let err = send(&PrintSink::Spooler, &queue(&p.name), "x", b"x").unwrap_err().0;
            assert_eq!(err, format!("E_PRINTER_VIRTUAL:{}", p.name));
        }
    }

    #[test]
    fn a_network_printer_gets_the_whole_job_on_a_connection_that_is_then_closed() {
        let _one = NET_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let (port, got) = printer(2);
        let s = network(port);
        assert_eq!(status(&PrintSink::Spooler, &s).state, "ready");
        let data: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
        assert_eq!(send(&PrintSink::Spooler, &s, "T2026-000001", &data).unwrap(), None);
        let got = got.join().unwrap();
        // the status probe brings nothing; the print brings every byte
        assert_eq!(got[0], b"");
        assert!(got[1] == data);
        assert_eq!(label(&s), format!("127.0.0.1:{port}"));
    }

    #[test]
    fn a_network_printer_that_does_not_answer_is_offline_and_nothing_was_sent() {
        let _one = NET_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let s = network(closed_port());
        assert_eq!(status(&PrintSink::Spooler, &s).state, "offline");
        assert_eq!(send(&PrintSink::Spooler, &s, "x", b"x").unwrap_err().0, "E_PRINT:offline");
    }

    #[test]
    fn a_job_the_printer_cuts_off_or_cannot_take_in_time_fails() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let mut sock = connect("127.0.0.1", port, Duration::from_secs(1)).unwrap();
        // the printer hangs up before the job is through
        drop(l.accept().unwrap());
        std::thread::sleep(Duration::from_millis(50));
        assert!(put(&mut sock, &vec![0u8; 8 << 20], Duration::from_secs(5)).is_err());
        let mut sock = connect("127.0.0.1", port, Duration::from_secs(1)).unwrap();
        assert_eq!(put(&mut sock, b"x", Duration::ZERO).unwrap_err().kind(), io::ErrorKind::TimedOut);
    }

    // Every Windows install ships "Microsoft Print to PDF". With an output
    // file set the spooler writes the job there instead of prompting, so the
    // whole path runs without paper: submit, follow the job until the queue
    // lets go of it, and the bytes arrive untouched.
    #[test]
    #[ignore = "talks to the Windows spooler"]
    fn spooler_passes_bytes_through_untouched() {
        let out = std::env::temp_dir().join(format!("tiquet-spool-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&out);
        let data: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        assert_eq!(spool("Microsoft Print to PDF", "tiquet-test", &data, Some(&out)), Ok(None));
        assert_eq!(std::fs::read(&out).unwrap(), data);
        let _ = std::fs::remove_file(&out);
    }
}
