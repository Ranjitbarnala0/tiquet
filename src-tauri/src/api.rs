// Single entry point for the UI. The Tauri shell and the dev HTTP server
// both call `dispatch`, so they exercise exactly the same code.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{fail, Error, Result};
use crate::escpos::{self, Opts};
use crate::layout::{self, TicketView};
use crate::settings::{self, Logo, Settings};
use crate::state::{AppState, SessionUser};
use crate::tickets::{self, PaymentIn, RefundIn, SaleLine, SaleOpts, ShopInfo, TicketRecord};
use crate::verifactu::{self, Env};
use crate::{aeat, auth, backup, cash, log, money, os, printer, products, recovery, reports, users};

// key of the recovery code's own attempt counter (no user has id 0)
const RECOVERY: i64 = 0;

// asked by the screen on its own every few seconds, not by a person: they do
// not keep an idle till unlocked
const PASSIVE: &[&str] =
    &["printers.status", "bootstrap", "log.client", "sale.status", "recovery.status", "backup.status", "verifactu.status"];

pub fn dispatch(st: &AppState, cmd: &str, args: Value) -> std::result::Result<Value, String> {
    // a bug in one handler must answer with an error, not take the till down
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| route(st, cmd, args)));
    if !PASSIVE.contains(&cmd) && st.current().is_some() {
        st.touch();
    }
    match out {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            if serious(&e.0) {
                log::error(format!("{cmd}: {}", e.0));
            }
            Err(e.0)
        }
        Err(p) => {
            let what = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            log::error(format!("{cmd} panicked: {what}"));
            Err("E_INTERNAL:panic".into())
        }
    }
}

// refusals the UI expects (bad input, wrong password, no permission) are
// not faults worth a log line
fn serious(code: &str) -> bool {
    !code.starts_with("E_") || ["E_PRINT", "E_DB", "E_IO", "E_INTERNAL"].iter().any(|c| code.starts_with(c))
}

pub fn client_log(a: &Value) {
    let msg = a.get("message").and_then(Value::as_str).unwrap_or_default();
    log::error(format!("ui: {msg}"));
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
        "recover" => recover(st, parse(a)?),
        "recovery.status" => {
            st.admin()?;
            recovery::status(&st.conn())
        }
        "recovery.new" => {
            let me = st.admin()?;
            let code = recovery::issue(&st.conn())?;
            log::info(format!("new recovery code made by {}", me.name));
            Ok(json!({ "code": code }))
        }
        "recovery.print" => {
            let me = st.admin()?;
            let code = a.get("code").and_then(Value::as_str).unwrap_or_default();
            let s = settings::load(&st.conn())?;
            let bytes = escpos::document(&layout::recovery_slip(&s, code, &me.name), None, &Opts::new(&s, false));
            let warning = printer::send(&st.sink, &s.printer_name, "recuperacion", &bytes)?;
            Ok(json!({ "warning": warning }))
        }
        "logout" => {
            st.set_session(None);
            Ok(Value::Null)
        }
        "log.client" => {
            client_log(&a);
            Ok(Value::Null)
        }

        "settings.get" => {
            st.user()?;
            to(settings::load(&st.conn())?)
        }
        "settings.save" => {
            let me = st.admin()?;
            log::info(format!("settings saved by {}", me.name));
            // keys the caller leaves out keep their saved value
            let conn = st.conn();
            let mut merged = serde_json::to_value(settings::load(&conn)?)?;
            if let (Some(cur), Some(new)) = (merged.as_object_mut(), a.as_object()) {
                for (k, v) in new {
                    cur.insert(k.clone(), v.clone());
                }
            }
            let saved = settings::save(&conn, parse(merged)?)?;
            st.set_lock_after(saved.auto_lock_min);
            to(saved)
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
        "products.recent" => {
            st.user()?;
            to(products::recent(&st.conn(), 24)?)
        }
        "products.delete" => {
            let me = st.admin()?;
            let deleted = products::delete(&st.conn(), id(&a)?)?;
            log::info(format!("product {} {} by {}", id(&a)?, if deleted { "deleted" } else { "archived" }, me.name));
            Ok(json!({ "deleted": deleted }))
        }
        "products.import" => {
            st.admin()?;
            let rows: Vec<products::ImportRow> = parse(a.get("rows").cloned().unwrap_or_default())?;
            let mut conn = st.conn();
            let vat = settings::load(&conn)?.default_vat_bp;
            to(products::import(&mut conn, rows, vat)?)
        }

        "sale.preview" => {
            st.user()?;
            preview(st, parse(a)?)
        }
        "sale.finalize" => {
            st.user()?;
            finalize(st, parse(a)?)
        }
        // a cart restored after a crash may already have been charged
        "sale.status" => {
            st.user()?;
            let sale_id = a.get("saleId").and_then(Value::as_str).unwrap_or_default();
            let conn = st.conn();
            let t = tickets::by_sale_id(&conn, sale_id)?.map(|id| tickets::load(&conn, id)).transpose()?;
            Ok(json!({ "ticket": t.map(|t| json!({ "id": t.id, "number": t.number, "totalCents": t.total_cents })) }))
        }

        "tickets.list" => {
            st.user()?;
            to(tickets::list(&st.conn(), &parse(a)?)?)
        }
        "tickets.get" => ticket_get(st, id(&a)?),
        // any user may reprint; the core decides whether it says COPIA
        "tickets.print" => {
            st.user()?;
            let (rec, s, logo) = {
                let conn = st.conn();
                let rec = tickets::load(&conn, id(&a)?)?;
                let logo = tickets::logo_for(&conn, &rec)?;
                (rec, settings::load(&conn)?, logo)
            };
            let warning = print_ticket(st, &rec, &s, logo.as_ref(), false)?;
            Ok(json!({ "warning": warning }))
        }
        "tickets.void" => {
            let me = st.admin()?;
            let reason = a.get("reason").and_then(Value::as_str).unwrap_or_default();
            tickets::void(&mut st.conn(), id(&a)?, &me.name, reason)?;
            aeat::kick();
            log::info(format!("ticket {} voided by {}: {reason}", id(&a)?, me.name));
            ticket_get(st, id(&a)?)
        }
        "tickets.refund" => {
            let me = st.admin()?;
            let r: RefundIn = parse(a.clone())?;
            if flag(&a, "preview", false) {
                return refund_preview(st, &me, &r);
            }
            let (rec, s, logo) = {
                let mut conn = st.conn();
                let s = settings::load(&conn)?;
                let rec = tickets::refund(&mut conn, &me, &s, &r)?;
                let logo = tickets::logo_for(&conn, &rec)?;
                (rec, s, logo)
            };
            st.note(&rec);
            aeat::kick();
            let of = rec.refund_of.as_ref().map(|t| t.number.as_str()).unwrap_or("?");
            log::info(format!(
                "refund {} of {of} {} {} by {}",
                rec.number,
                money::fmt(rec.total_cents),
                rec.payment.method,
                me.name
            ));
            Ok(issue(st, rec, &s, logo.as_ref(), flag(&a, "print", true)))
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
            let warning = printer::send(&st.sink, &s.printer_name, "resumen", &bytes)?;
            Ok(json!({ "warning": warning }))
        }
        "reports.export" => {
            st.admin()?;
            let vat_book = a.get("kind").and_then(Value::as_str) == Some("vat");
            let r: reports::Range = parse(a)?;
            let path = if vat_book { reports::export_vat_book(&st.conn(), &r)? } else { reports::export_csv(&st.conn(), &r)? };
            Ok(json!({ "path": path.to_string_lossy() }))
        }

        "users.list" => {
            st.admin()?;
            to(users::list(&st.conn(), false)?)
        }
        "users.save" => {
            let me = st.admin()?;
            let u: users::UserIn = parse(a)?;
            // the admin on duty cannot lock themselves out; another admin can
            if u.id == Some(me.id) && (!u.active || u.role != "admin") {
                return fail("E_SELF");
            }
            let row = users::save(&st.conn(), u)?;
            log::info(format!("user {} saved by {} (role {}, active {})", row.name, me.name, row.role, row.active));
            if row.id == me.id {
                st.set_session(Some(SessionUser { id: row.id, name: row.name.clone(), role: row.role.clone() }));
            }
            to(row)
        }

        "cash.state" => {
            st.user()?;
            let conn = st.conn();
            let open = cash::open_session(&conn)?;
            to(cash::drawer(&conn, open)?)
        }
        "cash.open" => {
            let me = st.user()?;
            let float = a.get("floatCents").and_then(Value::as_i64).unwrap_or(0);
            let s = {
                let conn = st.conn();
                cash::open(&conn, &me.name, float)?;
                settings::load(&conn)?
            };
            log::info(format!("till opened by {} with {}", me.name, money::fmt(float)));
            printer::send_detached(&st.sink, &s.printer_name, "cajon", &escpos::kick(&s));
            let conn = st.conn();
            let open = cash::open_session(&conn)?;
            to(cash::drawer(&conn, open)?)
        }
        "cash.move" => {
            let me = st.user()?;
            let cents = a.get("cents").and_then(Value::as_i64).unwrap_or(0);
            let reason = a.get("reason").and_then(Value::as_str).unwrap_or_default();
            let s = {
                let conn = st.conn();
                cash::add_move(&conn, &me.name, cents, reason)?;
                settings::load(&conn)?
            };
            log::info(format!("cash {} by {}: {reason}", money::fmt(cents), me.name));
            printer::send_detached(&st.sink, &s.printer_name, "cajon", &escpos::kick(&s));
            let conn = st.conn();
            let open = cash::open_session(&conn)?;
            to(cash::drawer(&conn, open)?)
        }
        "cash.close" => {
            let me = st.user()?;
            let counted = a.get("countedCents").and_then(Value::as_i64).unwrap_or(0);
            let note = a.get("note").and_then(Value::as_str).unwrap_or_default();
            let session = cash::close(&mut st.conn(), &me.name, counted, note)?;
            log::info(format!(
                "till closed by {}: counted {}, expected {}",
                me.name,
                money::fmt(counted),
                money::fmt(session.expected_cents.unwrap_or_default())
            ));
            let print_error = if flag(&a, "print", true) { print_close(st, session.id, &me.name).err().map(|e| e.0) } else { None };
            Ok(json!({ "session": session, "printError": print_error }))
        }
        "cash.print" => {
            let me = st.user()?;
            let warning = print_close(st, id(&a)?, &me.name)?;
            Ok(json!({ "warning": warning }))
        }
        "cash.history" => {
            st.admin()?;
            to(cash::history(&st.conn(), 60)?)
        }
        "training.wipe" => {
            let me = st.admin()?;
            let n = tickets::wipe_training(&mut st.conn())?;
            log::info(format!("{n} practice tickets wiped by {}", me.name));
            Ok(json!({ "deleted": n }))
        }

        "printers.list" => {
            st.admin()?;
            Ok(json!({ "printers": printer::list() }))
        }
        "printers.detect" => {
            st.admin()?;
            to(printer::detect(&printer::list()))
        }
        // once on a new PC: if nobody has chosen a printer yet and one is
        // clearly the receipt printer, use it
        "printers.auto" => {
            let me = st.admin()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            if !s.printer_name.trim().is_empty() || settings::get(&conn, "printerAutoDone")?.is_some() {
                return Ok(json!({ "applied": false }));
            }
            settings::put(&conn, "printerAutoDone", &tickets::now())?;
            let found = printer::detect(&printer::list());
            let Some(name) = found.best.clone() else {
                return Ok(json!({ "applied": false, "candidates": found.candidates }));
            };
            let saved = settings::save(&conn, Settings { printer_name: name.clone(), columns: found.columns, ..s })?;
            log::info(format!("receipt printer {name} chosen automatically for {}", me.name));
            Ok(json!({ "applied": true, "name": name, "settings": saved }))
        }
        "printers.status" => {
            st.user()?;
            let (name, last) = {
                let conn = st.conn();
                (settings::load(&conn)?.printer_name, tickets::last_print(&conn)?)
            };
            let status = printer::status(&st.sink, &name);
            Ok(json!({ "name": name, "state": status.state, "jobs": status.jobs, "last": last }))
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
                "defaultDir": backup::default_dir().to_string_lossy(),
                "last": settings::get(&conn, "lastBackupAt")?,
                "lastPath": settings::get(&conn, "lastBackupPath")?,
                "error": settings::get(&conn, "lastBackupError")?,
                "auto": s.auto_backup,
                "dataDir": st.data_dir.to_string_lossy(),
                "logPath": log::path().map(|p| p.to_string_lossy().into_owned()),
            }))
        }
        "backup.list" => {
            st.admin()?;
            let s = settings::load(&st.conn())?;
            to(backup::list(&backup_dirs(st, &s)))
        }
        "backup.restore" => {
            let me = st.admin()?;
            let path = PathBuf::from(a.get("path").and_then(Value::as_str).unwrap_or_default());
            let safety = {
                let mut conn = st.conn();
                let dirs = backup_dirs(st, &settings::load(&conn)?);
                let file = backup::allowed(&path, &dirs)?;
                backup::restore(&mut conn, &file, &st.data_dir)?
            };
            let missing = st.sync_marks()?;
            log::warn(format!("backup {} restored by {}", path.display(), me.name));
            // users and passwords may differ in the copy: everyone signs in again
            st.set_session(None);
            let missing: Vec<Value> = missing
                .iter()
                .map(|(series, from, to)| json!({ "first": format!("{series}-{from:06}"), "last": format!("{series}-{to:06}") }))
                .collect();
            Ok(json!({ "safetyCopy": safety.to_string_lossy(), "missing": missing }))
        }
        // VERI*FACTU: how the records are doing, the certificates this
        // Windows user has, a correction for a record the AEAT refused, and
        // the producer's declaration, which anyone may read
        "verifactu.status" => {
            st.user()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            let mut out = serde_json::to_value(verifactu::status(&conn, &s)?)?;
            out["clockDrift"] = json!(settings::get(&conn, "aeatClockDrift")?.and_then(|d| d.parse::<i64>().ok()));
            Ok(out)
        }
        "verifactu.certs" => {
            st.admin()?;
            to(aeat::certificates())
        }
        "verifactu.correct" => {
            let me = st.admin()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            let new = verifactu::correct(&conn, id(&a)?, &s)?;
            drop(conn);
            aeat::kick();
            log::info(format!("billing record {} corrected by {}", id(&a)?, me.name));
            Ok(json!({ "id": new }))
        }
        "verifactu.reach" => {
            st.admin()?;
            let env = a.get("env").and_then(Value::as_str).and_then(Env::parse).unwrap_or(Env::Prod);
            Ok(json!({ "status": aeat::reach(env)? }))
        }
        "verifactu.declaration" => {
            st.user()?;
            Ok(json!({ "text": verifactu::declaration() }))
        }

        "app.reveal" => {
            st.admin()?;
            let what = a.get("what").and_then(Value::as_str).unwrap_or_default();
            let s = settings::load(&st.conn())?;
            let dir = match what {
                "data" => st.data_dir.clone(),
                "backups" => backup::dir(&s),
                "exports" => backup::documents().join("Exportaciones"),
                _ => return fail("E_ARGS:what"),
            };
            fs::create_dir_all(&dir)?;
            // a bare file name inside that folder, never a path of its own
            let name = a.get("name").and_then(Value::as_str).filter(|n| Path::new(n).file_name().is_some_and(|f| f == *n));
            let path = name.map(|n| dir.join(n)).filter(|p| p.exists()).unwrap_or(dir);
            if st.desktop {
                os::reveal(&path);
            }
            Ok(json!({ "path": path.to_string_lossy() }))
        }

        _ => fail(format!("E_UNKNOWN:{cmd}")),
    }
}

// where backups may be listed and restored from: the configured folder, the
// default one, and the copies the app took itself before restores and upgrades
fn backup_dirs(st: &AppState, s: &Settings) -> Vec<PathBuf> {
    let mut dirs = vec![backup::dir(s), backup::default_dir(), st.data_dir.join("restauraciones"), st.data_dir.join("migraciones")];
    dirs.dedup();
    dirs
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
        "canRecover": recovery::exists(&conn)?,
        "installId": settings::install_id(&conn)?,
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
    // every ticket carries it, so it is asked for once, here
    if !crate::nif::spanish(&a.shop_nif) {
        return fail("E_INVALID:nif");
    }
    let s = Settings {
        shop_name: a.shop_name.trim().into(),
        shop_address: a.shop_address.trim().into(),
        shop_phone: a.shop_phone.trim().into(),
        shop_nif: crate::nif::normalize(&a.shop_nif),
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
    let code = recovery::issue(&tx)?;
    tx.commit()?;
    drop(conn);
    log::info(format!("first-run setup by {}", u.name));
    let me = SessionUser { id: u.id, name: u.name, role: u.role };
    st.set_session(Some(me.clone()));
    Ok(json!({ "user": me, "recoveryCode": code }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginIn {
    user_id: i64,
    password: String,
}

fn login(st: &AppState, a: LoginIn) -> Result<Value> {
    auth::check_lock(&st.conn(), a.user_id)?;
    let row = users::credentials(&st.conn(), a.user_id)?;
    let user = match row {
        Some((u, hash, true)) if auth::verify(&a.password, &hash) => u,
        _ => {
            let n = auth::missed(&st.conn(), a.user_id, 30)?;
            log::warn(format!("wrong password for user {} ({n} in a row)", a.user_id));
            return fail("E_AUTH");
        }
    };
    auth::cleared(&st.conn(), a.user_id)?;
    st.touch();
    log::info(format!("{} signed in", user.name));
    st.set_session(Some(user.clone()));
    backup::auto(&st.conn());
    Ok(json!({ "user": user }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecoverIn {
    code: String,
    user_id: i64,
    password: String,
}

// the code is spent on use: the answer carries the new one to write down
fn recover(st: &AppState, a: RecoverIn) -> Result<Value> {
    let mut conn = st.conn();
    auth::check_lock(&conn, RECOVERY)?;
    if !recovery::matches(&conn, &a.code)? {
        let n = auth::missed(&conn, RECOVERY, 300)?;
        log::warn(format!("wrong recovery code ({n} in a row)"));
        return fail("E_RECOVERY");
    }
    let user = match users::credentials(&conn, a.user_id)? {
        Some((u, _, true)) if u.role == "admin" => u,
        Some(_) => return fail("E_FORBIDDEN"),
        None => return fail("E_NOT_FOUND"),
    };
    let tx = conn.transaction()?;
    users::set_password(&tx, user.id, &a.password)?;
    let code = recovery::issue(&tx)?;
    auth::cleared(&tx, RECOVERY)?;
    auth::cleared(&tx, user.id)?;
    tx.commit()?;
    drop(conn);
    st.touch();
    log::warn(format!("password of {} reset with the recovery code", user.name));
    st.set_session(Some(user.clone()));
    Ok(json!({ "user": user, "recoveryCode": code }))
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
    #[serde(default)]
    customer: Option<tickets::Customer>,
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
    let customer = a.customer.filter(|c| !c.name.trim().is_empty());
    let tag = tickets::series_tag("sale", customer.is_some(), s.training);
    let (_, _, number) = tickets::next_number(&conn, &s.ticket_prefix, &tag, &issued_at)?;
    let shop = ShopInfo::from_settings(&s);
    let qr = verifactu::preview_qr(&s, &number, &issued_at, totals.total_cents);
    let v = TicketView {
        shop: &shop,
        kind: "sale",
        number: &number,
        issued_at: &issued_at,
        user_name: &me.name,
        lines: &lines,
        totals: &totals,
        payment: payment.as_ref(),
        refund_of: None,
        reason: None,
        show_vat: s.show_vat,
        copy: false,
        voided: false,
        customer: customer.as_ref(),
        training: s.training,
        qr: qr.as_deref(),
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
    #[serde(flatten)]
    opts: SaleOpts,
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

fn finalize(st: &AppState, a: FinalizeIn) -> Result<Value> {
    let me = st.user()?;
    let (rec, dup, s, logo) = {
        let mut conn = st.conn();
        let s = settings::load(&conn)?;
        let (rec, dup) = tickets::sell(&mut conn, &me, &s, a.lines, &a.payment, &a.opts)?;
        let logo = tickets::logo_for(&conn, &rec)?;
        (rec, dup, s, logo)
    };
    if dup {
        log::warn(format!("sale {} was already charged; not issued again", rec.number));
        return Ok(json!({ "ticket": rec, "printed": false, "printError": null, "printWarning": null, "duplicate": true }));
    }
    if !rec.training {
        st.note(&rec);
        aeat::kick();
    }
    log::info(format!(
        "sale {} {} {} by {}",
        rec.number,
        money::fmt(rec.total_cents),
        rec.payment.method,
        me.name
    ));
    let out = issue(st, rec, &s, logo.as_ref(), a.print);
    backup::auto(&st.conn());
    Ok(out)
}

// the ticket is committed before anything is sent to the printer: a jammed
// or unplugged printer never loses a ticket, it only reports printError
fn issue(st: &AppState, rec: TicketRecord, s: &Settings, logo: Option<&Logo>, print: bool) -> Value {
    let (print_error, print_warning) = if print {
        match print_ticket(st, &rec, s, logo, true) {
            Ok(w) => (None, w),
            Err(e) => (Some(e.0), None),
        }
    } else {
        if drawer_opens(s, &rec.payment.method) {
            printer::send_detached(&st.sink, &s.printer_name, "cajon", &escpos::kick(s));
        }
        (None, None)
    };
    if let Some(e) = print_error.as_ref().or(print_warning.as_ref()) {
        log::warn(format!("print {}: {e}", rec.number));
    }
    json!({
        "ticket": rec,
        "printed": print && print_error.is_none(),
        "printError": print_error,
        "printWarning": print_warning,
    })
}

fn refund_preview(st: &AppState, me: &SessionUser, r: &RefundIn) -> Result<Value> {
    let conn = st.conn();
    let s = settings::load(&conn)?;
    let plan = tickets::plan_refund(&conn, r)?;
    let issued_at = tickets::now();
    let tag = tickets::series_tag("refund", plan.original.customer.is_some(), plan.original.training);
    let (_, _, number) = tickets::next_number(&conn, &s.ticket_prefix, &tag, &issued_at)?;
    let shop = ShopInfo::from_settings(&s);
    let lines = plan.sale_lines();
    let qr = verifactu::preview_qr(&s, &number, &issued_at, plan.totals.total_cents).filter(|_| !plan.original.training);
    let v = TicketView {
        shop: &shop,
        kind: "refund",
        number: &number,
        issued_at: &issued_at,
        user_name: &me.name,
        lines: &lines,
        totals: &plan.totals,
        payment: Some(&plan.payment),
        refund_of: Some((&plan.original.number, &plan.original.issued_at)),
        reason: Some(&plan.reason),
        show_vat: s.show_vat,
        copy: false,
        voided: false,
        customer: plan.original.customer.as_ref(),
        training: plan.original.training,
        qr: qr.as_deref(),
    };
    let logo = settings::logo(&conn)?.is_some();
    Ok(json!({ "lines": layout::ticket(&v, &s, logo), "totals": plan.totals, "number": number }))
}

// The first print that may have reached the paper is the original; every
// later one, by anyone, says COPIA, and so do the extra copies set in Ajustes.
// The drawer opens only with the original printed at the sale itself.
fn print_ticket(st: &AppState, rec: &TicketRecord, s: &Settings, logo: Option<&Logo>, at_sale: bool) -> Result<Option<String>> {
    let (attempt, copy) = tickets::begin_print(&st.conn(), rec.id, false)?;
    let kick = at_sale && !copy && drawer_opens(s, &rec.payment.method);
    let mut data = escpos::document(&layout::record(rec, s, logo.is_some(), copy), logo, &Opts::new(s, kick));
    if !copy {
        let extra = layout::record(rec, s, logo.is_some(), true);
        for _ in 1..s.copies {
            data.extend(escpos::document(&extra, logo, &Opts::new(s, false)));
        }
    }
    let out = printer::send(&st.sink, &s.printer_name, &rec.number, &data);
    let (status, detail) = match &out {
        Ok(None) => ("printed", None),
        Ok(Some(w)) => ("sent", Some(w.as_str())),
        Err(e) if e.0 == "E_PRINT:timeout" || e.0.starts_with("E_PRINT:partial") => ("unknown", Some(e.0.as_str())),
        Err(e) => ("failed", Some(e.0.as_str())),
    };
    tickets::end_print(&st.conn(), attempt, status, detail)?;
    out
}

fn print_close(st: &AppState, id: i64, user: &str) -> Result<Option<String>> {
    let (lines, s) = {
        let conn = st.conn();
        let s = settings::load(&conn)?;
        let session = cash::get(&conn, id)?;
        let Some(closed_at) = session.closed_at.clone() else {
            return fail("E_CASH_OPEN");
        };
        let sum = reports::between(&conn, &session.opened_at, &closed_at)?;
        let drawer = cash::drawer(&conn, Some(session.clone()))?;
        (layout::cash_close(&s, &session, &drawer, &sum, user), s)
    };
    let bytes = escpos::document(&lines, None, &Opts::new(&s, false));
    printer::send(&st.sink, &s.printer_name, "cierre", &bytes)
}

fn ticket_get(st: &AppState, id: i64) -> Result<Value> {
    st.user()?;
    let conn = st.conn();
    let rec = tickets::load(&conn, id)?;
    let s = settings::load(&conn)?;
    let logo = tickets::logo_for(&conn, &rec)?;
    let lines = layout::record(&rec, &s, logo.is_some(), false);
    Ok(json!({ "ticket": rec, "lines": lines, "logo": logo }))
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
    let warning = printer::send(&st.sink, &s.printer_name, "prueba", &bytes)?;
    Ok(json!({ "warning": warning }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::PrintSink;

    use base64::Engine;

    fn state(sink: impl FnOnce(&std::path::Path) -> PrintSink) -> AppState {
        static DOCS: std::sync::Once = std::sync::Once::new();
        DOCS.call_once(|| std::env::set_var("TIQUET_DOCUMENTS", std::env::temp_dir().join("tiquet-test-documents")));
        // tests run in parallel and the clock can repeat: the counter keeps
        // every test in a folder of its own
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("tiquet-api-{}-{n}-{k}", std::process::id()));
        AppState::open(&dir, sink(&dir)).unwrap()
    }

    fn setup(st: &AppState) {
        let args = json!({ "shopName": "Tienda", "shopNif": "B12345674", "adminName": "Ana", "password": "123456", "samples": true, "lang": "es" });
        dispatch(st, "setup", args).unwrap();
        // the daily backup taken at login must land in the test's own folder
        let backups = st.data_dir.join("backups");
        let printer = if matches!(st.sink, PrintSink::Dir(_)) { "POS-80" } else { "" };
        dispatch(st, "settings.save", json!({ "backupDir": backups.to_string_lossy(), "printerName": printer })).unwrap();
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
        assert_eq!(st.data_dir.join("backups").read_dir().unwrap().count(), 1);

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
    fn refunds_are_admin_only_and_preview_matches_the_real_one() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let t = dispatch(&st, "sale.finalize", sale()).unwrap();
        let line = t["ticket"]["lines"][0]["id"].clone();
        let args = json!({ "id": t["ticket"]["id"], "lines": [{ "lineId": line, "qty": 1 }], "method": "cash", "reason": "roto" });

        let mut preview_args = args.clone();
        preview_args["preview"] = json!(true);
        let p = dispatch(&st, "tickets.refund", preview_args).unwrap();
        let r = dispatch(&st, "tickets.refund", args.clone()).unwrap();
        assert_eq!(p["number"], r["ticket"]["number"]);
        assert_eq!(p["totals"]["totalCents"], -690);
        assert_eq!(r["ticket"]["totalCents"], -690);
        assert_eq!(r["printed"], true);
        let got = dispatch(&st, "tickets.get", json!({ "id": r["ticket"]["id"] })).unwrap();
        assert_eq!(got["lines"], p["lines"]);
        assert_eq!(dispatch(&st, "tickets.refund", args.clone()).unwrap_err(), "E_REFUND_QTY");

        let jordi = dispatch(&st, "users.save", json!({ "name": "Jordi", "role": "cashier", "password": "4321" })).unwrap();
        dispatch(&st, "logout", json!({})).unwrap();
        dispatch(&st, "login", json!({ "userId": jordi["id"], "password": "4321" })).unwrap();
        assert_eq!(dispatch(&st, "tickets.refund", args).unwrap_err(), "E_FORBIDDEN");
    }

    #[test]
    fn a_forgotten_admin_password_is_reset_with_the_recovery_code() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        let args = json!({ "shopName": "Tienda", "shopNif": "B12345674", "adminName": "Ana", "password": "123456" });
        let code = dispatch(&st, "setup", args).unwrap()["recoveryCode"].as_str().unwrap().to_string();
        assert_eq!(dispatch(&st, "bootstrap", json!({})).unwrap()["canRecover"], true);
        let jordi = dispatch(&st, "users.save", json!({ "name": "Jordi", "role": "cashier", "password": "4321" })).unwrap();
        dispatch(&st, "logout", json!({})).unwrap();
        let ana = 1;

        let typed = code.to_lowercase().replace('-', " ");
        let cashier = json!({ "code": typed, "userId": jordi["id"], "password": "9999" });
        assert_eq!(dispatch(&st, "recover", cashier).unwrap_err(), "E_FORBIDDEN");
        let short = json!({ "code": typed, "userId": ana, "password": "1234" });
        assert_eq!(dispatch(&st, "recover", short).unwrap_err(), "E_INVALID:adminPassword");

        let ok = dispatch(&st, "recover", json!({ "code": typed, "userId": ana, "password": "567890" })).unwrap();
        assert_eq!(ok["user"]["name"], "Ana");
        let fresh = ok["recoveryCode"].as_str().unwrap().to_string();
        assert_ne!(fresh, code);
        dispatch(&st, "logout", json!({})).unwrap();
        assert_eq!(dispatch(&st, "login", json!({ "userId": ana, "password": "123456" })).unwrap_err(), "E_AUTH");
        dispatch(&st, "login", json!({ "userId": ana, "password": "567890" })).unwrap();
        dispatch(&st, "logout", json!({})).unwrap();

        // the spent code is dead, and guessing is cut off after five tries
        for _ in 0..5 {
            let used = json!({ "code": code, "userId": ana, "password": "000000" });
            assert_eq!(dispatch(&st, "recover", used).unwrap_err(), "E_RECOVERY");
        }
        let right = json!({ "code": fresh, "userId": ana, "password": "000000" });
        assert!(dispatch(&st, "recover", right).unwrap_err().starts_with("E_LOCKED:"));
    }

    #[test]
    fn a_retried_cart_is_not_charged_twice() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let mut cart = sale();
        cart["saleId"] = json!("7f3e2a10-cart");
        let first = dispatch(&st, "sale.finalize", cart.clone()).unwrap();
        assert_eq!(first["printed"], true);
        let again = dispatch(&st, "sale.finalize", cart).unwrap();
        assert_eq!(again["duplicate"], true);
        assert_eq!(again["ticket"]["number"], first["ticket"]["number"]);
        let status = dispatch(&st, "sale.status", json!({ "saleId": "7f3e2a10-cart" })).unwrap();
        assert_eq!(status["ticket"]["number"], first["ticket"]["number"]);
        assert_eq!(dispatch(&st, "sale.status", json!({ "saleId": "other" })).unwrap()["ticket"], Value::Null);
        assert_eq!(dispatch(&st, "tickets.list", json!({})).unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn restoring_an_old_backup_never_reuses_a_printed_number() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "sale.finalize", sale()).unwrap();
        let copy = dispatch(&st, "backup.now", json!({})).unwrap()["path"].as_str().unwrap().to_string();
        dispatch(&st, "sale.finalize", sale()).unwrap();
        let third = dispatch(&st, "sale.finalize", sale()).unwrap();
        let n3 = third["ticket"]["number"].as_str().unwrap().to_string();

        let list = dispatch(&st, "backup.list", json!({})).unwrap();
        assert!(list.as_array().unwrap().iter().any(|f| f["path"] == copy.as_str()));
        let outside = json!({ "path": st.data_dir.join("tiquet.db").to_string_lossy() });
        assert_eq!(dispatch(&st, "backup.restore", outside).unwrap_err(), "E_BACKUP_INVALID");

        let r = dispatch(&st, "backup.restore", json!({ "path": copy })).unwrap();
        assert_eq!(r["missing"][0]["last"], n3.as_str());
        assert_eq!(dispatch(&st, "tickets.list", json!({})).unwrap_err(), "E_SESSION");
        dispatch(&st, "login", json!({ "userId": 1, "password": "123456" })).unwrap();
        assert_eq!(dispatch(&st, "tickets.list", json!({})).unwrap().as_array().unwrap().len(), 1);
        let next = dispatch(&st, "sale.finalize", sale()).unwrap();
        let (series, seq) = n3.rsplit_once('-').unwrap();
        assert_eq!(next["ticket"]["number"], format!("{series}-{:06}", seq.parse::<i64>().unwrap() + 1).as_str());
        // the data from before the restore was kept and can itself be restored
        let safety = r["safetyCopy"].as_str().unwrap();
        let list = dispatch(&st, "backup.list", json!({})).unwrap();
        assert!(list.as_array().unwrap().iter().any(|f| f["path"] == safety));
    }

    #[test]
    fn reveal_stays_inside_the_app_folders() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let r = dispatch(&st, "app.reveal", json!({ "what": "data" })).unwrap();
        assert_eq!(r["path"], st.data_dir.to_string_lossy().as_ref());
        let sneaky = dispatch(&st, "app.reveal", json!({ "what": "backups", "name": "..\\..\\Windows" })).unwrap();
        assert!(!sneaky["path"].as_str().unwrap().contains("Windows"));
        assert_eq!(dispatch(&st, "app.reveal", json!({ "what": "C:\\" })).unwrap_err(), "E_ARGS:what");
    }

    #[test]
    fn a_ticket_prints_once_as_original_and_then_only_as_copies() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "settings.save", json!({ "copies": 2 })).unwrap();
        let t = dispatch(&st, "sale.finalize", sale()).unwrap();
        let id = t["ticket"]["id"].clone();
        dispatch(&st, "tickets.print", json!({ "id": id, "copy": false })).unwrap();
        let prints: Vec<String> = {
            let mut files: Vec<_> = std::fs::read_dir(st.data_dir.join("prints")).unwrap().map(|e| e.unwrap().path()).collect();
            files.sort();
            files.iter().map(|f| String::from_utf8_lossy(&std::fs::read(f).unwrap()).into_owned()).collect()
        };
        assert_eq!(prints.len(), 2);
        // the sale: original with the drawer kick, then the shop's copy
        assert_eq!(prints[0].matches("*** COPIA ***").count(), 1);
        assert!(prints[0].contains("\x1bp\0"));
        // asked for again later: a copy, and the drawer stays shut
        assert_eq!(prints[1].matches("*** COPIA ***").count(), 1);
        assert!(!prints[1].contains("\x1bp\0"));
        let got = dispatch(&st, "tickets.get", json!({ "id": id })).unwrap();
        assert_eq!(got["ticket"]["prints"].as_array().unwrap().len(), 2);
        assert_eq!(got["ticket"]["prints"][1]["copy"], true);
        let status = dispatch(&st, "printers.status", json!({})).unwrap();
        assert_eq!(status["last"]["status"], "printed");
    }

    #[test]
    fn a_reprint_shows_the_logo_the_ticket_was_issued_with() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let logo = |n: u8| {
            let bits = base64::engine::general_purpose::STANDARD.encode(vec![n; 8]);
            json!({ "w": 64, "h": 1, "bits": bits, "png": "data:image/png;base64,AAAA" })
        };
        dispatch(&st, "logo.set", logo(0xF0)).unwrap();
        let a = dispatch(&st, "sale.finalize", sale()).unwrap();
        dispatch(&st, "logo.set", logo(0x0F)).unwrap();
        let got = dispatch(&st, "tickets.get", json!({ "id": a["ticket"]["id"] })).unwrap();
        assert_eq!(got["logo"]["bits"], logo(0xF0)["bits"]);
        dispatch(&st, "logo.clear", json!({})).unwrap();
        let b = dispatch(&st, "sale.finalize", sale()).unwrap();
        assert_eq!(dispatch(&st, "tickets.get", json!({ "id": b["ticket"]["id"] })).unwrap()["logo"], Value::Null);
    }

    #[test]
    fn wrong_passwords_lock_even_across_a_restart() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("tiquet-lock-{}-{n}", std::process::id()));
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        setup(&st);
        dispatch(&st, "logout", json!({})).unwrap();
        for _ in 0..5 {
            assert_eq!(dispatch(&st, "login", json!({ "userId": 1, "password": "nope" })).unwrap_err(), "E_AUTH");
        }
        drop(st);
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        // even the right password waits out the lock
        let err = dispatch(&st, "login", json!({ "userId": 1, "password": "123456" })).unwrap_err();
        assert!(err.starts_with("E_LOCKED:"), "{err}");
    }

    #[test]
    fn an_unused_till_locks_itself_but_screen_polls_do_not_keep_it_open() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        st.set_lock_after(1);
        // pretend the last real action was two minutes ago
        dispatch(&st, "printers.status", json!({})).unwrap();
        *st.active_for_test() = std::time::Instant::now() - std::time::Duration::from_secs(120);
        assert_eq!(dispatch(&st, "printers.status", json!({})).unwrap_err(), "E_SESSION_IDLE");
        assert_eq!(dispatch(&st, "tickets.list", json!({})).unwrap_err(), "E_SESSION");
        dispatch(&st, "login", json!({ "userId": 1, "password": "123456" })).unwrap();
        assert!(dispatch(&st, "tickets.list", json!({})).is_ok());
        assert_eq!(
            dispatch(&st, "users.save", json!({ "name": "Eva", "role": "admin", "password": "1234" })).unwrap_err(),
            "E_INVALID:adminPassword"
        );
    }

    #[test]
    fn closing_the_till_prints_a_z_report() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "cash.open", json!({ "floatCents": 5000 })).unwrap();
        dispatch(&st, "sale.finalize", sale()).unwrap();
        let d = dispatch(&st, "cash.move", json!({ "cents": -1000, "reason": "Compra de bolsas" })).unwrap();
        assert_eq!(d["expectedCents"], 5000 + 690 - 1000);
        let r = dispatch(&st, "cash.close", json!({ "countedCents": 4690 })).unwrap();
        assert_eq!(r["printError"], Value::Null);
        let z: Vec<_> = std::fs::read_dir(st.data_dir.join("prints"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("cierre"))
            .collect();
        let text = String::from_utf8_lossy(&std::fs::read(z[0].path()).unwrap()).into_owned();
        for want in ["CIERRE DE CAJA", "Compra de bolsas", "Cuadra"] {
            assert!(text.contains(want), "{want} in {text}");
        }
        assert_eq!(dispatch(&st, "cash.state", json!({})).unwrap()["session"], Value::Null);
    }

    #[test]
    fn products_never_sold_are_deleted_and_sold_ones_archived() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let line = json!({ "productId": 1, "name": "Cable", "qty": 1, "unitCents": 690, "vatBp": 2100 });
        dispatch(&st, "sale.finalize", json!({ "lines": [line], "payment": { "method": "cash" } })).unwrap();
        dispatch(&st, "sale.finalize", json!({ "lines": [{ "name": "Pegatina", "qty": 2, "unitCents": 150, "vatBp": 2100 }], "payment": { "method": "card" } })).unwrap();
        let recent = dispatch(&st, "products.recent", json!({})).unwrap();
        assert_eq!(recent[0]["name"], "Pegatina");
        assert_eq!(recent[1]["productId"], 1);
        assert_eq!(dispatch(&st, "products.delete", json!({ "id": 1 })).unwrap()["deleted"], false);
        assert_eq!(dispatch(&st, "products.delete", json!({ "id": 2 })).unwrap()["deleted"], true);
        let all = dispatch(&st, "products.list", json!({ "all": true })).unwrap();
        assert!(all.as_array().unwrap().iter().all(|p| p["id"] != 2));
        assert!(all.as_array().unwrap().iter().any(|p| p["id"] == 1 && p["active"] == false));
        // the archived product no longer offers itself from the recent list
        assert_eq!(dispatch(&st, "products.recent", json!({})).unwrap()[1]["productId"], Value::Null);
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
        let again = json!({ "shopName": "Otra", "shopNif": "B12345674", "adminName": "Eve", "password": "000000" });
        assert_eq!(dispatch(&st, "setup", again).unwrap_err(), "E_SETUP_DONE");
        let me = st.user().unwrap();
        let demote = json!({ "id": me.id, "name": "Ana", "role": "cashier" });
        assert_eq!(dispatch(&st, "users.save", demote).unwrap_err(), "E_SELF");
        let off = json!({ "id": me.id, "name": "Ana", "role": "admin", "active": false });
        assert_eq!(dispatch(&st, "users.save", off).unwrap_err(), "E_SELF");
        assert_eq!(dispatch(&st, "products.list", json!({})).unwrap().as_array().unwrap().len(), 12);
    }
}
