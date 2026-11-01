use std::io::{self, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::mpsc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{fail, Error, Result};
use crate::escpos::{self, Opts};
use crate::layout::Line;
use crate::settings::{Logo, Settings};
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
// a page drawn through a printer's own driver: spooled, not watched
const DRAW: Duration = Duration::from_secs(30);

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub port: String,
    pub driver: String,
    #[serde(rename = "virtual")]
    pub is_virtual: bool,
    pub default: bool,
    // looks like a receipt printer: raw ESC/POS suits it; anything else
    // prints tickets as a document through its driver
    pub receipt: bool,
}

// A serial port a printer can hang on: Bluetooth printers paired in Windows
// and USB-serial ones show up as COM ports.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Port {
    pub name: String,
    // bluetooth | usb | serial
    pub kind: &'static str,
}

// What a print is: the ESC/POS bytes for a receipt printer, and the same
// documents as lines for a printer that prints pages through its driver.
#[derive(Default, Clone)]
pub struct Job {
    pub raw: Vec<u8>,
    pub pages: Vec<Page>,
}

#[derive(Clone)]
pub struct Page {
    pub lines: Vec<Line>,
    pub logo: Option<Logo>,
    pub cols: usize,
}

impl Job {
    pub fn raw(bytes: Vec<u8>) -> Job {
        Job { raw: bytes, pages: Vec::new() }
    }

    pub fn doc(lines: &[Line], logo: Option<&Logo>, o: &Opts) -> Job {
        let mut j = Job::default();
        j.push(lines, logo, o);
        j
    }

    pub fn push(&mut self, lines: &[Line], logo: Option<&Logo>, o: &Opts) {
        self.raw.extend(escpos::document(lines, logo, o));
        self.pages.push(Page { lines: lines.to_vec(), logo: logo.cloned(), cols: o.cols });
    }

    pub fn append(&mut self, other: Job) {
        self.raw.extend(other.raw);
        self.pages.extend(other.pages);
    }
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

// Where tickets go: a queue installed in Windows that takes raw ESC/POS,
// the same kind of queue printing pages through its own driver (an office
// printer, a label printer, Microsoft Print to PDF), a printer on the network
// that takes raw ESC/POS on a TCP port (9100 on nearly all of them), or a
// COM port (Bluetooth and USB-serial printers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Queue(String),
    Doc(String),
    Net(String, u16),
    Serial(String, u32),
}

// None: no printer set up yet
pub fn target(s: &Settings) -> Option<Target> {
    let (name, host, com) = (s.printer_name.trim(), s.printer_host.trim(), s.printer_com.trim());
    match s.printer_kind.as_str() {
        "network" if host.is_empty() => None,
        "network" => Some(Target::Net(host.into(), s.printer_port)),
        "serial" if com.is_empty() => None,
        "serial" => Some(Target::Serial(com.into(), s.printer_baud)),
        _ if name.is_empty() => None,
        "document" => Some(Target::Doc(name.into())),
        _ => Some(Target::Queue(name.into())),
    }
}

// what the screen and the test page call it
pub fn label(s: &Settings) -> String {
    match target(s) {
        Some(Target::Queue(name) | Target::Doc(name)) => name,
        Some(Target::Net(host, port)) => format!("{host}:{port}"),
        Some(Target::Serial(port, _)) => port,
        None => String::new(),
    }
}

// fax and OneNote queues open windows of their own for every job: they are
// no place for a ticket, even as a document
pub fn shows_ui(name: &str, port: &str, driver: &str) -> bool {
    let all = format!("{name} {port} {driver}").to_lowercase();
    ["onenote", "fax"].iter().any(|k| all.contains(k))
}

// a queue that asks where to save each job (PDF and XPS writers): Tiquet
// gives it a file instead, so no dialog ever opens
fn asks_for_file(port: &str) -> bool {
    let p = port.to_lowercase();
    p.starts_with("portprompt:") || p.starts_with("file:")
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
fn score(e: &Entry) -> (i32, bool) {
    const RECEIPT: &[&str] = &[
        "pos", "receipt", "ticket", "tickets", "thermal", "termica", "tm", "escpos", "xprinter", "premier", "itp", "bixolon",
        "citizen", "sewoo", "rongta", "hprt", "zjiang", "gprinter", "posbank", "custom", "metapace", "sam4s", "star",
    ];
    const OFFICE: &[&str] = &[
        "laserjet", "deskjet", "officejet", "envy", "pixma", "ecotank", "workforce", "brother", "canon", "samsung", "lexmark",
        "ricoh", "kyocera", "xerox", "sharp", "konica", "oki", "mfp", "pcl", "postscript", "series", "inkjet", "laser", "dcp", "mfc",
        "ecosys", "sensys", "imagerunner", "photosmart",
    ];
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
    (score, narrow)
}

pub fn detect(entries: &[Entry]) -> Detected {
    let mut scored: Vec<(i32, &Entry, bool)> = Vec::new();
    for e in entries.iter().filter(|e| !e.is_virtual) {
        let (score, narrow) = score(e);
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
        .map(|(name, port, driver)| {
            let mut e = Entry {
                is_virtual: is_virtual(&name, &port, &driver),
                default: default.as_deref() == Some(name.as_str()),
                receipt: false,
                name,
                port,
                driver,
            };
            e.receipt = !e.is_virtual && score(&e).0 > 0;
            e
        })
        .collect();
    out.sort_by(|a, b| a.is_virtual.cmp(&b.is_virtual).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

// COM ports Windows knows about, Bluetooth ones included, by number
pub fn ports() -> Vec<Port> {
    let mut out: Vec<Port> = win::ports()
        .into_iter()
        .map(|(device, name)| {
            let d = device.to_lowercase();
            let kind = if d.contains("bth") {
                "bluetooth"
            } else if ["usb", "vcp", "silab", "ch34", "prolific", "ftdi", "cdc"].iter().any(|k| d.contains(k)) {
                "usb"
            } else {
                "serial"
            };
            Port { name: name.to_uppercase(), kind }
        })
        .collect();
    let num = |p: &Port| p.name.trim_start_matches("COM").parse::<u32>().unwrap_or(u32::MAX);
    out.sort_by_key(num);
    out.dedup_by(|a, b| a.name == b.name);
    out
}

// What the printer says. An idle queue's state can be stale (Windows
// refreshes it while a job prints), so it informs the cashier but never
// blocks a print. A network printer is ready when it takes a connection; a
// COM port while Windows still has it.
pub fn status(sink: &PrintSink, s: &Settings) -> Status {
    let Some(to) = target(s) else {
        return Status::of("none");
    };
    match (sink, to) {
        (PrintSink::Dir(_), _) => Status::of("ready"),
        (PrintSink::Spooler, Target::Queue(name) | Target::Doc(name)) => {
            let doc = s.printer_kind == "document";
            let n = name.clone();
            match status_lane().run(Duration::from_secs(2), Duration::from_secs(3), move || win::info(&n)) {
                Err(_) => Status::of("unknown"),
                Ok(Err(_)) => Status::of("missing"),
                Ok(Ok(i)) if doc && shows_ui(&name, &i.port, &i.driver) => Status { state: "virtual", jobs: i.jobs },
                Ok(Ok(i)) if !doc && is_virtual(&name, &i.port, &i.driver) => Status { state: "virtual", jobs: i.jobs },
                Ok(Ok(i)) => Status { state: classify(i.attributes, i.status), jobs: i.jobs },
            }
        }
        // Windows lists a Bluetooth port only while it can reach the radio
        (PrintSink::Spooler, Target::Serial(port, _)) => {
            let here = ports().iter().any(|p| p.name.eq_ignore_ascii_case(&port));
            Status::of(if here { "ready" } else { "offline" })
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
    send_job(sink, s, doc, &Job::raw(data.to_vec()))
}

// Ok(Some("W_SAVED:<path>")): a PDF or XPS writer saved the document there.
pub fn send_job(sink: &PrintSink, s: &Settings, doc: &str, job: &Job) -> Result<Option<String>> {
    let Some(to) = target(s) else {
        return fail("E_NO_PRINTER");
    };
    match &to {
        Target::Queue(name) | Target::Doc(name) => match status(sink, s).state {
            "missing" => return fail(format!("E_PRINTER_MISSING:{name}")),
            "virtual" => return fail(format!("E_PRINTER_VIRTUAL:{name}")),
            _ => {}
        },
        _ => {}
    }
    match (sink, to) {
        (PrintSink::Dir(dir), _) => {
            std::fs::create_dir_all(dir)?;
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%3f");
            let name: String = doc
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect();
            std::fs::write(dir.join(format!("{stamp}-{name}.bin")), &job.raw)?;
            Ok(None)
        }
        (PrintSink::Spooler, Target::Queue(p)) => {
            let (d, b) = (doc.to_string(), job.raw.clone());
            on_lane(STALL + SLOW + Duration::from_secs(5), move || spool(&p, &d, &b, None))
        }
        // a drawer kick on its own has nothing to draw: a document printer
        // has no drawer
        (PrintSink::Spooler, Target::Doc(_)) if job.pages.is_empty() => Ok(None),
        (PrintSink::Spooler, Target::Doc(p)) => {
            let (d, pages) = (doc.to_string(), job.pages.clone());
            let out = win::info(&p).ok().filter(|i| asks_for_file(&i.port)).map(|i| saved_as(&p, &i.driver, doc));
            on_lane(DRAW + Duration::from_secs(5), move || {
                if let Some(dir) = out.as_ref().and_then(|f| f.parent()) {
                    std::fs::create_dir_all(dir).map_err(|e| format!("E_PRINT:{e}"))?;
                }
                win::draw(&p, &d, &pages, out.as_deref()).map_err(|e| format!("E_PRINT:{e}"))?;
                Ok(out.map(|f| format!("W_SAVED:{}", f.display())))
            })
        }
        (PrintSink::Spooler, Target::Net(host, port)) => {
            let b = job.raw.clone();
            on_lane(CONNECT + WRITE + Duration::from_secs(5), move || net(&host, port, &b, CONNECT, WRITE).map(|_| None))
        }
        (PrintSink::Spooler, Target::Serial(port, baud)) => {
            let b = job.raw.clone();
            on_lane(WRITE + Duration::from_secs(8), move || win::serial(&port, baud, &b, WRITE).map(|_| None))
        }
    }
}

// where a PDF or XPS writer puts what it is sent: Documents\Tiquet\PDF
fn saved_as(printer: &str, driver: &str, doc: &str) -> std::path::PathBuf {
    let all = format!("{printer} {driver}").to_lowercase();
    let ext = if all.contains("pdf") {
        "pdf"
    } else if all.contains("xps") {
        "oxps"
    } else {
        "prn"
    };
    let name: String = doc.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    crate::backup::documents().join("PDF").join(format!("{name}-{stamp}.{ext}"))
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
        // a document printer has no drawer to open
        PrintSink::Spooler if matches!(to, Target::Doc(_)) => {}
        PrintSink::Spooler => {
            let (name, d, b) = (label(s), doc.to_string(), data.to_vec());
            let task = Box::new(move || {
                let out = match &to {
                    Target::Queue(p) => spool(p, &d, &b, None).map(|_| ()),
                    Target::Doc(_) => Ok(()),
                    Target::Net(host, port) => net(host, *port, &b, CONNECT, WRITE),
                    Target::Serial(port, baud) => win::serial(port, *baud, &b, WRITE),
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

    use crate::layout::{Line, Size};
    use crate::settings::Logo;
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

    // HARDWARE\DEVICEMAP\SERIALCOMM: one value per COM port, named after the
    // device behind it (\Device\BthModem0, \Device\Silabser0)
    pub fn ports() -> Vec<(String, String)> {
        use windows_sys::Win32::System::Registry::{RegCloseKey, RegEnumValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ};

        let mut out = Vec::new();
        let path = wide("HARDWARE\\DEVICEMAP\\SERIALCOMM");
        unsafe {
            let mut key: HKEY = null_mut();
            if RegOpenKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return out;
            }
            for i in 0..256u32 {
                let mut name = [0u16; 256];
                let mut name_len = name.len() as u32;
                let mut data = [0u16; 64];
                let mut data_len = std::mem::size_of_val(&data) as u32;
                let mut kind = 0u32;
                let r = RegEnumValueW(key, i, name.as_mut_ptr(), &mut name_len, null(), &mut kind, data.as_mut_ptr() as *mut u8, &mut data_len);
                if r != 0 {
                    break;
                }
                if kind == REG_SZ {
                    let device = String::from_utf16_lossy(&name[..(name_len as usize).min(name.len())]);
                    let n = (data_len as usize / 2).min(data.len());
                    let com = String::from_utf16_lossy(&data[..n]).trim_end_matches('\0').trim().to_string();
                    if !com.is_empty() {
                        out.push((device, com));
                    }
                }
            }
            RegCloseKey(key);
        }
        out
    }

    // Raw bytes to a COM port. A Bluetooth port reaches the printer when it
    // is opened, which can take a few seconds; a USB-serial one sends at the
    // speed set here (Bluetooth ignores it). Nothing written: offline.
    pub fn serial(port: &str, baud: u32, data: &[u8], limit: std::time::Duration) -> Result<(), String> {
        use windows_sys::Win32::Devices::Communication::{GetCommState, SetCommState, SetCommTimeouts, COMMTIMEOUTS, DCB, NOPARITY, ONESTOPBIT};
        use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, WriteFile, FILE_SHARE_NONE, OPEN_EXISTING};

        let path = wide(&format!("\\\\.\\{port}"));
        unsafe {
            let h = CreateFileW(path.as_ptr(), GENERIC_WRITE, FILE_SHARE_NONE, null(), OPEN_EXISTING, 0, null_mut());
            if h == INVALID_HANDLE_VALUE {
                return Err("E_PRINT:offline".into());
            }
            let mut dcb: DCB = std::mem::zeroed();
            dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
            if GetCommState(h, &mut dcb) != 0 {
                dcb.BaudRate = baud;
                dcb.ByteSize = 8;
                dcb.Parity = NOPARITY;
                dcb.StopBits = ONESTOPBIT;
                SetCommState(h, &dcb);
            }
            let t = COMMTIMEOUTS {
                ReadIntervalTimeout: 0,
                ReadTotalTimeoutMultiplier: 0,
                ReadTotalTimeoutConstant: 0,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: limit.as_millis().min(u32::MAX as u128) as u32,
            };
            SetCommTimeouts(h, &t);
            let mut sent = 0usize;
            let mut out = Ok(());
            while sent < data.len() {
                let chunk = &data[sent..data.len().min(sent + 4096)];
                let mut n = 0u32;
                if WriteFile(h, chunk.as_ptr(), chunk.len() as u32, &mut n, null_mut()) == 0 || n == 0 {
                    out = Err(if sent == 0 { "E_PRINT:offline" } else { "E_PRINT:partial-error" }.to_string());
                    break;
                }
                sent += n as usize;
            }
            CloseHandle(h);
            out
        }
    }

    // A ticket drawn as a page through the printer's own driver, so every
    // printer Windows has can print it: an office or label printer, a
    // receipt printer installed with its Windows driver, a PDF writer (given
    // `out`, it writes that file and asks nothing). It keeps the look of the
    // paper roll: the receipt's columns, 80 mm wide at most, centred.
    pub fn draw(printer: &str, doc: &str, pages: &[super::Page], out: Option<&Path>) -> Result<(), String> {
        use windows_sys::Win32::Graphics::Gdi::{CreateDCW, DeleteDC, GetDeviceCaps, SetBkMode, HORZRES, LOGPIXELSX, TRANSPARENT, VERTRES};
        use windows_sys::Win32::Storage::Xps::{AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW};

        let (driver, device) = (wide("WINSPOOL"), wide(printer));
        let hdc = unsafe { CreateDCW(driver.as_ptr(), device.as_ptr(), null(), null()) };
        if hdc.is_null() {
            return Err("offline".into());
        }
        let name = wide(doc);
        let out_w = out.map(|p| wide(&p.to_string_lossy()));
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: name.as_ptr(),
            lpszOutput: out_w.as_ref().map_or(null(), |w| w.as_ptr()),
            lpszDatatype: null(),
            fwType: 0,
        };
        let result = unsafe {
            if StartDocW(hdc, &info) <= 0 {
                Err("error".to_string())
            } else {
                let dpi = GetDeviceCaps(hdc, LOGPIXELSX as i32).max(96);
                let page_w = GetDeviceCaps(hdc, HORZRES as i32).max(1);
                let page_h = GetDeviceCaps(hdc, VERTRES as i32).max(1);
                let roll = page_w.min(80 * dpi * 10 / 254);
                let mut ok = true;
                for p in pages {
                    let cols = p.cols.clamp(16, 64) as i32;
                    let cw = (roll / cols).max(1);
                    let mut c = Canvas { hdc, x0: (page_w - cw * cols) / 2, width: cw * cols, cw, lh: cw * 2 + cw / 4, y: 0, bottom: page_h, dpi, fonts: Vec::new() };
                    if StartPage(hdc) <= 0 {
                        ok = false;
                        break;
                    }
                    SetBkMode(hdc, TRANSPARENT as i32);
                    for line in &p.lines {
                        let fine = match line {
                            Line::Text { text, bold, size } => c.text(text, *bold, *size),
                            Line::Logo => p.logo.as_ref().map_or(true, |l| c.logo(l)),
                            Line::Qr { rows, module } => c.qr(rows, *module),
                        };
                        if !fine {
                            ok = false;
                            break;
                        }
                    }
                    c.free();
                    if !ok || EndPage(hdc) <= 0 {
                        ok = false;
                        break;
                    }
                }
                if ok && EndDoc(hdc) > 0 {
                    Ok(())
                } else {
                    AbortDoc(hdc);
                    Err("error".to_string())
                }
            }
        };
        unsafe {
            DeleteDC(hdc);
        }
        result
    }

    struct Canvas {
        hdc: windows_sys::Win32::Graphics::Gdi::HDC,
        x0: i32,
        width: i32,
        // one character cell of the receipt, and a line's height
        cw: i32,
        lh: i32,
        y: i32,
        bottom: i32,
        dpi: i32,
        fonts: Vec<((bool, u8), windows_sys::Win32::Graphics::Gdi::HFONT)>,
    }

    impl Canvas {
        // a line that does not fit goes on a new sheet
        fn room(&mut self, h: i32) -> bool {
            use windows_sys::Win32::Graphics::Gdi::{SetBkMode, TRANSPARENT};
            use windows_sys::Win32::Storage::Xps::{EndPage, StartPage};

            if self.y + h <= self.bottom || self.y == 0 {
                return true;
            }
            unsafe {
                if EndPage(self.hdc) <= 0 || StartPage(self.hdc) <= 0 {
                    return false;
                }
                SetBkMode(self.hdc, TRANSPARENT as i32);
            }
            self.y = 0;
            true
        }

        fn font(&mut self, bold: bool, size: Size) -> windows_sys::Win32::Graphics::Gdi::HFONT {
            use windows_sys::Win32::Graphics::Gdi::{CreateFontW, ANTIALIASED_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, FF_MODERN, FIXED_PITCH, FW_BOLD, FW_NORMAL, OUT_TT_PRECIS};

            let key = (bold, size as u8);
            if let Some((_, f)) = self.fonts.iter().find(|(k, _)| *k == key) {
                return *f;
            }
            let (h, w) = match size {
                Size::Normal => (self.cw * 2, self.cw),
                Size::Tall => (self.cw * 4, self.cw),
                Size::Big => (self.cw * 4, self.cw * 2),
            };
            let face = wide("Consolas");
            let weight = if bold { FW_BOLD } else { FW_NORMAL };
            let f = unsafe {
                CreateFontW(
                    h,
                    w,
                    0,
                    0,
                    weight as i32,
                    0,
                    0,
                    0,
                    DEFAULT_CHARSET as u32,
                    OUT_TT_PRECIS as u32,
                    CLIP_DEFAULT_PRECIS as u32,
                    ANTIALIASED_QUALITY as u32,
                    (FIXED_PITCH | FF_MODERN) as u32,
                    face.as_ptr(),
                )
            };
            self.fonts.push((key, f));
            f
        }

        fn free(&mut self) {
            use windows_sys::Win32::Graphics::Gdi::{DeleteObject, GetStockObject, SelectObject, SYSTEM_FONT};

            unsafe {
                SelectObject(self.hdc, GetStockObject(SYSTEM_FONT));
                for (_, f) in self.fonts.drain(..) {
                    DeleteObject(f);
                }
            }
        }

        fn text(&mut self, text: &str, bold: bool, size: Size) -> bool {
            use windows_sys::Win32::Graphics::Gdi::{ExtTextOutW, SelectObject};

            let (advance, dx) = match size {
                Size::Normal => (self.lh, self.cw),
                Size::Tall => (self.lh * 2, self.cw),
                Size::Big => (self.lh * 2, self.cw * 2),
            };
            if !self.room(advance) {
                return false;
            }
            let t = text.trim_end();
            if !t.is_empty() {
                let f = self.font(bold, size);
                let w: Vec<u16> = t.encode_utf16().collect();
                let dxs = vec![dx; w.len()];
                unsafe {
                    SelectObject(self.hdc, f);
                    ExtTextOutW(self.hdc, self.x0, self.y, 0, null(), w.as_ptr(), w.len() as u32, dxs.as_ptr());
                }
            }
            self.y += advance;
            true
        }

        // the 1-bit logo at the size it prints on a 203 dpi roll
        fn logo(&mut self, l: &Logo) -> bool {
            use windows_sys::Win32::Graphics::Gdi::{StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, RGBQUAD, SRCCOPY};

            let Some(bits) = l.raster() else { return true };
            let (w, h) = (l.w as i32, l.h as i32);
            let dw = (w * self.dpi / 203).min(self.width).max(1);
            let dh = (h * dw / w.max(1)).max(1);
            if !self.room(dh) {
                return false;
            }
            let row = (l.w as usize).div_ceil(8);
            let stride = (l.w as usize).div_ceil(32) * 4;
            let mut buf = vec![0u8; stride * l.h as usize];
            for y in 0..l.h as usize {
                buf[y * stride..y * stride + row].copy_from_slice(&bits[y * row..(y + 1) * row]);
            }
            #[repr(C)]
            struct Bmi {
                head: BITMAPINFOHEADER,
                colors: [RGBQUAD; 2],
            }
            let white = RGBQUAD { rgbBlue: 255, rgbGreen: 255, rgbRed: 255, rgbReserved: 0 };
            let black = RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 };
            let bmi = Bmi {
                head: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 1,
                    biCompression: BI_RGB,
                    biSizeImage: buf.len() as u32,
                    biXPelsPerMeter: 0,
                    biYPelsPerMeter: 0,
                    biClrUsed: 2,
                    biClrImportant: 2,
                },
                colors: [white, black],
            };
            unsafe {
                StretchDIBits(
                    self.hdc,
                    self.x0 + (self.width - dw) / 2,
                    self.y,
                    dw,
                    dh,
                    0,
                    0,
                    w,
                    h,
                    buf.as_ptr() as *const c_void,
                    &bmi as *const Bmi as *const BITMAPINFO,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
            self.y += dh;
            true
        }

        fn qr(&mut self, rows: &[String], module: usize) -> bool {
            use windows_sys::Win32::Foundation::RECT;
            use windows_sys::Win32::Graphics::Gdi::{FillRect, GetStockObject, BLACK_BRUSH};

            let m = (module as i32 * self.dpi / 203).max(1);
            let size = rows.len() as i32 * m;
            let pad = self.lh / 2;
            if !self.room(size + 2 * pad) {
                return false;
            }
            let (x, y) = (self.x0 + (self.width - size) / 2, self.y + pad);
            unsafe {
                let brush = GetStockObject(BLACK_BRUSH);
                for (r, row) in rows.iter().enumerate() {
                    for (c, ch) in row.chars().enumerate() {
                        if ch == '1' {
                            let (c, r) = (c as i32, r as i32);
                            let rect = RECT { left: x + c * m, top: y + r * m, right: x + (c + 1) * m, bottom: y + (r + 1) * m };
                            FillRect(self.hdc, &rect, brush);
                        }
                    }
                }
            }
            self.y += size + 2 * pad;
            true
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
    pub fn ports() -> Vec<(String, String)> {
        Vec::new()
    }

    pub fn serial(_: &str, _: u32, _: &[u8], _: std::time::Duration) -> Result<(), String> {
        Err("E_PRINT:offline".into())
    }

    pub fn draw(_: &str, _: &str, _: &[super::Page], _: Option<&Path>) -> Result<(), String> {
        Err("printing needs Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    // a page through a real Windows driver, the PDF writer every Windows has:
    // cargo test --lib -- --ignored
    #[test]
    #[ignore]
    fn a_ticket_prints_as_a_document_on_the_pdf_writer() {
        use crate::layout::Size;
        let Some(writer) = list().into_iter().find(|e| e.name == "Microsoft Print to PDF") else { return };
        let out = std::env::var_os("TIQUET_DOC_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join(format!("tiquet-doc-{}.pdf", std::process::id())));
        let _ = std::fs::remove_file(&out);
        let text = |t: &str, bold: bool, size: Size| Line::Text { text: t.into(), bold, size };
        let lines = vec![
            text("   MI TIENDA", true, Size::Big),
            text("Calle Mayor 1, Figueres", false, Size::Normal),
            text("------------------------------------------------", false, Size::Normal),
            text("Funda silicona iPhone 15                  12,50", false, Size::Normal),
            text("TOTAL                     12,50", true, Size::Tall),
            Line::Qr { rows: (0..21).map(|r| (0..21).map(|c| if (r * c + r) % 3 == 0 { '1' } else { '0' }).collect()).collect(), module: 6 },
            text("        ¡Gracias por su visita! €", false, Size::Normal),
        ];
        win::draw(&writer.name, "prueba", &[Page { lines, logo: None, cols: 48 }], Some(&out)).unwrap();
        // the spooler writes the file a moment after the job ends
        for _ in 0..100 {
            if out.metadata().map(|m| m.len() > 0).unwrap_or(false) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(std::fs::read(&out).unwrap().starts_with(b"%PDF"));
    }

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
            receipt: false,
        };
        // an inkjet on USB is not a receipt printer, an Epson TM is
        let inkjet = e("EPSON L3150 Series", "USB001", "EPSON L3150 Series");
        assert!(score(&inkjet).0 <= 0);
        assert!(score(&e("EPSON TM-T20II", "USB002", "EPSON TM-T20II Receipt")).0 > 0);
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
