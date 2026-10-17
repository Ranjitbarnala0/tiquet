use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::Connection;
use serde::Serialize;

use crate::error::{fail, Result};
use crate::tickets::{self, TicketRecord};
use crate::{db, log, paths};

pub enum PrintSink {
    Spooler,
    Dir(PathBuf),
}

#[derive(Clone, Debug, Serialize)]
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
    pub sink: PrintSink,
    pub data_dir: PathBuf,
    // only the packaged app opens Explorer windows; tests and the dev server
    // just answer with the path
    pub desktop: bool,
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
            sink,
            data_dir: data_dir.to_path_buf(),
            desktop: false,
        };
        st.sync_marks()?;
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

    pub fn set_lock_after(&self, minutes: u32) {
        self.lock_after.store(minutes as u64 * 60, Ordering::SeqCst);
    }

    pub fn touch(&self) {
        *lock(&self.active) = Instant::now();
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
            log::warn(format!("tickets {series}-{from:06} to {series}-{to:06} were issued but are not in this database"));
        }
        paths::write_marks(&self.data_dir, &tickets::series_marks(&conn)?);
        Ok(missing)
    }

    // right after a ticket is committed, before it is printed
    pub fn note(&self, t: &TicketRecord) {
        if let Some((series, seq)) = t.number.rsplit_once('-') {
            if let Ok(seq) = seq.parse() {
                paths::write_marks(&self.data_dir, &[(series.to_string(), seq)].into());
            }
        }
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        lock(&self.db)
    }

    pub fn current(&self) -> Option<SessionUser> {
        lock(&self.session).clone()
    }

    pub fn set_session(&self, u: Option<SessionUser>) {
        *lock(&self.session) = u;
    }

    // A till left unused locks itself, so an admin who walks away does not
    // leave voids, refunds and user management open to whoever comes next.
    pub fn user(&self) -> Result<SessionUser> {
        let Some(u) = self.current() else {
            return fail("E_SESSION");
        };
        let limit = self.lock_after.load(Ordering::SeqCst);
        if limit > 0 && lock(&self.active).elapsed().as_secs() >= limit {
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
