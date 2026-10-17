use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{fail, Result};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub shop_name: String,
    pub shop_address: String,
    pub shop_phone: String,
    pub shop_nif: String,
    pub footer: String,
    pub ticket_prefix: String,
    pub default_vat_bp: i64,
    pub show_vat: bool,
    pub printer_name: String,
    pub columns: u32,
    pub codepage: String,
    pub cut: bool,
    pub feed_lines: u32,
    pub drawer: String,
    pub cancel_chinese: bool,
    pub copies: u32,
    pub backup_dir: String,
    pub auto_backup: bool,
    pub lang: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            shop_name: String::new(),
            shop_address: String::new(),
            shop_phone: String::new(),
            shop_nif: String::new(),
            footer: "¡Gracias por su visita!".into(),
            ticket_prefix: "T".into(),
            default_vat_bp: 2100,
            show_vat: true,
            printer_name: String::new(),
            columns: 48,
            codepage: "cp858".into(),
            cut: true,
            feed_lines: 4,
            drawer: "cash".into(),
            cancel_chinese: true,
            copies: 1,
            backup_dir: String::new(),
            auto_backup: true,
            lang: "es".into(),
        }
    }
}

impl Settings {
    pub fn clean(mut self) -> Settings {
        if ![32, 42, 48].contains(&self.columns) {
            self.columns = 48;
        }
        self.feed_lines = self.feed_lines.min(12);
        self.copies = self.copies.clamp(1, 3);
        self.default_vat_bp = self.default_vat_bp.clamp(0, 10_000);
        let prefix: String = self
            .ticket_prefix
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(6)
            .collect();
        self.ticket_prefix = if prefix.is_empty() { "T".into() } else { prefix.to_uppercase() };
        if !["cp858", "cp850", "cp1252", "ascii"].contains(&self.codepage.as_str()) {
            self.codepage = "cp858".into();
        }
        if !["cash", "always", "never"].contains(&self.drawer.as_str()) {
            self.drawer = "cash".into();
        }
        if !["es", "en"].contains(&self.lang.as_str()) {
            self.lang = "es".into();
        }
        self
    }
}

pub fn get(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn put(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn load(conn: &Connection) -> Result<Settings> {
    Ok(match get(conn, "settings")? {
        Some(json) => serde_json::from_str::<Settings>(&json).unwrap_or_default().clean(),
        None => Settings::default(),
    })
}

pub fn save(conn: &Connection, s: Settings) -> Result<Settings> {
    let s = s.clean();
    put(conn, "settings", &serde_json::to_string(&s)?)?;
    Ok(s)
}

// 1-bit raster ready for GS v 0: rows of ceil(w/8) bytes, MSB first, 1 = black.
// `png` is the same bitmap for on-screen preview.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Logo {
    pub w: u32,
    pub h: u32,
    pub bits: String,
    pub png: String,
}

impl Logo {
    pub fn raster(&self) -> Option<Vec<u8>> {
        let bytes = base64::engine::general_purpose::STANDARD.decode(&self.bits).ok()?;
        let need = (self.w as usize).div_ceil(8) * self.h as usize;
        (bytes.len() == need).then_some(bytes)
    }
}

pub fn logo(conn: &Connection) -> Result<Option<Logo>> {
    Ok(get(conn, "logo")?.and_then(|j| serde_json::from_str(&j).ok()))
}

pub fn set_logo(conn: &Connection, l: &Logo) -> Result<()> {
    if !(8..=576).contains(&l.w) || !(1..=800).contains(&l.h) || l.raster().is_none() {
        return fail("E_INVALID:logo");
    }
    if !l.png.starts_with("data:image/png;base64,") || l.png.len() > 3_000_000 {
        return fail("E_INVALID:logo");
    }
    put(conn, "logo", &serde_json::to_string(l)?)
}

pub fn clear_logo(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = 'logo'", [])?;
    Ok(())
}
