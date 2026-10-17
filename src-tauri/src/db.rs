use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use crate::error::Result;

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

const MIGRATIONS: &[&str] = &[SCHEMA_V1];

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", 1)?;
    conn.pragma_update(None, "foreign_keys", 1)?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version.max(0) as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}
