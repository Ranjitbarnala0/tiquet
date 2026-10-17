// Single entry point for the UI. The Tauri shell and the dev HTTP server
// both call `dispatch`, so they exercise exactly the same code.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{fail, Error, Result};
use crate::escpos::{self, Opts};
use crate::layout::{self, TicketView};
use crate::settings::{self, Logo, Settings};
use crate::state::{AppState, SessionUser};
use crate::tickets::{self, PaymentIn, SaleLine, ShopInfo, TicketRecord};
use crate::{auth, backup, printer, products, reports, users};

pub fn dispatch(st: &AppState, cmd: &str, args: Value) -> std::result::Result<Value, String> {
    route(st, cmd, args).map_err(|e| e.0)
}

fn parse<T: DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|e| Error(format!("E_ARGS:{e}")))
}

fn to<T: Serialize>(v: T) -> Result<Value> {
    Ok(serde_json::to_value(v)?)
}

fn id(a: &Value) -> Result<i64> {
    a.get("id").and_then(Value::as_i64).ok_or_else(|| Error("E_ARGS:id".into()))
}

fn flag(a: &Value, key: &str, default: bool) -> bool {
    a.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn route(st: &AppState, cmd: &str, a: Value) -> Result<Value> {
    match cmd {
        "bootstrap" => bootstrap(st),
        "setup" => setup(st, parse(a)?),
        "login" => login(st, parse(a)?),
        "logout" => {
            st.set_session(None);
            Ok(Value::Null)
        }

        "settings.get" => {
            st.user()?;
            to(settings::load(&st.conn())?)
        }
        "settings.save" => {
            st.admin()?;
            // keys the caller leaves out keep their saved value
            let conn = st.conn();
            let mut merged = serde_json::to_value(settings::load(&conn)?)?;
            if let (Some(cur), Some(new)) = (merged.as_object_mut(), a.as_object()) {
                for (k, v) in new {
                    cur.insert(k.clone(), v.clone());
                }
            }
            to(settings::save(&conn, parse(merged)?)?)
        }
        "logo.get" => {
            st.user()?;
            to(settings::logo(&st.conn())?)
        }
        "logo.set" => {
            st.admin()?;
            let l: Logo = parse(a)?;
            settings::set_logo(&st.conn(), &l)?;
            Ok(Value::Null)
        }
        "logo.clear" => {
            st.admin()?;
            settings::clear_logo(&st.conn())?;
            Ok(Value::Null)
        }

        "products.list" => {
            st.user()?;
            to(products::list(&st.conn(), flag(&a, "all", false))?)
        }
        "products.save" => {
            st.admin()?;
            to(products::save(&st.conn(), parse(a)?)?)
        }
        "products.import" => {
            st.admin()?;
            let rows: Vec<products::Product> = parse(a.get("rows").cloned().unwrap_or_default())?;
            to(products::import(&mut st.conn(), rows)?)
        }

        "sale.preview" => {
            st.user()?;
            preview(st, parse(a)?)
        }
        "sale.finalize" => {
            st.user()?;
            finalize(st, parse(a)?)
        }

        "tickets.list" => {
            st.user()?;
            to(tickets::list(&st.conn(), &parse(a)?)?)
        }
        "tickets.get" => ticket_get(st, id(&a)?),
        "tickets.print" => {
            st.user()?;
            let (rec, s, logo) = {
                let conn = st.conn();
                (tickets::load(&conn, id(&a)?)?, settings::load(&conn)?, settings::logo(&conn)?)
            };
            print_ticket(st, &rec, &s, logo.as_ref(), flag(&a, "copy", true))?;
            Ok(Value::Null)
        }
        "tickets.void" => {
            let me = st.admin()?;
            let reason = a.get("reason").and_then(Value::as_str).unwrap_or_default();
            tickets::void(&mut st.conn(), id(&a)?, &me.name, reason)?;
            ticket_get(st, id(&a)?)
        }

        "reports.summary" => {
            st.admin()?;
            to(reports::summary(&st.conn(), &parse(a)?)?)
        }
        "reports.print" => {
            let me = st.admin()?;
            let r: reports::Range = parse(a)?;
            let (lines, s) = {
                let conn = st.conn();
                let s = settings::load(&conn)?;
                let sum = reports::summary(&conn, &r)?;
                (layout::summary(&sum, &r.from, &r.to, &s, &me.name), s)
            };
            let bytes = escpos::document(&lines, None, &Opts::new(&s, false));
            printer::send(&st.sink, &s.printer_name, "resumen", &bytes)?;
            Ok(Value::Null)
        }
        "reports.export" => {
            st.admin()?;
            let path = reports::export_csv(&st.conn(), &parse(a)?)?;
            Ok(json!({ "path": path.to_string_lossy() }))
        }

        "users.list" => {
            st.admin()?;
            to(users::list(&st.conn(), false)?)
        }
        "users.save" => {
            let me = st.admin()?;
            let row = users::save(&st.conn(), parse(a)?)?;
            if row.id == me.id {
                st.set_session(Some(SessionUser { id: row.id, name: row.name.clone(), role: row.role.clone() }));
            }
            to(row)
        }

        "printers.list" => {
            st.admin()?;
            to(printer::list())
        }
        "printers.test" => {
            st.admin()?;
            printer_test(st, parse(a)?)
        }

        "backup.now" => {
            st.admin()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            let path = backup::run(&conn, &backup::dir(&s))?;
            Ok(json!({ "path": path.to_string_lossy() }))
        }
        "backup.status" => {
            st.admin()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            Ok(json!({
                "dir": backup::dir(&s).to_string_lossy(),
                "defaultDir": backup::dir(&Settings::default()).to_string_lossy(),
                "last": settings::get(&conn, "lastBackupAt")?,
                "lastPath": settings::get(&conn, "lastBackupPath")?,
                "error": settings::get(&conn, "lastBackupError")?,
                "dataDir": st.data_dir.to_string_lossy(),
            }))
        }

        _ => fail(format!("E_UNKNOWN:{cmd}")),
    }
}

fn bootstrap(st: &AppState) -> Result<Value> {
    let conn = st.conn();
    let s = settings::load(&conn)?;
    let list: Vec<Value> = users::list(&conn, true)?
        .into_iter()
        .map(|u| json!({ "id": u.id, "name": u.name, "role": u.role }))
        .collect();
    Ok(json!({
        "needsSetup": users::count(&conn)? == 0,
        "users": list,
        "lang": s.lang,
        "shopName": s.shop_name,
        "session": st.current(),
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetupIn {
    shop_name: String,
    #[serde(default)]
    shop_address: String,
    #[serde(default)]
    shop_phone: String,
    #[serde(default)]
    shop_nif: String,
    admin_name: String,
    password: String,
    #[serde(default)]
    samples: bool,
    #[serde(default)]
    lang: String,
}

fn setup(st: &AppState, a: SetupIn) -> Result<Value> {
    let mut conn = st.conn();
    if users::count(&conn)? > 0 {
        return fail("E_SETUP_DONE");
    }
    if a.shop_name.trim().is_empty() {
        return fail("E_INVALID:shopName");
    }
    let s = Settings {
        shop_name: a.shop_name.trim().into(),
        shop_address: a.shop_address.trim().into(),
        shop_phone: a.shop_phone.trim().into(),
        shop_nif: a.shop_nif.trim().to_uppercase(),
        lang: a.lang,
        ..settings::load(&conn)?
    };
    let tx = conn.transaction()?;
    settings::save(&tx, s)?;
    let u = users::save(
        &tx,
        users::UserIn { id: None, name: a.admin_name, role: "admin".into(), password: a.password, active: true },
    )?;
    if a.samples {
        products::seed(&tx)?;
    }
    tx.commit()?;
    drop(conn);
    let me = SessionUser { id: u.id, name: u.name, role: u.role };
    st.set_session(Some(me.clone()));
    Ok(json!({ "user": me }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginIn {
    user_id: i64,
    password: String,
}

// five wrong passwords lock that user for 30 seconds
fn login(st: &AppState, a: LoginIn) -> Result<Value> {
    if let Some((n, at)) = st.failures().get(&a.user_id) {
        let wait = 30u64.saturating_sub(at.elapsed().as_secs());
        if *n >= 5 && wait > 0 {
            return fail(format!("E_LOCKED:{wait}"));
        }
    }
    let row = users::credentials(&st.conn(), a.user_id)?;
    let user = match row {
        Some((u, hash, true)) if auth::verify(&a.password, &hash) => u,
        _ => {
            let mut f = st.failures();
            let e = f.entry(a.user_id).or_insert((0, std::time::Instant::now()));
            if e.1.elapsed().as_secs() >= 30 {
                e.0 = 0;
            }
            e.0 += 1;
            e.1 = std::time::Instant::now();
            return fail("E_AUTH");
        }
    };
    st.failures().remove(&a.user_id);
    st.set_session(Some(user.clone()));
    backup::auto(&st.conn());
    Ok(json!({ "user": user }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewIn {
    #[serde(default)]
    lines: Vec<SaleLine>,
    #[serde(default)]
    payment: Option<PaymentIn>,
    #[serde(default)]
    settings: Option<Settings>,
    #[serde(default)]
    sample: bool,
}

fn preview(st: &AppState, a: PreviewIn) -> Result<Value> {
    let me = st.user()?;
    if a.settings.is_some() {
        st.admin()?;
    }
    let conn = st.conn();
    let s = match a.settings {
        Some(s) => s.clean(),
        None => settings::load(&conn)?,
    };
    let logo = settings::logo(&conn)?;
    let lines = if a.sample { tickets::sample_lines(s.default_vat_bp) } else { tickets::clamp(a.lines) };
    let totals = tickets::compute(&lines);
    let payment_in = if a.sample { Some(PaymentIn { method: "cash".into(), tendered_cents: Some(5000) }) } else { a.payment };
    let (payment, payment_error) = match payment_in.map(|p| tickets::resolve_payment(&p, totals.total_cents)) {
        Some(Ok(p)) => (Some(p), None),
        Some(Err(e)) => (None, Some(e.0)),
        None => (None, None),
    };
    let issued_at = tickets::now();
    let (_, _, number) = tickets::next_number(&conn, &s.ticket_prefix, &issued_at)?;
    let shop = ShopInfo::from_settings(&s);
    let v = TicketView {
        shop: &shop,
        number: &number,
        issued_at: &issued_at,
        user_name: &me.name,
        lines: &lines,
        totals: &totals,
        payment: payment.as_ref(),
        copy: false,
        voided: false,
    };
    Ok(json!({
        "lines": layout::ticket(&v, &s, logo.is_some()),
        "totals": totals,
        "number": number,
        "payment": payment,
        "paymentError": payment_error,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinalizeIn {
    lines: Vec<SaleLine>,
    payment: PaymentIn,
    #[serde(default = "yes")]
    print: bool,
}

fn yes() -> bool {
    true
}

fn drawer_opens(s: &Settings, method: &str) -> bool {
    match s.drawer.as_str() {
        "always" => true,
        "cash" => method == "cash",
        _ => false,
    }
}

// the sale is committed before anything is sent to the printer: a jammed or
// unplugged printer never loses a ticket, it only reports printError
fn finalize(st: &AppState, a: FinalizeIn) -> Result<Value> {
    let me = st.user()?;
    let (rec, s, logo) = {
        let mut conn = st.conn();
        let s = settings::load(&conn)?;
        let rec = tickets::finalize(&mut conn, &me, &s, a.lines, &a.payment)?;
        let logo = settings::logo(&conn)?;
        (rec, s, logo)
    };
    let print_error = if a.print {
        print_ticket(st, &rec, &s, logo.as_ref(), false).err().map(|e| e.0)
    } else {
        if drawer_opens(&s, &rec.payment.method) {
            let _ = printer::send(&st.sink, &s.printer_name, "cajon", &escpos::kick(&s));
        }
        None
    };
    Ok(json!({ "ticket": rec, "printed": a.print && print_error.is_none(), "printError": print_error }))
}

fn print_ticket(st: &AppState, rec: &TicketRecord, s: &Settings, logo: Option<&Logo>, copy: bool) -> Result<()> {
    let lines = layout::record(rec, s, logo.is_some(), copy);
    let copies = if copy { 1 } else { s.copies };
    let mut data = Vec::new();
    for i in 0..copies {
        let kick = i == 0 && !copy && drawer_opens(s, &rec.payment.method);
        data.extend(escpos::document(&lines, logo, &Opts::new(s, kick)));
    }
    printer::send(&st.sink, &s.printer_name, &rec.number, &data)
}

fn ticket_get(st: &AppState, id: i64) -> Result<Value> {
    st.user()?;
    let conn = st.conn();
    let rec = tickets::load(&conn, id)?;
    let s = settings::load(&conn)?;
    let logo = settings::logo(&conn)?;
    let lines = layout::record(&rec, &s, logo.is_some(), false);
    Ok(json!({ "ticket": rec, "lines": lines }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestIn {
    settings: Settings,
    kind: String,
}

fn printer_test(st: &AppState, t: TestIn) -> Result<Value> {
    let me = st.admin()?;
    let s = t.settings.clean();
    let logo = settings::logo(&st.conn())?;
    let bytes = match t.kind.as_str() {
        "charset" => escpos::charset_test(&s),
        "drawer" => escpos::kick(&s),
        _ => {
            let lines = layout::test_page(&s, &me.name, logo.is_some());
            escpos::document(&lines, logo.as_ref(), &Opts::new(&s, false))
        }
    };
    printer::send(&st.sink, &s.printer_name, "prueba", &bytes)?;
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PrintSink;

    fn state(sink: impl FnOnce(&std::path::Path) -> PrintSink) -> AppState {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("tiquet-api-{}-{n}", std::process::id()));
        AppState::open(&dir, sink(&dir)).unwrap()
    }

    fn setup(st: &AppState) {
        let args = json!({ "shopName": "Tienda", "adminName": "Ana", "password": "1234", "samples": true, "lang": "es" });
        dispatch(st, "setup", args).unwrap();
    }

    fn sale() -> Value {
        json!({ "lines": [{ "name": "Cable", "qty": 1, "unitCents": 690, "vatBp": 2100 }], "payment": { "method": "cash" } })
    }

    #[test]
    fn a_failed_print_never_loses_the_sale() {
        let st = state(|_| PrintSink::Spooler);
        setup(&st);
        let r = dispatch(&st, "sale.finalize", sale()).unwrap();
        assert_eq!(r["printed"], false);
        assert_eq!(r["printError"], "E_NO_PRINTER");
        assert_eq!(r["ticket"]["totalCents"], 690);
        let list = dispatch(&st, "tickets.list", json!({})).unwrap();
        assert_eq!(list.as_array().unwrap().len(), 1);
    }

    #[test]
    fn roles_are_enforced_by_the_core() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        assert_eq!(dispatch(&st, "products.list", json!({})).unwrap_err(), "E_SESSION");
        setup(&st);
        let jordi = dispatch(&st, "users.save", json!({ "name": "Jordi", "role": "cashier", "password": "4321" })).unwrap();
        dispatch(&st, "logout", json!({})).unwrap();
        dispatch(&st, "login", json!({ "userId": jordi["id"], "password": "4321" })).unwrap();

        for cmd in ["users.list", "settings.save", "products.save", "reports.summary", "tickets.void", "backup.now", "printers.test"] {
            assert_eq!(dispatch(&st, cmd, json!({})).unwrap_err(), "E_FORBIDDEN", "{cmd}");
        }
        let preview = json!({ "sample": true, "settings": { "shopName": "X" } });
        assert_eq!(dispatch(&st, "sale.preview", preview).unwrap_err(), "E_FORBIDDEN");

        let t = dispatch(&st, "sale.finalize", sale()).unwrap();
        assert_eq!(t["ticket"]["userName"], "Jordi");
        assert!(dispatch(&st, "tickets.list", json!({})).is_ok());
        assert!(dispatch(&st, "tickets.print", json!({ "id": t["ticket"]["id"], "copy": true })).is_ok());
    }

    #[test]
    fn partial_settings_save_keeps_the_rest() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "settings.save", json!({ "printerName": "POS-80", "footer": "Hasta pronto" })).unwrap();
        let s = dispatch(&st, "settings.save", json!({})).unwrap();
        assert_eq!(s["shopName"], "Tienda");
        assert_eq!(s["printerName"], "POS-80");
        assert_eq!(s["footer"], "Hasta pronto");
    }

    #[test]
    fn setup_only_runs_once_and_keeps_one_admin() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let again = json!({ "shopName": "Otra", "adminName": "Eve", "password": "0000" });
        assert_eq!(dispatch(&st, "setup", again).unwrap_err(), "E_SETUP_DONE");
        let me = st.user().unwrap();
        let demote = json!({ "id": me.id, "name": "Ana", "role": "cashier" });
        assert_eq!(dispatch(&st, "users.save", demote).unwrap_err(), "E_LAST_ADMIN");
        assert_eq!(dispatch(&st, "products.list", json!({})).unwrap().as_array().unwrap().len(), 12);
    }
}
