use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{fail, Result};
use crate::lan::Main;
use crate::printer::Job;
use crate::tickets::{self, TicketRecord};
use crate::{db, log, paths};

pub enum PrintSink {
    Spooler,
    Dir(PathBuf),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionUser {
    pub id: i64,
    pub name: String,
    pub role: String,
}

pub struct AppState {
    db: Mutex<Connection>,
    session: Mutex<Option<SessionUser>>,
    // last thing someone did at the till, and how long it may sit unused
    active: Mutex<Instant>,
    lock_after: AtomicU64,
    // Ajustes can sign someone in when Tiquet opens: once per start, never
    // again after they lock the till or sign out
    auto_login_done: AtomicBool,
    // Papel automático reads the paper of the printer set up once a run
    paper_read: AtomicBool,
    pub sink: PrintSink,
    pub data_dir: PathBuf,
    // only the packaged app opens Explorer windows; tests and the dev server
    // just answer with the path
    pub desktop: bool,
    // the main till this PC works with, when it is not the main one itself
    remote: Mutex<Option<Main>>,
}

// Another PC of the shop signed in on this till: who, and when it was last
// used, apart from this till's own screen.
pub struct Slot {
    session: Mutex<Option<SessionUser>>,
    active: Mutex<Instant>,
}

impl Default for Slot {
    fn default() -> Self {
        Slot::new()
    }
}

impl Slot {
    pub fn new() -> Slot {
        Slot { session: Mutex::new(None), active: Mutex::new(Instant::now()) }
    }

    pub fn user(&self) -> Option<SessionUser> {
        lock(&self.session).clone()
    }

    pub fn sign_in(&self, u: SessionUser) {
        *lock(&self.active) = Instant::now();
        *lock(&self.session) = Some(u);
    }
}

// The request being answered for another PC of the shop, on this thread: its
// sign-in, its own printer and paper, and what was printed for it, which goes
// back to it to be printed there.
pub struct Caller {
    pub slot: Arc<Slot>,
    pub device: Option<Map<String, Value>>,
    pub jobs: RefCell<Vec<(String, Job)>>,
}

thread_local! {
    static CALLER: RefCell<Option<Rc<Caller>>> = const { RefCell::new(None) };
}

pub fn caller() -> Option<Rc<Caller>> {
    CALLER.with(|c| c.borrow().clone())
}

pub fn as_caller<T>(c: Rc<Caller>, f: impl FnOnce() -> T) -> T {
    struct Done;
    impl Drop for Done {
        fn drop(&mut self) {
            CALLER.with(|c| *c.borrow_mut() = None);
        }
    }
    CALLER.with(|x| *x.borrow_mut() = Some(c));
    let _done = Done;
    f()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    pub fn open(data_dir: &Path, sink: PrintSink) -> Result<AppState> {
        log::init(data_dir);
        log::info(format!("Tiquet {} starting, data in {}", env!("CARGO_PKG_VERSION"), data_dir.display()));
        let conn = db::open(&data_dir.join("tiquet.db")).inspect_err(|e| log::error(format!("database: {e}")))?;
        let st = AppState {
            db: Mutex::new(conn),
            session: Mutex::new(None),
            active: Mutex::new(Instant::now()),
            lock_after: AtomicU64::new(0),
            auto_login_done: AtomicBool::new(false),
            paper_read: AtomicBool::new(false),
            sink,
            data_dir: data_dir.to_path_buf(),
            desktop: false,
            remote: Mutex::new(None),
        };
        st.set_remote(crate::lan::load_main(&st.conn()));
        st.sync_marks()?;
        crate::settings::upgrade(&st.conn())?;
        let minutes = crate::settings::load(&st.conn())?.auto_lock_min;
        st.set_lock_after(minutes);
        Ok(st)
    }

    // what --check reports about the database it opened
    pub fn health(&self) -> serde_json::Value {
        let conn = self.conn();
        let one = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(-1);
        let last: Option<String> =
            conn.query_row("SELECT number FROM tickets WHERE training = 0 ORDER BY id DESC LIMIT 1", [], |r| r.get(0)).ok();
        serde_json::json!({
            "version": one("PRAGMA user_version"),
            "tickets": one("SELECT COUNT(*) FROM tickets WHERE training = 0"),
            "lines": one("SELECT COUNT(*) FROM ticket_lines"),
            "users": one("SELECT COUNT(*) FROM users"),
            "products": one("SELECT COUNT(*) FROM products"),
            "fiscalRecords": one("SELECT COUNT(*) FROM fiscal_records"),
            "fiscalPending": one("SELECT COUNT(*) FROM fiscal_records r WHERE NOT EXISTS (SELECT 1 FROM fiscal_sends s WHERE s.record_id = r.id)"),
            "lastTicket": last,
            "integrity": conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)).unwrap_or_default(),
        })
    }

    // true the first time it is asked after the app starts
    pub fn first_open(&self) -> bool {
        !self.auto_login_done.swap(true, Ordering::SeqCst)
    }

    pub fn paper_read(&self) -> bool {
        self.paper_read.load(Ordering::SeqCst)
    }

    pub fn set_paper_read(&self) {
        self.paper_read.store(true, Ordering::SeqCst);
    }

    pub fn set_lock_after(&self, minutes: u32) {
        self.lock_after.store(minutes as u64 * 60, Ordering::SeqCst);
    }

    pub fn touch(&self) {
        match caller() {
            Some(c) => *lock(&c.slot.active) = Instant::now(),
            None => *lock(&self.active) = Instant::now(),
        }
    }

    // Explorer, the clipboard and links open on the PC the person is at
    pub fn desktop(&self) -> bool {
        self.desktop && caller().is_none()
    }

    pub fn remote(&self) -> Option<Main> {
        lock(&self.remote).clone()
    }

    pub fn set_remote(&self, m: Option<Main>) {
        *lock(&self.remote) = m;
    }

    #[cfg(test)]
    pub fn active_for_test(&self) -> MutexGuard<'_, Instant> {
        lock(&self.active)
    }

    // The numbering file and the database each learn what the other has seen.
    // Returns the ranges printed at some point but missing from the database.
    pub fn sync_marks(&self) -> Result<Vec<(String, i64, i64)>> {
        let conn = self.conn();
        let missing = tickets::raise_floors(&conn, &paths::read_marks(&self.data_dir))?;
        for (series, from, to) in &missing {
            log::warn(format!("numbers {series}-{from:06} to {series}-{to:06} were issued but are not in this database"));
        }
        paths::write_marks(&self.data_dir, &tickets::series_marks(&conn)?);
        Ok(missing)
    }

    // right after a ticket is committed, before it is printed
    pub fn note(&self, t: &TicketRecord) {
        self.mark(&t.number);
    }

    // a ticket or repair number now handed out
    pub fn mark(&self, number: &str) {
        if let Some((series, seq)) = number.rsplit_once('-') {
            if let Ok(seq) = seq.parse() {
                paths::write_marks(&self.data_dir, &[(series.to_string(), seq)].into());
            }
        }
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        lock(&self.db)
    }

    pub fn current(&self) -> Option<SessionUser> {
        match caller() {
            Some(c) => c.slot.user(),
            None => lock(&self.session).clone(),
        }
    }

    pub fn set_session(&self, u: Option<SessionUser>) {
        match caller() {
            Some(c) => *lock(&c.slot.session) = u,
            None => *lock(&self.session) = u,
        }
    }

    // A till left unused locks itself, so an admin who walks away does not
    // leave voids, refunds and user management open to whoever comes next.
    pub fn user(&self) -> Result<SessionUser> {
        let Some(u) = self.current() else {
            return fail("E_SESSION");
        };
        let limit = self.lock_after.load(Ordering::SeqCst);
        let idle = match caller() {
            Some(c) => lock(&c.slot.active).elapsed(),
            None => lock(&self.active).elapsed(),
        };
        if limit > 0 && idle.as_secs() >= limit {
            self.set_session(None);
            log::info(format!("{} locked out after {} min without use", u.name, limit / 60));
            return fail("E_SESSION_IDLE");
        }
        Ok(u)
    }

    pub fn admin(&self) -> Result<SessionUser> {
        let u = self.user()?;
        if u.role == "admin" {
            Ok(u)
        } else {
            fail("E_FORBIDDEN")
        }
    }
}
