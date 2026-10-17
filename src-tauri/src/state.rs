use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use rusqlite::Connection;
use serde::Serialize;

use crate::db;
use crate::error::{fail, Result};

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
    failures: Mutex<HashMap<i64, (u32, Instant)>>,
    pub sink: PrintSink,
    pub data_dir: PathBuf,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl AppState {
    pub fn open(data_dir: &Path, sink: PrintSink) -> Result<AppState> {
        let conn = db::open(&data_dir.join("tiquet.db"))?;
        Ok(AppState {
            db: Mutex::new(conn),
            session: Mutex::new(None),
            failures: Mutex::new(HashMap::new()),
            sink,
            data_dir: data_dir.to_path_buf(),
        })
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

    pub fn failures(&self) -> MutexGuard<'_, HashMap<i64, (u32, Instant)>> {
        lock(&self.failures)
    }

    pub fn user(&self) -> Result<SessionUser> {
        match self.current() {
            Some(u) => Ok(u),
            None => fail("E_SESSION"),
        }
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
