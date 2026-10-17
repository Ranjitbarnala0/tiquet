use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Local;
use rusqlite::{params, Connection, OpenFlags};

use crate::error::{fail, Result};
use crate::log;
use crate::money::split_vat;

const SCHEMA_V1: &str = r#"
CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE users (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL,
  role       TEXT NOT NULL CHECK (role IN ('admin', 'cashier')),
  pass_hash  TEXT NOT NULL,
  active     INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime'))
);
CREATE UNIQUE INDEX users_name ON users (name COLLATE NOCASE);

CREATE TABLE products (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL,
  price_cents INTEGER NOT NULL,
  vat_bp      INTEGER NOT NULL DEFAULT 2100,
  barcode     TEXT NOT NULL DEFAULT '',
  favorite    INTEGER NOT NULL DEFAULT 0,
  active      INTEGER NOT NULL DEFAULT 1,
  sold_qty    INTEGER NOT NULL DEFAULT 0,
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime'))
);
CREATE INDEX products_barcode ON products (barcode) WHERE barcode <> '';

CREATE TABLE tickets (
  id             INTEGER PRIMARY KEY,
  number         TEXT NOT NULL UNIQUE,
  series         TEXT NOT NULL,
  seq            INTEGER NOT NULL,
  issued_at      TEXT NOT NULL,
  user_id        INTEGER NOT NULL REFERENCES users (id),
  user_name      TEXT NOT NULL,
  total_cents    INTEGER NOT NULL,
  item_count     INTEGER NOT NULL,
  payment_method TEXT NOT NULL CHECK (payment_method IN ('cash', 'card', 'bizum')),
  tendered_cents INTEGER NOT NULL,
  change_cents   INTEGER NOT NULL,
  shop           TEXT NOT NULL,
  voided_at      TEXT,
  voided_by      TEXT,
  void_reason    TEXT,
  UNIQUE (series, seq)
);
CREATE INDEX tickets_issued ON tickets (issued_at);

CREATE TABLE ticket_lines (
  id          INTEGER PRIMARY KEY,
  ticket_id   INTEGER NOT NULL REFERENCES tickets (id),
  pos         INTEGER NOT NULL,
  product_id  INTEGER REFERENCES products (id),
  name        TEXT NOT NULL,
  qty         INTEGER NOT NULL,
  unit_cents  INTEGER NOT NULL,
  vat_bp      INTEGER NOT NULL,
  total_cents INTEGER NOT NULL
);
CREATE INDEX ticket_lines_ticket ON ticket_lines (ticket_id);

-- an issued ticket is a legal record: only the void columns may ever change
CREATE TRIGGER tickets_no_delete BEFORE DELETE ON tickets
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be deleted'); END;

CREATE TRIGGER tickets_frozen BEFORE UPDATE OF
  number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
  payment_method, tendered_cents, change_cents, shop ON tickets
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

CREATE TRIGGER ticket_lines_no_delete BEFORE DELETE ON ticket_lines
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be deleted'); END;

CREATE TRIGGER ticket_lines_frozen BEFORE UPDATE ON ticket_lines
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;
"#;

// v2: returns. A refund is a ticket of its own (a rectifying simplified
// invoice) that points at the sale it corrects, line by line.
const SCHEMA_V2: &str = r#"
ALTER TABLE tickets ADD COLUMN kind TEXT NOT NULL DEFAULT 'sale' CHECK (kind IN ('sale', 'refund'));
ALTER TABLE tickets ADD COLUMN refund_of INTEGER REFERENCES tickets (id);
ALTER TABLE tickets ADD COLUMN refund_reason TEXT;
ALTER TABLE ticket_lines ADD COLUMN refund_of_line INTEGER REFERENCES ticket_lines (id);
CREATE INDEX tickets_refund_of ON tickets (refund_of);
CREATE INDEX ticket_lines_refund_of ON ticket_lines (refund_of_line);

DROP TRIGGER tickets_frozen;
CREATE TRIGGER tickets_frozen BEFORE UPDATE OF
  number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
  payment_method, tendered_cents, change_cents, shop, kind, refund_of, refund_reason ON tickets
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;
"#;

// v3: data safety. Lines and VAT rows can only be written while a ticket is
// still open (sealed = 0), a void is final, a cart is sold once (client_id),
// numbering never goes below what was already handed out (series_floor), and
// the VAT breakdown is stored exactly as printed.
const SCHEMA_V3: &str = r#"
ALTER TABLE tickets ADD COLUMN client_id TEXT;
ALTER TABLE tickets ADD COLUMN sealed INTEGER NOT NULL DEFAULT 1;
CREATE UNIQUE INDEX tickets_client ON tickets (client_id) WHERE client_id IS NOT NULL;

CREATE TABLE series_floor (
  series TEXT PRIMARY KEY,
  seq    INTEGER NOT NULL
);

CREATE TABLE ticket_vat (
  ticket_id   INTEGER NOT NULL REFERENCES tickets (id),
  vat_bp      INTEGER NOT NULL,
  base_cents  INTEGER NOT NULL,
  vat_cents   INTEGER NOT NULL,
  total_cents INTEGER NOT NULL,
  PRIMARY KEY (ticket_id, vat_bp)
);
"#;

const SCHEMA_V3_SEAL: &str = r#"
DROP TRIGGER tickets_frozen;
CREATE TRIGGER tickets_frozen BEFORE UPDATE OF
  id, number, series, seq, issued_at, user_id, user_name, total_cents, item_count, payment_method,
  tendered_cents, change_cents, shop, kind, refund_of, refund_reason, client_id ON tickets
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

CREATE TRIGGER tickets_sealed BEFORE UPDATE OF sealed ON tickets WHEN OLD.sealed <> 0
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

CREATE TRIGGER tickets_void_once BEFORE UPDATE OF voided_at, voided_by, void_reason ON tickets
WHEN OLD.voided_at IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'a voided ticket stays voided'); END;

CREATE TRIGGER ticket_lines_open_only BEFORE INSERT ON ticket_lines
WHEN (SELECT sealed FROM tickets WHERE id = NEW.ticket_id) IS NOT 0
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

CREATE TRIGGER ticket_vat_open_only BEFORE INSERT ON ticket_vat
WHEN (SELECT sealed FROM tickets WHERE id = NEW.ticket_id) IS NOT 0
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

CREATE TRIGGER ticket_vat_frozen BEFORE UPDATE ON ticket_vat
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

CREATE TRIGGER ticket_vat_no_delete BEFORE DELETE ON ticket_vat
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be deleted'); END;

CREATE TRIGGER series_floor_up BEFORE UPDATE ON series_floor WHEN NEW.seq < OLD.seq
BEGIN SELECT RAISE(ABORT, 'numbering never goes back'); END;
"#;

// tickets issued before v3 get the breakdown they printed with: the same
// per-rate rounding the layout uses
fn backfill_vat(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("SELECT ticket_id, vat_bp, SUM(total_cents) FROM ticket_lines GROUP BY ticket_id, vat_bp")?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (ticket, bp, gross) in rows {
        let (base, vat) = split_vat(gross, bp);
        conn.execute(
            "INSERT INTO ticket_vat (ticket_id, vat_bp, base_cents, vat_cents, total_cents) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ticket, bp, base, vat, gross],
        )?;
    }
    Ok(())
}

// v4: every attempt to print a ticket and how it ended, so the first print
// that may have reached the paper is the only unmarked original; and every
// logo ever used, so a reprint shows the one the ticket was issued with.
const SCHEMA_V4: &str = r#"
CREATE TABLE ticket_prints (
  id        INTEGER PRIMARY KEY,
  ticket_id INTEGER NOT NULL REFERENCES tickets (id),
  at        TEXT NOT NULL,
  copy      INTEGER NOT NULL,
  status    TEXT NOT NULL CHECK (status IN ('sent', 'printed', 'failed', 'unknown')),
  detail    TEXT
);
CREATE INDEX ticket_prints_ticket ON ticket_prints (ticket_id);

CREATE TABLE logos (
  id   INTEGER PRIMARY KEY,
  hash TEXT NOT NULL UNIQUE,
  logo TEXT NOT NULL
);
CREATE TRIGGER logos_frozen BEFORE UPDATE ON logos
BEGIN SELECT RAISE(ABORT, 'a logo used on tickets cannot change'); END;
CREATE TRIGGER logos_no_delete BEFORE DELETE ON logos
BEGIN SELECT RAISE(ABORT, 'a logo used on tickets cannot change'); END;
"#;

// v5: wrong passwords are counted in the database, so restarting the app
// does not reset a lockout; and a sale line keeps the catalogue price it was
// sold against, so price changes at the till show up in reports.
const SCHEMA_V5: &str = r#"
CREATE TABLE attempts (
  key          INTEGER PRIMARY KEY,
  failures     INTEGER NOT NULL,
  locked_until TEXT
);
ALTER TABLE ticket_lines ADD COLUMN list_cents INTEGER;
"#;

// v6: full invoices carry the customer's details; practice tickets (training
// mode) never count and are the only tickets that may be deleted; and the
// cash drawer gets its opening float, cash moves and the count at closing.
const SCHEMA_V6: &str = r#"
ALTER TABLE tickets ADD COLUMN customer_name TEXT;
ALTER TABLE tickets ADD COLUMN customer_nif TEXT;
ALTER TABLE tickets ADD COLUMN customer_address TEXT;
ALTER TABLE tickets ADD COLUMN training INTEGER NOT NULL DEFAULT 0;
CREATE INDEX tickets_training ON tickets (training);

DROP TRIGGER tickets_frozen;
CREATE TRIGGER tickets_frozen BEFORE UPDATE OF
  id, number, series, seq, issued_at, user_id, user_name, total_cents, item_count, payment_method,
  tendered_cents, change_cents, shop, kind, refund_of, refund_reason, client_id,
  customer_name, customer_nif, customer_address, training ON tickets
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be modified'); END;

DROP TRIGGER tickets_no_delete;
CREATE TRIGGER tickets_no_delete BEFORE DELETE ON tickets WHEN OLD.training = 0
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be deleted'); END;

DROP TRIGGER ticket_lines_no_delete;
CREATE TRIGGER ticket_lines_no_delete BEFORE DELETE ON ticket_lines
WHEN (SELECT training FROM tickets WHERE id = OLD.ticket_id) IS NOT 1
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be deleted'); END;

DROP TRIGGER ticket_vat_no_delete;
CREATE TRIGGER ticket_vat_no_delete BEFORE DELETE ON ticket_vat
WHEN (SELECT training FROM tickets WHERE id = OLD.ticket_id) IS NOT 1
BEGIN SELECT RAISE(ABORT, 'issued tickets cannot be deleted'); END;

CREATE TABLE cash_sessions (
  id             INTEGER PRIMARY KEY,
  opened_at      TEXT NOT NULL,
  opened_by      TEXT NOT NULL,
  float_cents    INTEGER NOT NULL,
  closed_at      TEXT,
  closed_by      TEXT,
  expected_cents INTEGER,
  counted_cents  INTEGER,
  note           TEXT
);

CREATE TABLE cash_moves (
  id         INTEGER PRIMARY KEY,
  session_id INTEGER NOT NULL REFERENCES cash_sessions (id),
  at         TEXT NOT NULL,
  by_name    TEXT NOT NULL,
  cents      INTEGER NOT NULL,
  reason     TEXT NOT NULL
);
CREATE INDEX cash_moves_session ON cash_moves (session_id);

CREATE TRIGGER cash_closed BEFORE UPDATE ON cash_sessions WHEN OLD.closed_at IS NOT NULL
BEGIN SELECT RAISE(ABORT, 'a closed till count cannot change'); END;
CREATE TRIGGER cash_sessions_no_delete BEFORE DELETE ON cash_sessions
BEGIN SELECT RAISE(ABORT, 'till counts cannot be deleted'); END;
CREATE TRIGGER cash_moves_frozen BEFORE UPDATE ON cash_moves
BEGIN SELECT RAISE(ABORT, 'cash moves cannot change'); END;
CREATE TRIGGER cash_moves_no_delete BEFORE DELETE ON cash_moves
BEGIN SELECT RAISE(ABORT, 'cash moves cannot change'); END;
"#;

enum Step {
    Sql(&'static str),
    Code(fn(&Connection) -> Result<()>),
}

// one entry per schema version, run in order inside one transaction each
const MIGRATIONS: &[&[Step]] = &[
    &[Step::Sql(SCHEMA_V1)],
    &[Step::Sql(SCHEMA_V2)],
    &[Step::Sql(SCHEMA_V3), Step::Code(backfill_vat), Step::Sql(SCHEMA_V3_SEAL)],
    &[Step::Sql(SCHEMA_V4)],
    &[Step::Sql(SCHEMA_V5)],
    &[Step::Sql(SCHEMA_V6)],
];

pub const VERSION: i64 = MIGRATIONS.len() as i64;

pub fn open(path: &Path) -> Result<Connection> {
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    fs::create_dir_all(&dir)?;
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    // FULL: a sale that was answered as saved survives a power cut, so its
    // number can never be handed out twice
    conn.pragma_update(None, "synchronous", 2)?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    check(&conn)?;
    migrate(&conn, Some(&dir.join("migraciones")))?;
    Ok(conn)
}

// a consistent copy, safe while the app keeps running
pub fn copy_to(conn: &Connection, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let quoted = path.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{quoted}'"))?;
    Ok(())
}

// <dir>/<stem>-<date>.db, with -2, -3... when two land in the same second
pub fn unique_path(dir: &Path, stem: &str) -> PathBuf {
    let base = format!("{stem}-{}", Local::now().format("%Y%m%d-%H%M%S"));
    let mut path = dir.join(format!("{base}.db"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.db"));
        n += 1;
    }
    path
}

// opens a file someone wants to restore without writing to it, and says
// whether it is a Tiquet database this version can read
pub fn inspect(path: &Path) -> Result<i64> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .or_else(|_| fail("E_BACKUP_INVALID"))?;
    check(&conn).or_else(|_| fail("E_BACKUP_INVALID"))?;
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if version > VERSION {
        return fail(format!("E_DB_NEWER:{version}"));
    }
    let users: rusqlite::Result<i64> = conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0));
    if version < 1 || !matches!(users, Ok(n) if n > 0) {
        return fail("E_BACKUP_INVALID");
    }
    Ok(version)
}

// a damaged file is reported before anything is written on top of it
pub fn check(conn: &Connection) -> Result<()> {
    let verdict: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if verdict != "ok" {
        return fail(format!("E_DB_CORRUPT:{verdict}"));
    }
    Ok(())
}

pub fn migrate(conn: &Connection, keep_copy_in: Option<&Path>) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    // data written by a newer Tiquet must not be touched by an older one
    if version > VERSION {
        return fail(format!("E_DB_NEWER:{version}"));
    }
    if version == VERSION {
        return Ok(());
    }
    // an upgrade of real data starts from a copy it can be rolled back to
    if let (Some(dir), true) = (keep_copy_in, version > 0) {
        let copy = unique_path(dir, &format!("tiquet-v{version}"));
        copy_to(conn, &copy)?;
        log::info(format!("database v{version} copied to {} before upgrading to v{VERSION}", copy.display()));
    }
    // rebuilding a table later needs foreign keys off, and the switch is
    // ignored inside a transaction
    conn.pragma_update(None, "foreign_keys", 0)?;
    let run = || -> Result<()> {
        for (i, steps) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
            let tx = conn.unchecked_transaction()?;
            for step in steps.iter() {
                match step {
                    Step::Sql(sql) => tx.execute_batch(sql)?,
                    Step::Code(f) => f(&tx)?,
                }
            }
            tx.pragma_update(None, "user_version", (i + 1) as i64)?;
            let broken: Option<String> = tx.query_row("PRAGMA foreign_key_check", [], |r| r.get(0)).ok();
            if let Some(table) = broken {
                return fail(format!("E_DB:foreign key check failed in {table}"));
            }
            tx.commit()?;
        }
        Ok(())
    };
    let done = run();
    conn.pragma_update(None, "foreign_keys", 1)?;
    done
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_tickets_survive_the_upgrade_and_can_be_returned() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("tiquet-mig-{n}")).join("t.db");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(SCHEMA_V1).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
            c.execute_batch(
                "INSERT INTO users (name, role, pass_hash) VALUES ('Ana', 'admin', 'x');
                 INSERT INTO tickets (number, series, seq, issued_at, user_id, user_name, total_cents, item_count,
                                      payment_method, tendered_cents, change_cents, shop)
                 VALUES ('T2026-000001', 'T2026', 1, '2026-10-16T12:00:00', 1, 'Ana', 690, 1, 'cash', 690, 0, '{}');
                 INSERT INTO ticket_lines (ticket_id, pos, name, qty, unit_cents, vat_bp, total_cents)
                 VALUES (1, 0, 'Cable', 1, 690, 2100, 690);",
            )
            .unwrap();
        }
        let c = open(&path).unwrap();
        let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, VERSION);
        let sync: i64 = c.query_row("PRAGMA synchronous", [], |r| r.get(0)).unwrap();
        assert_eq!(sync, 2);
        let copies = fs::read_dir(path.parent().unwrap().join("migraciones")).unwrap().count();
        assert_eq!(copies, 1);
        let vat: (i64, i64, i64) = c
            .query_row("SELECT base_cents, vat_cents, total_cents FROM ticket_vat WHERE ticket_id = 1", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .unwrap();
        assert_eq!(vat, (570, 120, 690));
        // the upgraded ticket is sealed like any new one
        assert!(c.execute("INSERT INTO ticket_lines (ticket_id, pos, name, qty, unit_cents, vat_bp, total_cents) VALUES (1, 1, 'x', 1, 1, 0, 1)", []).is_err());
        let kind: String = c.query_row("SELECT kind FROM tickets WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(kind, "sale");
        assert!(c.execute("UPDATE tickets SET kind = 'refund' WHERE id = 1", []).is_err());
        let mut c = c;
        let user = crate::state::SessionUser { id: 1, name: "Ana".into(), role: "admin".into() };
        let line = crate::tickets::load(&c, 1).unwrap().lines[0].id.unwrap();
        let back = crate::tickets::RefundIn {
            id: 1,
            lines: vec![crate::tickets::RefundLine { line_id: line, qty: 1 }],
            method: "cash".into(),
            reason: String::new(),
            clock_ok: false,
        };
        let r = crate::tickets::refund(&mut c, &user, &crate::settings::Settings::default(), &back).unwrap();
        assert_eq!((r.total_cents, r.refund_of.unwrap().number.as_str()), (-690, "T2026-000001"));
        drop(c);

        // a database from a newer Tiquet is left alone
        let c = Connection::open(&path).unwrap();
        c.pragma_update(None, "user_version", 99).unwrap();
        drop(c);
        assert_eq!(open(&path).unwrap_err().0, "E_DB_NEWER:99");
    }
}
