use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{fail, Result};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub shop_name: String,
    // the owner's full name or company name, when the shop trades under another
    pub legal_name: String,
    pub shop_address: String,
    pub shop_phone: String,
    pub shop_nif: String,
    pub footer: String,
    pub ticket_prefix: String,
    pub default_vat_bp: i64,
    pub show_vat: bool,
    pub printer_name: String,
    // "windows": the queue above; "network": raw ESC/POS over TCP to
    // printer_host:printer_port
    pub printer_kind: String,
    pub printer_host: String,
    pub printer_port: u16,
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
    // minutes without use before the till locks itself; 0 never
    pub auto_lock_min: u32,
    // practice mode: sales get their own series, print as not valid and
    // never count anywhere
    pub training: bool,
    // VERI*FACTU: off, test (the AEAT's preproducción) or prod
    pub verifactu: String,
    // SHA-1 thumbprint of the certificate in the Windows store the records
    // are sent with
    pub cert: String,
    // opened by Windows at sign-in, and full screen with no way out but the
    // button in the header
    pub autostart: bool,
    pub fullscreen: bool,
    // printed on the resguardo a customer gets for a phone left for repair
    pub repair_terms: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            shop_name: String::new(),
            legal_name: String::new(),
            shop_address: String::new(),
            shop_phone: String::new(),
            shop_nif: String::new(),
            footer: "¡Gracias por su visita!".into(),
            ticket_prefix: "T".into(),
            default_vat_bp: 2100,
            show_vat: true,
            printer_name: String::new(),
            printer_kind: "windows".into(),
            printer_host: String::new(),
            printer_port: 9100,
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
            auto_lock_min: 10,
            training: false,
            verifactu: "off".into(),
            cert: String::new(),
            autostart: false,
            fullscreen: false,
            repair_terms: "La reparación tiene 3 meses de garantía. Haga una copia de sus datos antes de dejar el equipo.".into(),
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
        if !["windows", "network"].contains(&self.printer_kind.as_str()) {
            self.printer_kind = "windows".into();
        }
        self.printer_host = self.printer_host.trim().chars().take(253).collect();
        // "192.168.1.50:9100" typed into the address box
        if let Some((host, port)) = self.printer_host.clone().split_once(':') {
            if let Ok(port) = port.parse() {
                self.printer_host = host.into();
                self.printer_port = port;
            }
        }
        if self.printer_port == 0 {
            self.printer_port = 9100;
        }
        if !["cash", "always", "never"].contains(&self.drawer.as_str()) {
            self.drawer = "cash".into();
        }
        if !["es", "en"].contains(&self.lang.as_str()) {
            self.lang = "es".into();
        }
        if ![0, 2, 5, 10, 15, 30, 60].contains(&self.auto_lock_min) {
            self.auto_lock_min = 10;
        }
        if !["off", "test", "prod"].contains(&self.verifactu.as_str()) {
            self.verifactu = "off".into();
        }
        self.cert = self.cert.chars().filter(char::is_ascii_hexdigit).take(40).collect::<String>().to_uppercase();
        self.repair_terms = self.repair_terms.trim().chars().take(600).collect();
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

// identifies this database, so state kept outside it (the cart in the
// webview) is never applied to a different or restored database
pub fn install_id(conn: &Connection) -> Result<String> {
    if let Some(id) = get(conn, "installId")? {
        return Ok(id);
    }
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut b = [0u8; 8];
    OsRng.fill_bytes(&mut b);
    let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
    put(conn, "installId", &id)?;
    Ok(id)
}

pub fn load(conn: &Connection) -> Result<Settings> {
    let Some(json) = get(conn, "settings")? else {
        return Ok(Settings::default());
    };
    if let Ok(s) = serde_json::from_str::<Settings>(&json) {
        return Ok(s.clean());
    }
    // one unreadable value must not blank the shop's name and NIF on every
    // ticket: keep each field that still reads, default only the bad ones
    crate::log::error("settings: stored value did not read; keeping the fields that do");
    let stored: Value = serde_json::from_str(&json).unwrap_or(Value::Null);
    let mut merged = serde_json::to_value(Settings::default())?;
    if let (Some(m), Some(o)) = (merged.as_object_mut(), stored.as_object()) {
        for (k, v) in o {
            let mut trial = m.clone();
            trial.insert(k.clone(), v.clone());
            if serde_json::from_value::<Settings>(Value::Object(trial)).is_ok() {
                m.insert(k.clone(), v.clone());
            }
        }
    }
    Ok(serde_json::from_value::<Settings>(merged)?.clean())
}

pub fn save(conn: &Connection, s: Settings) -> Result<Settings> {
    let mut s = s.clean();
    // REP2026 is the repairs' series; RE would make it the practice tickets' too
    if ["RE", "REP"].contains(&s.ticket_prefix.as_str()) {
        return fail("E_INVALID:ticketPrefix");
    }
    if !s.shop_nif.trim().is_empty() {
        if !crate::nif::spanish(&s.shop_nif) {
            return fail("E_INVALID:nif");
        }
        s.shop_nif = crate::nif::normalize(&s.shop_nif);
    }
    crate::verifactu::check_settings(conn, &s)?;
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

// FNV-1a: enough to tell two logos apart, no crypto needed
fn digest(s: &str) -> String {
    let h = s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3));
    format!("{h:016x}-{}", s.len())
}

// The id a new ticket records for the logo printed on it: 0 for none. Each
// distinct logo is kept for good in `logos`.
pub fn logo_id(conn: &Connection) -> Result<i64> {
    let Some(json) = get(conn, "logo")? else { return Ok(0) };
    let hash = digest(&json);
    conn.execute("INSERT OR IGNORE INTO logos (hash, logo) VALUES (?1, ?2)", params![hash, json])?;
    Ok(conn.query_row("SELECT id FROM logos WHERE hash = ?1", [hash], |r| r.get(0))?)
}

pub fn logo_by_id(conn: &Connection, id: i64) -> Result<Option<Logo>> {
    let json: Option<String> = conn.query_row("SELECT logo FROM logos WHERE id = ?1", [id], |r| r.get(0)).optional()?;
    Ok(json.and_then(|j| serde_json::from_str(&j).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_value_does_not_wipe_the_rest() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let conn = crate::db::open(&std::env::temp_dir().join(format!("tiquet-set-{n}")).join("t.db")).unwrap();
        put(&conn, "settings", r#"{"shopName":"Mi Tienda","shopNif":"B12345678","columns":"ancho","copies":2}"#).unwrap();
        let s = load(&conn).unwrap();
        assert_eq!((s.shop_name.as_str(), s.shop_nif.as_str(), s.copies, s.columns), ("Mi Tienda", "B12345678", 2, 48));
    }

    #[test]
    fn a_network_address_typed_with_its_port_is_split() {
        let s = Settings { printer_host: " 192.168.1.50:9101 ".into(), ..Settings::default() }.clean();
        assert_eq!((s.printer_host.as_str(), s.printer_port), ("192.168.1.50", 9101));
        let s = Settings { printer_host: "fe80::1".into(), printer_port: 0, printer_kind: "usb".into(), ..Settings::default() }.clean();
        assert_eq!((s.printer_host.as_str(), s.printer_port, s.printer_kind.as_str()), ("fe80::1", 9100, "windows"));
    }
}
