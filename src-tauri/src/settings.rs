use base64::Engine;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{fail, Result};
use crate::money::div_round;

// the totals the main page can show
pub const DAY_CELLS: [&str; 6] = ["cash", "card", "recharge", "total", "expense", "net"];
const MAX_MACHINES: usize = 12;
const FIRST_MACHINE: &str = "Datáfono";

// A card terminal or a recharge machine, named by the shop. What is done on one
// keeps its id, so renaming it changes nothing and taking it away leaves its
// name on what it did.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Machine {
    pub id: i64,
    pub name: String,
    // customers pay by card on it: that money goes to the bank, not the drawer
    pub card: bool,
    // recharges are made on it
    pub recharge: bool,
    // its recharges are paid in cash at the counter, even if it also takes cards
    pub recharge_cash: bool,
    // its recharges count as an expense, less what the shop earns on them
    pub recharge_expense: bool,
    pub commission_bp: i64,
    // how much of the sales it takes counts as an expense: all of it when that
    // money is someone else's, the bank's fee, or nothing
    pub takings_bp: i64,
    pub removed: bool,
}

impl Default for Machine {
    fn default() -> Self {
        Machine { id: 0, name: String::new(), card: true, recharge: true, recharge_cash: false, recharge_expense: true, commission_bp: 0, takings_bp: 0, removed: false }
    }
}

impl Machine {
    // the part of a sale on it that counts as an expense
    pub fn sale_cost(&self, total_cents: i64) -> i64 {
        div_round(total_cents * self.takings_bp, 10_000)
    }

    // a machine that takes no cards only ever sees cash
    pub fn recharge_method(&self) -> &'static str {
        if self.card && !self.recharge_cash {
            "card"
        } else {
            "cash"
        }
    }
}

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
    // what the ticket shows of the shop: a shop that only wants a simple
    // ticket can leave its NIF, address, phone or the cashier off (a full
    // invoice and VERI*FACTU always carry the NIF)
    pub ticket_nif: bool,
    pub ticket_address: bool,
    pub ticket_phone: bool,
    pub ticket_cashier: bool,
    pub printer_name: String,
    // "windows": raw ESC/POS to the queue above; "document": the same queue
    // printing a page through its own driver (an office printer, a PDF);
    // "network": raw ESC/POS over TCP to printer_host:printer_port;
    // "serial": raw ESC/POS to a COM port (Bluetooth and USB-serial printers)
    pub printer_kind: String,
    pub printer_host: String,
    pub printer_port: u16,
    pub printer_com: String,
    pub printer_baud: u32,
    pub columns: u32,
    // Papel automático: the columns follow the printer (printer::auto_columns),
    // 58 mm when nothing says; choosing a width in Ajustes turns it off
    pub paper_auto: bool,
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
    // the user signed in when Tiquet opens, with no password asked; 0 asks
    pub auto_login: i64,
    // cashiers may correct and delete what they sold or typed today; the
    // owner may do it to anything
    pub book_staff_delete: bool,
    // new versions come down and install themselves when the till is free
    pub auto_update: bool,
    // cashiers see the day's totals on the main page
    pub staff_totals: bool,
    // cashiers may give returns too, not only the owner
    pub staff_refund: bool,
    // a Bizum button beside cash and card on the main page
    pub pay_bizum: bool,
    // what the main page and the rail offer
    pub show_recharge: bool,
    pub show_expense: bool,
    pub show_repairs: bool,
    // the totals under the main page, comma separated (DAY_CELLS); none hides them
    pub day_cells: String,
    // products on the main page and in Productos: "new", the last added first,
    // or "az"
    pub product_order: String,
    // one-tap purchases in the expense window, comma separated, and how
    // purchases are usually paid
    pub expense_concepts: String,
    pub expense_method: String,
    // what the drawer usually starts the day with, offered when opening it
    pub float_cents: i64,
    // the card terminals and recharge machines; there is always at least one
    pub machines: Vec<Machine>,
    // with more than one, the card button asks which; off, it uses card_machine
    pub machine_ask: bool,
    pub card_machine: i64,
    // what the recharge window starts on: a machine, or 0 for cash
    pub recharge_machine: i64,
    // the amounts and operators the recharge window offers, comma separated
    pub recharge_amounts: String,
    pub recharge_operators: String,
    // recharges in cash count as expenses too, less what the shop earns on them
    pub recharge_cash_expense: bool,
    pub recharge_commission_bp: i64,
    // a slip comes out with every recharge
    pub recharge_print: bool,
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
    // the printer full invoices come out of on A4, on this PC; none prints
    // them on the receipt printer
    pub invoice_printer: String,
    // under the totals of every A4 invoice
    pub invoice_terms: String,
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
            ticket_nif: true,
            ticket_address: true,
            ticket_phone: true,
            ticket_cashier: true,
            printer_name: String::new(),
            printer_kind: "windows".into(),
            printer_host: String::new(),
            printer_port: 9100,
            printer_com: String::new(),
            printer_baud: 9600,
            columns: 48,
            paper_auto: true,
            codepage: "cp858".into(),
            cut: true,
            feed_lines: 4,
            drawer: "cash".into(),
            cancel_chinese: true,
            copies: 1,
            backup_dir: String::new(),
            auto_backup: true,
            lang: "es".into(),
            auto_lock_min: 0,
            auto_login: 0,
            book_staff_delete: true,
            auto_update: true,
            staff_totals: true,
            staff_refund: false,
            pay_bizum: false,
            show_recharge: true,
            show_expense: true,
            show_repairs: true,
            day_cells: DAY_CELLS.join(","),
            product_order: "new".into(),
            expense_concepts: String::new(),
            expense_method: "cash".into(),
            float_cents: 0,
            machines: vec![Machine { id: 1, name: FIRST_MACHINE.into(), ..Machine::default() }],
            machine_ask: true,
            card_machine: 1,
            recharge_machine: 0,
            recharge_amounts: "5, 10, 15, 20, 30, 50".into(),
            recharge_operators: String::new(),
            recharge_cash_expense: false,
            recharge_commission_bp: 0,
            recharge_print: false,
            training: false,
            verifactu: "off".into(),
            cert: String::new(),
            autostart: false,
            fullscreen: false,
            repair_terms: "Si el equipo no se recoge en 3 meses desde el aviso de que está listo, la tienda deja de responder de él. La reparación tiene 3 meses de garantía. Haga una copia de sus datos antes de dejar el equipo.".into(),
            invoice_printer: String::new(),
            invoice_terms: "Garantía legal de 3 años en productos nuevos. Conserve esta factura: es necesaria para cualquier garantía o devolución.".into(),
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
        if !["windows", "document", "network", "serial"].contains(&self.printer_kind.as_str()) {
            self.printer_kind = "windows".into();
        }
        // "com5", " COM5 " or a whole "\\.\COM5": the port's own name
        let com = self.printer_com.trim().trim_start_matches(r"\\.\").to_uppercase();
        self.printer_com = match com.strip_prefix("COM") {
            Some(n) if !n.is_empty() && n.len() <= 3 && n.chars().all(|c| c.is_ascii_digit()) => com,
            _ => String::new(),
        };
        if ![1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200].contains(&self.printer_baud) {
            self.printer_baud = 9600;
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
        if ![0, 2, 5, 10, 15, 30, 60, 120, 240].contains(&self.auto_lock_min) {
            self.auto_lock_min = 0;
        }
        self.auto_login = self.auto_login.max(0);
        self.recharge_amounts = self.recharge_amounts.trim().chars().take(120).collect();
        self.recharge_operators = self.recharge_operators.trim().chars().take(200).collect();
        self.recharge_commission_bp = self.recharge_commission_bp.clamp(0, 10_000);
        self.expense_concepts = self.expense_concepts.trim().chars().take(200).collect();
        if !["cash", "card"].contains(&self.expense_method.as_str()) {
            self.expense_method = "cash".into();
        }
        if !["new", "az"].contains(&self.product_order.as_str()) {
            self.product_order = "new".into();
        }
        self.float_cents = self.float_cents.clamp(0, 100 * crate::tickets::MAX_PRICE);
        self.day_cells = DAY_CELLS.iter().filter(|c| self.day_cells.split(',').any(|x| x.trim() == **c)).copied().collect::<Vec<_>>().join(",");
        self.clean_machines();
        if !["off", "test", "prod"].contains(&self.verifactu.as_str()) {
            self.verifactu = "off".into();
        }
        // every VERI*FACTU record names the NIF, and so must its ticket
        if self.verifactu != "off" {
            self.ticket_nif = true;
        }
        self.cert = self.cert.chars().filter(char::is_ascii_hexdigit).take(40).collect::<String>().to_uppercase();
        self.repair_terms = self.repair_terms.trim().chars().take(600).collect();
        self
    }

    // there is always one, and a default is one that can do the job
    fn clean_machines(&mut self) {
        let mut next = self.machines.iter().map(|m| m.id).max().unwrap_or(0).max(0) + 1;
        let mut out: Vec<Machine> = Vec::new();
        for mut m in std::mem::take(&mut self.machines) {
            if !m.removed && out.iter().filter(|o| !o.removed).count() == MAX_MACHINES {
                continue;
            }
            if m.id <= 0 || out.iter().any(|o| o.id == m.id) {
                m.id = next;
                next += 1;
            }
            m.name = m.name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(30).collect();
            if m.name.is_empty() {
                m.name = format!("Máquina {}", m.id);
            }
            m.commission_bp = m.commission_bp.clamp(0, 10_000);
            m.takings_bp = m.takings_bp.clamp(0, 10_000);
            out.push(m);
        }
        if out.iter().all(|m| m.removed) {
            out.push(Machine { id: next, name: FIRST_MACHINE.into(), ..Machine::default() });
        }
        if !out.iter().any(|m| m.id == self.card_machine && !m.removed && m.card) {
            self.card_machine = out.iter().find(|m| !m.removed && m.card).map_or(0, |m| m.id);
        }
        if !out.iter().any(|m| m.id == self.recharge_machine && !m.removed && m.recharge) {
            self.recharge_machine = 0;
        }
        self.machines = out;
    }

    // one that was taken away is still found: what it did keeps its name
    pub fn machine(&self, id: Option<i64>) -> Option<&Machine> {
        self.machines.iter().find(|m| Some(m.id) == id)
    }

    pub fn machine_name(&self, id: i64) -> &str {
        self.machine(Some(id)).map_or("Sin máquina", |m| m.name.as_str())
    }

    // the machine a payment went through: a card's, the one asked for or the default
    pub fn paid_on(&self, method: &str, asked: Option<i64>) -> Option<&Machine> {
        if method != "card" {
            return None;
        }
        self.machine(asked).filter(|m| m.card).or_else(|| self.machine(Some(self.card_machine)))
    }

    // the screens and the paper only tell machines apart when there are several
    pub fn several_machines(&self) -> bool {
        self.machines.iter().filter(|m| !m.removed).count() > 1
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

pub fn clear(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
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

// What each PC keeps for itself when several work on one shop's data: its
// printer, its paper, its screen. Everything else is the shop's.
pub const DEVICE_KEYS: &[&str] = &[
    "printerName",
    "printerKind",
    "invoicePrinter",
    "printerHost",
    "printerPort",
    "printerCom",
    "printerBaud",
    "columns",
    "paperAuto",
    "codepage",
    "cut",
    "feedLines",
    "drawer",
    "cancelChinese",
    "copies",
    "autostart",
    "fullscreen",
    "lang",
];

// The settings as the PC being answered sees them: the shop's, with that PC's
// own printer and paper when the request comes from another PC of the shop.
pub fn load(conn: &Connection) -> Result<Settings> {
    let s = load_own(conn)?;
    Ok(match crate::state::caller().and_then(|c| c.device.clone()) {
        Some(d) => with_device(s, &d),
        None => s,
    })
}

// this PC's own part of the settings
pub fn device(s: &Settings) -> Map<String, Value> {
    let all = serde_json::to_value(s).unwrap_or_default();
    DEVICE_KEYS.iter().filter_map(|k| all.get(*k).map(|v| (k.to_string(), v.clone()))).collect()
}

// the shop's settings with a PC's own printer and paper; a value that does
// not read is left as it was
pub fn with_device(s: Settings, d: &Map<String, Value>) -> Settings {
    let Ok(Value::Object(mut all)) = serde_json::to_value(&s) else { return s };
    for k in DEVICE_KEYS {
        if let Some(v) = d.get(*k) {
            let mut trial = all.clone();
            trial.insert(k.to_string(), v.clone());
            if serde_json::from_value::<Settings>(Value::Object(trial)).is_ok() {
                all.insert(k.to_string(), v.clone());
            }
        }
    }
    serde_json::from_value::<Settings>(Value::Object(all)).map(Settings::clean).unwrap_or(s)
}

// the settings stored on this PC, whoever is asking
pub fn load_own(conn: &Connection) -> Result<Settings> {
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

// 1.5 on: the till locks itself only when someone chooses it in Ajustes.
// Installs from before kept the old default of 10 minutes, which asked for
// the password far too often at a busy counter, so it is switched off once.
pub fn upgrade(conn: &Connection) -> Result<()> {
    if get(conn, "lockOff15")?.is_none() {
        if get(conn, "settings")?.is_some() {
            let mut s = load(conn)?;
            if s.auto_lock_min != 0 {
                s.auto_lock_min = 0;
                put(conn, "settings", &serde_json::to_string(&s)?)?;
                crate::log::info("the till no longer locks itself when unused (1.5 default)");
            }
        }
        put(conn, "lockOff15", "1")?;
    }
    // 1.6: the default resguardo text also gives the 3 months to collect;
    // a text the shop wrote itself stays
    if get(conn, "terms16")?.is_none() {
        if get(conn, "settings")?.is_some() {
            let mut s = load(conn)?;
            if s.repair_terms == "La reparación tiene 3 meses de garantía. Haga una copia de sus datos antes de dejar el equipo." {
                s.repair_terms = Settings::default().repair_terms;
                put(conn, "settings", &serde_json::to_string(&s)?)?;
            }
        }
        put(conn, "terms16", "1")?;
    }
    // 1.7.1: Papel automático. 48 columns on a Windows queue was nearly always
    // the old default, and is read again from the printer; any other width,
    // or a network or Bluetooth printer set up by hand, stays as chosen.
    if get(conn, "paper171")?.is_none() {
        if get(conn, "settings")?.is_some() {
            let mut s = load(conn)?;
            if s.columns != 48 || s.printer_kind != "windows" {
                s.paper_auto = false;
                put(conn, "settings", &serde_json::to_string(&s)?)?;
            }
        }
        put(conn, "paper171", "1")?;
    }
    // 1.9: recharges go by machine. The machine every shop starts with keeps
    // the commission the shop had set.
    if get(conn, "machines19")?.is_none() {
        if get(conn, "settings")?.is_some() {
            let mut s = load(conn)?;
            let bp = s.recharge_commission_bp;
            s.machines.iter_mut().for_each(|m| m.commission_bp = bp);
            put(conn, "settings", &serde_json::to_string(&s)?)?;
        }
        put(conn, "machines19", "1")?;
    }
    Ok(())
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
    fn papel_automatico_reads_the_old_default_again_and_keeps_a_chosen_width() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        for (i, (columns, kind, auto)) in [(48, "windows", true), (42, "windows", false), (32, "windows", false), (48, "network", false)].into_iter().enumerate() {
            let conn = crate::db::open(&std::env::temp_dir().join(format!("tiquet-paper-{n}-{i}")).join("t.db")).unwrap();
            // stored by 1.7.0, which had no paperAuto
            put(&conn, "settings", &format!(r#"{{"shopName":"Mi Tienda","columns":{columns},"printerKind":"{kind}"}}"#)).unwrap();
            upgrade(&conn).unwrap();
            let s = load(&conn).unwrap();
            assert_eq!((s.paper_auto, s.columns), (auto, columns), "{columns} {kind}");
        }
    }

    #[test]
    fn a_shop_always_has_a_machine_and_its_defaults_can_do_the_job() {
        // all taken away: one comes back, with an id none of them had
        let gone = Machine { id: 4, name: "Vieja".into(), removed: true, ..Machine::default() };
        let s = Settings { machines: vec![gone.clone()], card_machine: 4, recharge_machine: 4, ..Settings::default() }.clean();
        assert_eq!(s.machines.len(), 2);
        assert_eq!((s.machines[1].id, s.machines[1].name.as_str(), s.card_machine, s.recharge_machine), (5, "Datáfono", 5, 0));
        assert_eq!(s.machine_name(4), "Vieja");
        // a blank name, a repeated id, an amount that is no percentage
        let a = Machine { id: 2, name: "   ".into(), commission_bp: 20_000, takings_bp: -1, ..Machine::default() };
        let s = Settings { machines: vec![a.clone(), a], card_machine: 9, ..Settings::default() }.clean();
        assert_eq!(s.machines.iter().map(|m| m.id).collect::<Vec<_>>(), [2, 3]);
        assert_eq!((s.machines[0].name.as_str(), s.machines[0].commission_bp, s.machines[0].takings_bp), ("Máquina 2", 10_000, 0));
        assert_eq!(s.card_machine, 2);
        // a recharge machine cannot be the card default, nor a card one the recharges'
        let cards = Machine { id: 1, name: "BBVA".into(), recharge: false, ..Machine::default() };
        let tops = Machine { id: 2, name: "Recargas".into(), card: false, ..Machine::default() };
        let s = Settings { machines: vec![cards, tops], card_machine: 2, recharge_machine: 1, ..Settings::default() }.clean();
        assert_eq!((s.card_machine, s.recharge_machine), (1, 0));
        assert!(s.several_machines());
        assert_eq!(s.machine(Some(2)).unwrap().recharge_method(), "cash");
        // a card goes on the machine asked for if it takes cards, or the default
        assert_eq!(s.paid_on("card", Some(2)).map(|m| m.id), Some(1));
        assert_eq!(s.paid_on("cash", Some(1)).map(|m| m.id), None);
        // the totals are the known ones, and none at all is a choice
        let s = Settings { day_cells: " net, oro ,cash".into(), ..Settings::default() }.clean();
        assert_eq!(s.day_cells, "cash,net");
        assert_eq!(Settings { day_cells: String::new(), ..Settings::default() }.clean().day_cells, "");
        // products go newest first unless the shop chose the alphabet
        assert_eq!(Settings::default().product_order, "new");
        assert_eq!(Settings { product_order: "az".into(), ..Settings::default() }.clean().product_order, "az");
        assert_eq!(Settings { product_order: "x".into(), ..Settings::default() }.clean().product_order, "new");
    }

    #[test]
    fn the_first_machine_keeps_the_commission_the_shop_had() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let conn = crate::db::open(&std::env::temp_dir().join(format!("tiquet-mach-{n}")).join("t.db")).unwrap();
        // stored by 1.8, which had no machines
        put(&conn, "settings", r#"{"shopName":"Mi Tienda","rechargeCommissionBp":350}"#).unwrap();
        upgrade(&conn).unwrap();
        let s = load(&conn).unwrap();
        assert_eq!((s.machines.len(), s.machines[0].commission_bp, s.card_machine, s.shop_name.as_str()), (1, 350, 1, "Mi Tienda"));
        assert!(s.machines[0].card && s.machines[0].recharge && s.machines[0].recharge_expense && !s.recharge_cash_expense);
        // once: a commission changed later stays
        let mut s = s;
        s.machines[0].commission_bp = 100;
        put(&conn, "settings", &serde_json::to_string(&s).unwrap()).unwrap();
        upgrade(&conn).unwrap();
        assert_eq!(load(&conn).unwrap().machines[0].commission_bp, 100);
    }

    #[test]
    fn a_network_address_typed_with_its_port_is_split() {
        let s = Settings { printer_host: " 192.168.1.50:9101 ".into(), ..Settings::default() }.clean();
        assert_eq!((s.printer_host.as_str(), s.printer_port), ("192.168.1.50", 9101));
        let s = Settings { printer_host: "fe80::1".into(), printer_port: 0, printer_kind: "usb".into(), ..Settings::default() }.clean();
        assert_eq!((s.printer_host.as_str(), s.printer_port, s.printer_kind.as_str()), ("fe80::1", 9100, "windows"));
    }
}
