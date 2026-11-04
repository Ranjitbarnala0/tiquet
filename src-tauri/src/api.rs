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
use crate::printer::Job;
use crate::settings::{self, Logo, Settings};
use crate::state::{AppState, SessionUser};
use crate::tickets::{self, PaymentIn, RefundIn, SaleLine, SaleOpts, ShopInfo, TicketRecord};
use crate::verifactu::{self, Env};
use crate::{aeat, auth, backup, book, cash, diag, log, mail, money, os, printer, products, recovery, repairs, reports, share, users};

// who was signed in as Tiquet closed to update itself
const RESUME: &str = "resumeUser";
// how long that lasts: more than this and it was no update
const RESUME_SECS: i64 = 300;

// the screen shows the newest of a long period; the totals count them all
const DAY_ROWS: usize = 1500;

// key of the recovery code's own attempt counter (no user has id 0)
const RECOVERY: i64 = 0;

// asked by the screen on its own every few seconds, not by a person: they do
// not keep an idle till unlocked
const PASSIVE: &[&str] = &[
    "printers.status",
    "printers.auto",
    "bootstrap",
    "log.client",
    "sale.status",
    "recovery.status",
    "backup.status",
    "verifactu.status",
    "cash.state",
];

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
            let job = Job::doc(&layout::recovery_slip(&s, code, &me.name), None, &Opts::new(&s, false));
            let warning = printer::send_job(&st.sink, &s, "recuperacion", &job)?;
            Ok(json!({ "warning": warning }))
        }
        "logout" => {
            st.set_session(None);
            settings::clear(&st.conn(), RESUME)?;
            Ok(Value::Null)
        }
        // Tiquet is closing to update itself and opens again by itself: whoever
        // is signed in carries on, with no password to type
        "session.keep" => {
            let me = st.user()?;
            settings::put(&st.conn(), RESUME, &json!({ "id": me.id, "at": tickets::now() }).to_string())?;
            Ok(Value::Null)
        }
        // the screen says someone is using it (typing, clicking, scrolling):
        // a till in use never locks itself
        "session.ping" => {
            st.user()?;
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
            let old = settings::load(&st.conn())?;
            let mut merged = serde_json::to_value(&old)?;
            if let (Some(cur), Some(new)) = (merged.as_object_mut(), a.as_object()) {
                for (k, v) in new {
                    cur.insert(k.clone(), v.clone());
                }
            }
            let mut new: Settings = parse(merged)?;
            // a width given without Automático is the shop's own choice
            if a.get("columns").is_some() && a.get("paperAuto").is_none() {
                new.paper_auto = false;
            }
            // asked of the printer before the database is held
            if new.paper_auto {
                new.columns = printer::auto_columns(&st.sink, &new).unwrap_or(new.columns);
            }
            let conn = st.conn();
            let saved = settings::save(&conn, new)?;
            st.set_lock_after(saved.auto_lock_min);
            // "None" chosen on purpose: no printer is then picked on its own
            if saved.printer_name.trim().is_empty() && !old.printer_name.trim().is_empty() {
                settings::put(&conn, "printerOff", "1")?;
            } else if !saved.printer_name.trim().is_empty() {
                conn.execute("DELETE FROM settings WHERE key = 'printerOff'", [])?;
            }
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
        // the ticket for the customer's phone or inbox, as text and as PDF
        "tickets.text" => {
            st.user()?;
            let conn = st.conn();
            let rec = tickets::load(&conn, id(&a)?)?;
            Ok(json!({ "text": share::text(&rec, &settings::load(&conn)?) }))
        }
        "tickets.pdf" => {
            st.user()?;
            let path = {
                let conn = st.conn();
                let rec = tickets::load(&conn, id(&a)?)?;
                let logo = tickets::logo_for(&conn, &rec)?;
                share::save_pdf(&rec, &settings::load(&conn)?, logo.as_ref())?
            };
            let copied = flag(&a, "copy", false) && st.desktop && os::copy_file(&path);
            if flag(&a, "reveal", false) && st.desktop {
                os::reveal(&path);
            }
            Ok(json!({ "path": path.to_string_lossy(), "copied": copied }))
        }
        "tickets.whatsapp" => {
            st.user()?;
            let phone = share::phone(a.get("phone").and_then(Value::as_str).unwrap_or_default()).ok_or_else(|| Error("E_INVALID:phone".into()))?;
            let (number, text) = {
                let conn = st.conn();
                let rec = tickets::load(&conn, id(&a)?)?;
                (rec.number.clone(), share::text(&rec, &settings::load(&conn)?))
            };
            let out = whatsapp(st, &phone, &share::whatsapp_ticket(&text), flag(&a, "web", false));
            log::info(format!("ticket {number} sent by WhatsApp"));
            Ok(out)
        }
        // with no mail account set up, the mail app this PC has opens with
        // the PDF on the clipboard to paste in
        "tickets.email" => {
            let me = st.user()?;
            let to = a.get("to").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            if !mail::valid(&to) {
                return fail("E_INVALID:email");
            }
            let (account, rec, s, logo) = {
                let conn = st.conn();
                let rec = tickets::load(&conn, id(&a)?)?;
                let logo = tickets::logo_for(&conn, &rec)?;
                (mail::account(&conn), rec, settings::load(&conn)?, logo)
            };
            match account {
                Ok(acc) => {
                    let pdf = share::pdf(&rec, &s, logo.as_ref());
                    acc.send(&rec.shop.name, &to, &share::subject(&rec), &share::email_body(&rec), Some((share::file_name(&rec.number), pdf)))?;
                    log::info(format!("ticket {} emailed by {}", rec.number, me.name));
                    Ok(json!({ "sent": true }))
                }
                Err(e) if e.0 == "E_MAIL_SETUP" => {
                    let path = share::save_pdf(&rec, &s, logo.as_ref())?;
                    let url = share::mailto_url(&to, &share::subject(&rec), &share::email_body(&rec));
                    let opened = st.desktop && os::handles("mailto") && os::open(&url);
                    let copied = st.desktop && os::copy_file(&path);
                    Ok(json!({ "sent": false, "opened": opened, "copied": copied, "url": url, "path": path.to_string_lossy() }))
                }
                Err(e) => Err(e),
            }
        }
        // Deleting a sale voids it: it stays on record with who and when, and
        // leaves every list and total. A cashier deletes what they sold today,
        // when Ajustes allows it; the owner any sale, or today's alone once
        // VERI*FACTU is on.
        "tickets.void" => {
            let me = st.user()?;
            let t = id(&a)?;
            let reason = match a.get("reason").and_then(Value::as_str).map(str::trim) {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => format!("Borrado por {}", me.name),
            };
            {
                let mut conn = st.conn();
                let s = settings::load(&conn)?;
                may_fix(&conn, &me, &s, t)?;
                tickets::void_on(&mut conn, t, &me.name, &reason, me.role == "admin" && verifactu::Env::of(&s).is_none())?;
            }
            aeat::kick();
            log::info(format!("ticket {t} voided by {}: {reason}", me.name));
            ticket_get(st, t)
        }
        // charged the wrong way: put right on the ticket itself, which keeps
        // who changed it and when
        "tickets.payment" => {
            let me = st.user()?;
            let t = id(&a)?;
            let method = a.get("method").and_then(Value::as_str).unwrap_or_default();
            {
                let mut conn = st.conn();
                let s = settings::load(&conn)?;
                may_fix(&conn, &me, &s, t)?;
                tickets::set_payment(&mut conn, t, method, &me.name)?;
            }
            log::info(format!("ticket {t} now paid by {method}, changed by {}", me.name));
            ticket_get(st, t)
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

        // cashiers take phones in, change their status, print and hand them
        // back; only an admin cancels one
        "repairs.list" => {
            st.user()?;
            to(repairs::list(&st.conn(), &parse(a)?)?)
        }
        "repairs.get" => repair_get(st, id(&a)?),
        "repairs.preview" => {
            let me = st.user()?;
            repair_preview(st, &me, parse(a)?)
        }
        "repairs.create" => {
            let me = st.user()?;
            repair_create(st, &me, parse(a)?)
        }
        "repairs.update" => {
            let me = st.user()?;
            let r: repairs::RepairIn = parse(a.clone())?;
            repairs::update(&mut st.conn(), id(&a)?, &me.name, &r)?;
            repair_get(st, id(&a)?)
        }
        "repairs.status" => {
            let me = st.user()?;
            let status = a.get("status").and_then(Value::as_str).unwrap_or_default();
            let r = repairs::set_status(&mut st.conn(), id(&a)?, &me.name, status)?;
            log::info(format!("repair {} now {status}, by {}", r.number, me.name));
            repair_get(st, id(&a)?)
        }
        "repairs.print" => {
            st.user()?;
            let what = a.get("what").and_then(Value::as_str).unwrap_or("both");
            if !["resguardo", "slip", "both", "delivery"].contains(&what) {
                return fail("E_ARGS:what");
            }
            let (r, s, logo) = {
                let conn = st.conn();
                let r = repairs::load(&conn, id(&a)?)?;
                let logo = tickets::shop_logo(&conn, &r.shop)?;
                (r, settings::load(&conn)?, logo)
            };
            let warning = print_repair(st, &r, &s, logo.as_ref(), what)?;
            Ok(json!({ "warning": warning }))
        }
        "repairs.deliver" => {
            let me = st.user()?;
            let r = repairs::deliver(&mut st.conn(), id(&a)?, &me.name)?;
            log::info(format!("repair {} handed back by {} with nothing left to pay", r.number, me.name));
            repair_get(st, id(&a)?)
        }
        "repairs.cancel" => {
            let me = st.admin()?;
            let reason = a.get("reason").and_then(Value::as_str).unwrap_or_default();
            let r = repairs::cancel(&mut st.conn(), id(&a)?, &me.name, reason)?;
            log::info(format!("repair {} cancelled by {}: {reason}", r.number, me.name));
            repair_get(st, id(&a)?)
        }

        "share.whatsapp" => {
            st.user()?;
            let phone = share::phone(a.get("phone").and_then(Value::as_str).unwrap_or_default()).ok_or_else(|| Error("E_INVALID:phone".into()))?;
            let text = a.get("text").and_then(Value::as_str).unwrap_or_default();
            if text.trim().is_empty() || text.chars().count() > 4000 {
                return fail("E_ARGS:text");
            }
            Ok(whatsapp(st, &phone, text, flag(&a, "web", false)))
        }

        // the shop's own mail account, for tickets sent straight from here
        // whether tickets go straight from Tiquet or through the mail app
        "email.status" => {
            st.user()?;
            Ok(json!({ "ready": mail::load(&st.conn())?.0.ready() }))
        }
        "email.get" => {
            st.admin()?;
            let (config, has_password) = mail::load(&st.conn())?;
            Ok(json!({ "config": config, "hasPassword": has_password }))
        }
        "email.save" => {
            let me = st.admin()?;
            let cfg: mail::Config = parse(a.get("config").cloned().unwrap_or_default())?;
            let conn = st.conn();
            let config = mail::save(&conn, cfg, a.get("password").and_then(Value::as_str))?;
            log::info(format!("mail account saved by {}", me.name));
            Ok(json!({ "config": config, "hasPassword": mail::load(&conn)?.1 }))
        }
        "email.test" => {
            st.admin()?;
            let to = a.get("to").and_then(Value::as_str).unwrap_or_default().trim().to_string();
            if !mail::valid(&to) {
                return fail("E_INVALID:email");
            }
            let (acc, shop) = {
                let conn = st.conn();
                (mail::account(&conn)?, settings::load(&conn)?.shop_name)
            };
            let body = "Este es un correo de prueba enviado desde Tiquet.\n\nSi lo recibes, los tickets ya se pueden enviar por email.";
            acc.send(&shop, &to, "Prueba de Tiquet", body, None)?;
            Ok(json!({ "sent": true }))
        }

        "reports.summary" => {
            st.admin()?;
            let r: reports::Range = parse(a)?;
            let conn = st.conn();
            let mut out = to(reports::summary(&conn, &r)?)?;
            out["day"] = to(book::summary(&book::rows(&conn, &r, false, false)?))?;
            Ok(out)
        }
        "reports.print" => {
            let me = st.admin()?;
            let r: reports::Range = parse(a)?;
            let (lines, s) = {
                let conn = st.conn();
                let s = settings::load(&conn)?;
                let sum = reports::summary(&conn, &r)?;
                let day = book::summary(&book::rows(&conn, &r, false, false)?);
                (layout::summary(&sum, &day, &r.from, &r.to, &s, &me.name), s)
            };
            let job = Job::doc(&lines, None, &Opts::new(&s, false));
            let warning = printer::send_job(&st.sink, &s, "resumen", &job)?;
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

        // recharges and expenses, one line each. Either way money moves, so the
        // drawer opens like it does for a sale.
        "book.add" => {
            let me = st.user()?;
            let (entry, duplicate, s) = {
                let conn = st.conn();
                let s = settings::load(&conn)?;
                let (entry, duplicate) = book::add(&conn, &me, &s, parse(a)?)?;
                (entry, duplicate, s)
            };
            if !duplicate {
                log::info(format!("book: {} {} {} by {}", entry.kind, entry.concept, money::fmt(entry.cents), me.name));
                if drawer_opens(&s, &entry.method) {
                    printer::send_detached(&st.sink, &s, "cajon", &escpos::kick(&s));
                }
            }
            Ok(json!({ "entry": entry, "duplicate": duplicate }))
        }
        "book.edit" => {
            let me = st.user()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            let e = book::edit(&conn, &me, &s, id(&a)?, parse(a)?)?;
            log::info(format!("book: {} {} corrected by {}", e.concept, money::fmt(e.cents), me.name));
            to(e)
        }
        // a receipt for a recharge, when the customer asks for one
        "book.print" => {
            st.user()?;
            let (lines, s) = {
                let conn = st.conn();
                let s = settings::load(&conn)?;
                (layout::recharge_slip(&book::get(&conn, id(&a)?)?, &s), s)
            };
            let warning = printer::send_job(&st.sink, &s, "recarga", &Job::doc(&lines, None, &Opts::new(&s, false)))?;
            Ok(json!({ "warning": warning }))
        }
        // the day: sales, recharges and expenses with their totals. A cashier
        // sees today, the owner any period and what was deleted.
        "day.list" => {
            let me = st.user()?;
            let admin = me.role == "admin";
            let mut range: reports::Range = parse(a.clone())?;
            if !admin {
                let today = tickets::now()[..10].to_string();
                range = reports::Range { from: today.clone(), to: today };
            }
            let conn = st.conn();
            let s = settings::load(&conn)?;
            let mut rows = book::rows(&conn, &range, s.training, admin && flag(&a, "deleted", false))?;
            let summary = book::summary(&rows);
            let hidden = !admin && !s.staff_totals;
            let capped = rows.len() > DAY_ROWS;
            rows.truncate(DAY_ROWS);
            Ok(json!({
                "rows": rows,
                "summary": if hidden { Value::Null } else { to(summary)? },
                "from": range.from,
                "to": range.to,
                "capped": capped,
                "staffDelete": s.book_staff_delete,
                "anyDay": admin && verifactu::Env::of(&s).is_none(),
            }))
        }
        // the figures under the main page, after every sale
        "day.totals" => {
            let me = st.user()?;
            let conn = st.conn();
            let s = settings::load(&conn)?;
            if me.role != "admin" && !s.staff_totals {
                return Ok(Value::Null);
            }
            let today = tickets::now()[..10].to_string();
            let range = reports::Range { from: today.clone(), to: today };
            to(book::summary(&book::rows(&conn, &range, s.training, false)?))
        }
        // A whole day, or only its sales, recharges or expenses, out of every
        // list and total, for the owner. Nothing is erased: each ticket is
        // voided and each entry marked, with who and why.
        "day.delete" => {
            let me = st.admin()?;
            let date = a.get("date").and_then(Value::as_str).unwrap_or_default().to_string();
            reports::Range { from: date.clone(), to: date.clone() }.bounds()?;
            let what = a.get("what").and_then(Value::as_str).unwrap_or("all").to_string();
            if !["all", "sales", "recharges", "expenses"].contains(&what.as_str()) {
                return fail("E_INVALID:kind");
            }
            let reason = match a.get("reason").and_then(Value::as_str).map(str::trim) {
                Some(r) if !r.is_empty() => r.to_string(),
                _ => format!("Día borrado por {}", me.name),
            };
            let gone = {
                let mut conn = st.conn();
                let s = settings::load(&conn)?;
                tickets::void_day(&mut conn, &me.name, &date, s.training, &reason, verifactu::Env::of(&s).is_none(), &what)?
            };
            aeat::kick();
            log::warn(format!("day {date} ({what}) deleted by {}: {} tickets, {} entries, {reason}", me.name, gone.tickets, gone.entries));
            to(gone)
        }
        "book.delete" => {
            let me = st.user()?;
            let conn = st.conn();
            let staff = settings::load(&conn)?.book_staff_delete;
            let reason = a.get("reason").and_then(Value::as_str).unwrap_or_default();
            let e = book::delete(&conn, &me, staff, id(&a)?, reason)?;
            log::info(format!("book: {} {} deleted by {}", e.concept, money::fmt(e.cents), me.name));
            to(e)
        }
        "book.restore" => {
            let me = st.user()?;
            let conn = st.conn();
            let staff = settings::load(&conn)?.book_staff_delete;
            to(book::restore(&conn, &me, staff, id(&a)?)?)
        }
        "book.quick" => {
            st.user()?;
            let conn = st.conn();
            let training = settings::load(&conn)?.training;
            to(book::quick(&conn, training)?)
        }
        "book.export" => {
            st.admin()?;
            let r: reports::Range = parse(a)?;
            let path = book::export(&st.conn(), &r)?;
            Ok(json!({ "path": path.to_string_lossy() }))
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
            printer::send_detached(&st.sink, &s, "cajon", &escpos::kick(&s));
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
            printer::send_detached(&st.sink, &s, "cajon", &escpos::kick(&s));
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
            book::wipe_training(&st.conn())?;
            log::info(format!("{n} practice tickets wiped by {}", me.name));
            Ok(json!({ "deleted": n }))
        }

        "printers.list" => {
            st.admin()?;
            Ok(json!({ "printers": printer::list(), "ports": printer::ports() }))
        }
        "printers.detect" => {
            st.admin()?;
            to(printer::detect(&printer::list()))
        }
        // A receipt printer set up on its own, asked every few seconds by the
        // screen: on a new PC, when one is plugged in later, or when Windows
        // renamed its queue (another USB port makes "POS-80 (Copia 1)"). Only
        // a clear receipt printer is taken, never over a network or COM
        // printer, nor when someone chose "None" on purpose.
        "printers.auto" => {
            let me = st.user()?;
            let (s, off) = {
                let conn = st.conn();
                (settings::load(&conn)?, settings::get(&conn, "printerOff")?.is_some())
            };
            let name = s.printer_name.trim().to_string();
            let kind_ok = ["windows", "document"].contains(&s.printer_kind.as_str());
            if !kind_ok || (name.is_empty() && off) {
                return Ok(json!({ "applied": false }));
            }
            if !name.is_empty() && printer::status(&st.sink, &s).state != "missing" {
                return paper_check(st, &me.name, s);
            }
            let found = printer::detect(&printer::list());
            let Some(best) = found.best.clone().filter(|b| *b != name) else {
                return Ok(json!({ "applied": false, "candidates": found.candidates }));
            };
            // the paper its driver or its name says, else 58 mm; a width someone chose stays
            let columns = if s.paper_auto { found.columns } else { s.columns };
            let saved = settings::save(&st.conn(), Settings { printer_name: best.clone(), printer_kind: "windows".into(), columns, ..s })?;
            st.set_paper_read();
            log::info(format!("receipt printer {best} chosen automatically for {}, {columns} columns (was {name:?})", me.name));
            Ok(json!({ "applied": true, "name": best, "columns": columns, "replaced": Some(name).filter(|n| !n.is_empty()), "settings": saved }))
        }
        "printers.status" => {
            st.user()?;
            let (s, last) = {
                let conn = st.conn();
                (settings::load(&conn)?, tickets::last_print(&conn)?)
            };
            let status = printer::status(&st.sink, &s);
            Ok(json!({ "name": printer::label(&s), "state": status.state, "jobs": status.jobs, "last": last }))
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

        // a zip for whoever gives support; Explorer opens on it
        "diagnostics.save" => {
            let me = st.admin()?;
            let path = diag::save(st)?;
            log::info(format!("diagnostics saved to {} by {}", path.display(), me.name));
            if st.desktop {
                os::reveal(&path);
            }
            Ok(json!({ "path": path.to_string_lossy() }))
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

// WhatsApp opens on that chat with the message written, ready to send: the
// app when this PC has it, WhatsApp Web otherwise
fn whatsapp(st: &AppState, phone: &str, message: &str, web: bool) -> Value {
    let app = !web && os::handles("whatsapp");
    let url = share::whatsapp_url(phone, message, app);
    let opened = st.desktop && os::open(&url);
    json!({ "url": url, "opened": opened, "app": app })
}

fn bootstrap(st: &AppState) -> Result<Value> {
    let conn = st.conn();
    let s = settings::load(&conn)?;
    // Ajustes can sign someone in as Tiquet opens: on its first screen only,
    // never again after they lock the till or sign out
    let first = st.first_open();
    if first && st.current().is_none() {
        let resumed = resume(&conn)?;
        let open_as = resumed.or((s.auto_login > 0).then_some(s.auto_login));
        if let Some((u, _, true)) = open_as.map(|id| users::credentials(&conn, id)).transpose()?.flatten() {
            log::info(format!("{} signed in as Tiquet opened", u.name));
            st.touch();
            st.set_session(Some(u));
            backup::auto(&conn);
        }
    }
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
        "autoUpdate": s.auto_update,
    }))
}

// who to sign back in after an update, if Tiquet closed for one moments ago
fn resume(conn: &rusqlite::Connection) -> Result<Option<i64>> {
    let Some(stored) = settings::get(conn, RESUME)? else {
        return Ok(None);
    };
    settings::clear(conn, RESUME)?;
    let v: Value = serde_json::from_str(&stored).unwrap_or(Value::Null);
    let at = v["at"].as_str().and_then(|t| chrono::NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S").ok());
    let fresh = at.is_some_and(|at| (0..=RESUME_SECS).contains(&(chrono::Local::now().naive_local() - at).num_seconds()));
    Ok(v["id"].as_i64().filter(|_| fresh))
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
    // optional: a shop that only wants simple tickets may leave it out
    if !a.shop_nif.trim().is_empty() && !crate::nif::spanish(&a.shop_nif) {
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
    let mut lines = if a.sample { tickets::sample_lines(s.default_vat_bp) } else { tickets::clamp(a.lines) };
    repairs::canon(&conn, &mut lines)?;
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

// A cashier corrects or deletes what they sold today, when Ajustes allows it;
// the owner anything.
fn may_fix(conn: &rusqlite::Connection, me: &SessionUser, s: &Settings, ticket: i64) -> Result<()> {
    if me.role == "admin" {
        return Ok(());
    }
    let t = tickets::load(conn, ticket)?;
    if s.book_staff_delete && t.user_name == me.name && t.issued_at.get(..10) == tickets::now().get(..10) {
        Ok(())
    } else {
        fail("E_FORBIDDEN")
    }
}

fn finalize(st: &AppState, a: FinalizeIn) -> Result<Value> {
    let me = st.user()?;
    let (rec, dup, s, logo) = {
        let mut conn = st.conn();
        let s = settings::load(&conn)?;
        if let Some(old) = a.opts.replaces {
            may_fix(&conn, &me, &s, old)?;
        }
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
    // the day's copy for a till left open overnight runs beside the till:
    // the first sale of the day never waits for it
    if backup::due(&st.conn()) {
        backup::later(st.data_dir.join("tiquet.db"));
    }
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
            printer::send_detached(&st.sink, s, "cajon", &escpos::kick(s));
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
    let mut job = Job::doc(&layout::record(rec, s, logo.is_some(), copy), logo, &Opts::new(s, kick));
    if !copy {
        let extra = layout::record(rec, s, logo.is_some(), true);
        for _ in 1..s.copies {
            job.push(&extra, logo, &Opts::new(s, false));
        }
    }
    let out = printer::send_job(&st.sink, s, &rec.number, &job);
    let (status, detail) = match &out {
        Ok(None) => ("printed", None),
        Ok(Some(w)) => ("sent", Some(w.as_str())),
        Err(e) if e.0 == "E_PRINT:timeout" || e.0.starts_with("E_PRINT:partial") => ("unknown", Some(e.0.as_str())),
        Err(e) => ("failed", Some(e.0.as_str())),
    };
    tickets::end_print(&st.conn(), attempt, status, detail)?;
    out
}

// Papel automático for the printer already set up, once a run: an install
// from before it (48 columns on a 58 mm roll), or a driver set up for
// another roll since. When Windows does not answer it is asked again later.
fn paper_check(st: &AppState, user: &str, s: Settings) -> Result<Value> {
    if !s.paper_auto || st.paper_read() {
        return Ok(json!({ "applied": false }));
    }
    let Some(columns) = printer::auto_columns(&st.sink, &s) else {
        return Ok(json!({ "applied": false }));
    };
    st.set_paper_read();
    if columns == s.columns {
        return Ok(json!({ "applied": false }));
    }
    let (name, was) = (s.printer_name.clone(), s.columns);
    let saved = settings::save(&st.conn(), Settings { columns, ..s })?;
    log::info(format!("paper of {name} read for {user}: {columns} columns, was {was}"));
    Ok(json!({ "applied": true, "paper": true, "name": name, "columns": columns, "settings": saved }))
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
    let job = Job::doc(&lines, None, &Opts::new(&s, false));
    printer::send_job(&st.sink, &s, "cierre", &job)
}

// the resguardo goes first, then the slip for the phone
fn print_repair(st: &AppState, r: &repairs::Repair, s: &Settings, logo: Option<&Logo>, what: &str) -> Result<Option<String>> {
    let mut job = Job::default();
    if what == "delivery" {
        job.push(&layout::repair_delivery(r, s, logo.is_some()), logo, &Opts::new(s, false));
        return printer::send_job(&st.sink, s, &r.number, &job);
    }
    if what != "slip" {
        job.push(&layout::resguardo(r, s, logo.is_some()), logo, &Opts::new(s, false));
    }
    if what != "resguardo" {
        job.push(&layout::repair_slip(r, s), None, &Opts::new(s, false));
    }
    printer::send_job(&st.sink, s, &r.number, &job)
}

fn repair_get(st: &AppState, id: i64) -> Result<Value> {
    st.user()?;
    let conn = st.conn();
    let r = repairs::load(&conn, id)?;
    let s = settings::load(&conn)?;
    let logo = tickets::shop_logo(&conn, &r.shop)?;
    let (resguardo, slip) = (layout::resguardo(&r, &s, logo.is_some()), layout::repair_slip(&r, &s));
    Ok(json!({ "repair": r, "resguardo": resguardo, "slip": slip, "logo": logo }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepairPreviewIn {
    #[serde(flatten)]
    repair: repairs::RepairIn,
    #[serde(default)]
    deposit_cents: i64,
    // a repair being corrected keeps its own number, date and deposit
    #[serde(default)]
    id: Option<i64>,
}

fn repair_preview(st: &AppState, me: &SessionUser, a: RepairPreviewIn) -> Result<Value> {
    let conn = st.conn();
    let s = settings::load(&conn)?;
    let r = match a.id {
        Some(id) => repairs::revised(&conn, id, &a.repair)?,
        None => repairs::draft(&conn, me, &s, &a.repair, a.deposit_cents)?,
    };
    let logo = tickets::shop_logo(&conn, &r.shop)?.is_some();
    Ok(json!({ "number": r.number, "resguardo": layout::resguardo(&r, &s, logo), "slip": layout::repair_slip(&r, &s) }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepairCreateIn {
    #[serde(flatten)]
    repair: repairs::NewRepair,
    #[serde(default = "yes")]
    print: bool,
}

// The repair and its deposit ticket are saved before anything prints. The
// deposit ticket prints like any sale (the original opens the drawer), then
// the resguardo and the slip.
fn repair_create(st: &AppState, me: &SessionUser, a: RepairCreateIn) -> Result<Value> {
    let (created, s) = {
        let mut conn = st.conn();
        let s = settings::load(&conn)?;
        (repairs::create(&mut conn, me, &s, &a.repair)?, s)
    };
    let repairs::Created { repair, deposit, duplicate } = created;
    if duplicate {
        log::warn(format!("repair {} was already taken in; not made again", repair.number));
        return Ok(json!({ "repair": repair, "deposit": null, "printed": false, "printError": null, "printWarning": null, "duplicate": true }));
    }
    st.mark(&repair.number);
    log::info(format!("repair {} taken in by {}", repair.number, me.name));
    let deposit = match deposit {
        Some(t) => {
            st.note(&t);
            aeat::kick();
            log::info(format!("deposit {} {} {} for {}", t.number, money::fmt(t.total_cents), t.payment.method, repair.number));
            let logo = tickets::logo_for(&st.conn(), &t)?;
            Some(issue(st, t, &s, logo.as_ref(), a.print))
        }
        None => None,
    };
    let (print_error, print_warning) = if a.print {
        let logo = tickets::shop_logo(&st.conn(), &repair.shop)?;
        match print_repair(st, &repair, &s, logo.as_ref(), "both") {
            Ok(w) => (None, w),
            Err(e) => (Some(e.0), None),
        }
    } else {
        (None, None)
    };
    if let Some(e) = print_error.as_ref().or(print_warning.as_ref()) {
        log::warn(format!("print {}: {e}", repair.number));
    }
    backup::auto(&st.conn());
    Ok(json!({
        "repair": repair,
        "deposit": deposit,
        "printed": a.print && print_error.is_none(),
        "printError": print_error,
        "printWarning": print_warning,
    }))
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
    let job = match t.kind.as_str() {
        "charset" => Job::raw(escpos::charset_test(&s)),
        "drawer" => Job::raw(escpos::kick(&s)),
        _ => {
            let lines = layout::test_page(&s, &me.name, logo.is_some());
            Job::doc(&lines, logo.as_ref(), &Opts::new(&s, false))
        }
    };
    let warning = printer::send_job(&st.sink, &s, "prueba", &job)?;
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
        // and today's is done, so no sale starts one on a thread of its own
        settings::put(&st.conn(), "lastBackupAt", &tickets::now()).unwrap();
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
        // the first sign-in of the day takes the backup
        st.conn().execute("DELETE FROM settings WHERE key = 'lastBackupAt'", []).unwrap();
        dispatch(&st, "login", json!({ "userId": jordi["id"], "password": "4321" })).unwrap();
        assert_eq!(st.data_dir.join("backups").read_dir().unwrap().count(), 1);

        for cmd in ["users.list", "settings.save", "products.save", "reports.summary", "day.delete", "backup.now", "printers.test"] {
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
    fn a_cashier_takes_a_phone_in_and_hands_it_back_but_only_an_admin_cancels() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let jordi = dispatch(&st, "users.save", json!({ "name": "Jordi", "role": "cashier", "password": "4321" })).unwrap();
        dispatch(&st, "logout", json!({})).unwrap();
        dispatch(&st, "login", json!({ "userId": jordi["id"], "password": "4321" })).unwrap();
        dispatch(&st, "cash.open", json!({ "floatCents": 5000 })).unwrap();
        let printed = |st: &AppState| -> Vec<String> {
            let mut files: Vec<_> = std::fs::read_dir(st.data_dir.join("prints")).unwrap().map(|e| e.unwrap().path()).collect();
            files.sort();
            files.iter().filter(|f| !f.to_string_lossy().contains("cajon")).map(|f| String::from_utf8_lossy(&std::fs::read(f).unwrap()).into_owned()).collect()
        };

        let phone = json!({
            "customerName": "Joan García", "customerPhone": "600 123 456", "brand": "Apple", "model": "iPhone 12", "colour": "Negro",
            "imei": "356789012345678", "fault": "No carga", "notes": "Código 9090", "estimateCents": 8000,
            "depositCents": 2000, "payment": { "method": "cash", "tenderedCents": 5000 }, "requestId": "form-1"
        });
        let preview = dispatch(&st, "repairs.preview", phone.clone()).unwrap();
        let c = dispatch(&st, "repairs.create", phone.clone()).unwrap();
        let r = c["repair"].clone();
        assert_eq!(preview["number"], r["number"]);
        assert_eq!((c["printed"].clone(), c["printError"].clone()), (json!(true), Value::Null));
        assert_eq!(c["deposit"]["ticket"]["payment"]["changeCents"], 3000);
        assert_eq!(r["createdBy"], "Jordi");
        // the deposit ticket with the drawer kick, then the resguardo and the slip
        let out = printed(&st);
        assert_eq!(out.len(), 2);
        assert!(out[0].contains("Anticipo reparaci") && out[0].contains("\x1bp\0"));
        assert!(out[1].contains("RESGUARDO DE REPARACI") && out[1].matches(r["number"].as_str().unwrap()).count() >= 3);
        assert!(!out[1].contains("9090"));
        // the deposit is money in the till
        assert_eq!(dispatch(&st, "cash.state", json!({})).unwrap()["expectedCents"], 7000);
        assert_eq!(dispatch(&st, "repairs.create", phone).unwrap()["duplicate"], true);

        let id = r["id"].clone();
        dispatch(&st, "repairs.status", json!({ "id": id, "status": "ready" })).unwrap();
        dispatch(&st, "repairs.print", json!({ "id": id, "what": "slip" })).unwrap();
        assert_eq!(printed(&st).len(), 3);
        assert_eq!(dispatch(&st, "repairs.print", json!({ "id": id, "what": "todo" })).unwrap_err(), "E_ARGS:what");
        assert_eq!(dispatch(&st, "repairs.cancel", json!({ "id": id, "reason": "error" })).unwrap_err(), "E_FORBIDDEN");
        let open = dispatch(&st, "repairs.list", json!({ "q": "garcia" })).unwrap();
        assert_eq!(open[0]["status"], "ready");

        // Entregar y cobrar: the line Venta sells hands the phone back
        let got = dispatch(&st, "repairs.get", json!({ "id": id })).unwrap();
        assert_eq!(got["repair"]["dueCents"], 6000);
        assert!(got["resguardo"].as_array().unwrap().iter().any(|l| l["text"].as_str().is_some_and(|t| t.contains("A pagar al recoger"))));
        let line = json!({ "name": got["repair"]["lineName"], "qty": 1, "unitCents": 6000, "vatBp": 2100, "repairId": id });
        let sale = json!({ "lines": [line], "payment": { "method": "card" } });
        let p = dispatch(&st, "sale.preview", sale.clone()).unwrap();
        let t = dispatch(&st, "sale.finalize", sale).unwrap();
        assert_eq!(t["ticket"]["lines"][0]["repairId"], id);
        let shown = dispatch(&st, "tickets.get", json!({ "id": t["ticket"]["id"] })).unwrap();
        assert_eq!(shown["lines"], p["lines"]);
        let after = dispatch(&st, "repairs.get", json!({ "id": id })).unwrap();
        assert_eq!((after["repair"]["status"].clone(), after["repair"]["ticket"]["number"].clone()), (json!("delivered"), t["ticket"]["number"].clone()));
        assert_eq!(dispatch(&st, "repairs.list", json!({})).unwrap().as_array().unwrap().len(), 0);

        // an admin cancels one taken in by mistake
        let mistake = json!({ "customerName": "Marta", "customerPhone": "699 000 000", "model": "Redmi 9", "fault": "Batería" });
        let m = dispatch(&st, "repairs.create", mistake).unwrap()["repair"]["id"].clone();
        dispatch(&st, "logout", json!({})).unwrap();
        dispatch(&st, "login", json!({ "userId": 1, "password": "123456" })).unwrap();
        assert_eq!(dispatch(&st, "repairs.cancel", json!({ "id": m, "reason": " " })).unwrap_err(), "E_REASON");
        let gone = dispatch(&st, "repairs.cancel", json!({ "id": m, "reason": "duplicada" })).unwrap();
        assert_eq!(gone["repair"]["status"], "cancelled");
        assert_eq!(dispatch(&st, "repairs.list", json!({ "status": "cancelled" })).unwrap()[0]["id"], m);
        // REP2026 is the repairs' series, so tickets cannot take it
        for prefix in ["REP", "RE"] {
            assert_eq!(dispatch(&st, "settings.save", json!({ "ticketPrefix": prefix })).unwrap_err(), "E_INVALID:ticketPrefix");
        }
        assert_eq!(dispatch(&st, "settings.save", json!({ "ticketPrefix": "REPA" })).unwrap()["ticketPrefix"], "REPA");
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

    // what a network printer received, one entry per connection
    fn listen(connections: usize) -> (u16, std::thread::JoinHandle<Vec<String>>) {
        use std::io::Read;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let got = std::thread::spawn(move || {
            (0..connections)
                .map(|_| {
                    let (mut c, _) = l.accept().unwrap();
                    c.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                    let mut buf = Vec::new();
                    c.read_to_end(&mut buf).unwrap();
                    String::from_utf8_lossy(&buf).into_owned()
                })
                .collect()
        });
        (port, got)
    }

    #[test]
    fn a_network_printer_prints_the_original_once_and_then_copies() {
        let _one = printer::NET_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let st = state(|_| PrintSink::Spooler);
        setup(&st);
        let off = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        dispatch(&st, "settings.save", json!({ "printerKind": "network", "printerHost": "127.0.0.1", "printerPort": off })).unwrap();
        let t = dispatch(&st, "sale.finalize", sale()).unwrap();
        assert_eq!((t["printed"].as_bool(), t["printError"].as_str()), (Some(false), Some("E_PRINT:offline")));
        let id = t["ticket"]["id"].clone();

        // the printer is back: nothing reached it before, so this is the original
        let (back, got) = listen(2);
        dispatch(&st, "settings.save", json!({ "printerPort": back })).unwrap();
        dispatch(&st, "tickets.print", json!({ "id": id })).unwrap();
        dispatch(&st, "tickets.print", json!({ "id": id })).unwrap();
        let prints = got.join().unwrap();
        assert!(prints[0].starts_with("\x1b@") && prints[0].contains("Cable"));
        assert!(!prints[0].contains("*** COPIA ***"));
        assert!(prints[1].contains("*** COPIA ***"));
        let got = dispatch(&st, "tickets.get", json!({ "id": id })).unwrap();
        let states: Vec<_> = got["ticket"]["prints"].as_array().unwrap().iter().map(|p| p["status"].clone()).collect();
        assert_eq!(states, ["failed", "printed", "printed"]);

        // Ajustes tests the address typed in, saved or not
        let (port, got) = listen(3);
        let draft = json!({ "printerKind": "network", "printerHost": "127.0.0.1", "printerPort": port });
        for kind in ["page", "charset", "drawer"] {
            assert_eq!(dispatch(&st, "printers.test", json!({ "settings": draft, "kind": kind })).unwrap()["warning"], Value::Null);
        }
        let tests = got.join().unwrap();
        assert!(tests[0].contains("PRUEBA DE IMPRESI") && tests[0].contains(&format!("127.0.0.1:{port}")));
        assert!(tests[1].contains("PRUEBA DE CARACTERES"));
        assert!(tests[2].ends_with("\x1bp\0\x19\u{fffd}"));
        let status = dispatch(&st, "printers.status", json!({})).unwrap();
        assert_eq!((status["name"].as_str(), status["state"].as_str()), (Some(format!("127.0.0.1:{back}").as_str()), Some("offline")));
    }

    #[test]
    fn the_first_sale_of_a_new_day_takes_the_backup_after_answering() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        // a till left open since another day
        settings::put(&st.conn(), "lastBackupAt", "2001-01-01T21:00:00").unwrap();
        assert!(backup::due(&st.conn()));
        dispatch(&st, "sale.finalize", sale()).unwrap();
        let t = std::time::Instant::now();
        while backup::due(&st.conn()) && t.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!backup::due(&st.conn()));
        let files = backup::list(&[st.data_dir.join("backups")]);
        assert_eq!(files.len(), 1);
        // the copy has the sale in it
        let copy = rusqlite::Connection::open(&files[0].path).unwrap();
        assert_eq!(copy.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
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
    #[test]
    fn the_till_no_longer_locks_itself_unless_chosen_and_use_keeps_it_open() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        assert_eq!(dispatch(&st, "settings.get", json!({})).unwrap()["autoLockMin"], 0);
        // the old default was 10 minutes; a fresh CI machine may not have been up for much longer
        let ago = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(11 * 60));
        *st.active_for_test() = ago.unwrap_or_else(std::time::Instant::now);
        assert!(dispatch(&st, "tickets.list", json!({})).is_ok());
        // chosen in Ajustes: the screen's pings keep a till in use open
        dispatch(&st, "settings.save", json!({ "autoLockMin": 2 })).unwrap();
        *st.active_for_test() = std::time::Instant::now() - std::time::Duration::from_secs(100);
        dispatch(&st, "session.ping", json!({})).unwrap();
        *st.active_for_test() -= std::time::Duration::from_secs(60);
        assert!(dispatch(&st, "tickets.list", json!({})).is_ok());
    }

    #[test]
    fn an_old_ten_minute_lock_is_switched_off_once_by_the_upgrade() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let conn = st.conn();
        let mut s = settings::load(&conn).unwrap();
        s.auto_lock_min = 10;
        settings::put(&conn, "settings", &serde_json::to_string(&s).unwrap()).unwrap();
        conn.execute("DELETE FROM settings WHERE key = 'lockOff15'", []).unwrap();
        settings::upgrade(&conn).unwrap();
        assert_eq!(settings::load(&conn).unwrap().auto_lock_min, 0);
        // a lock chosen after the upgrade stays
        s.auto_lock_min = 15;
        settings::put(&conn, "settings", &serde_json::to_string(&s).unwrap()).unwrap();
        settings::upgrade(&conn).unwrap();
        assert_eq!(settings::load(&conn).unwrap().auto_lock_min, 15);
        // a till already on 1.5 still gets the 1.6 resguardo text, once
        let mut s = settings::load(&conn).unwrap();
        s.repair_terms = "La reparación tiene 3 meses de garantía. Haga una copia de sus datos antes de dejar el equipo.".into();
        settings::put(&conn, "settings", &serde_json::to_string(&s).unwrap()).unwrap();
        conn.execute("DELETE FROM settings WHERE key = 'terms16'", []).unwrap();
        settings::upgrade(&conn).unwrap();
        assert!(settings::load(&conn).unwrap().repair_terms.contains("no se recoge en 3 meses"));
        assert_eq!(settings::load(&conn).unwrap().auto_lock_min, 15);
    }

    #[test]
    fn auto_sign_in_happens_once_as_tiquet_opens() {
        let dir = {
            let st = state(|d| PrintSink::Dir(d.join("prints")));
            setup(&st);
            dispatch(&st, "settings.save", json!({ "autoLogin": 1 })).unwrap();
            st.data_dir.clone()
        };
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        let b = dispatch(&st, "bootstrap", json!({})).unwrap();
        assert_eq!(b["session"]["name"], "Ana");
        // locking the till asks for the password from then on
        dispatch(&st, "logout", json!({})).unwrap();
        assert!(dispatch(&st, "bootstrap", json!({})).unwrap()["session"].is_null());
    }

    #[test]
    fn a_ticket_goes_out_as_text_pdf_whatsapp_or_email() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let r = dispatch(&st, "sale.finalize", json!({ "lines": [{ "name": "Funda iPhone", "qty": 1, "unitCents": 1250, "vatBp": 2100 }], "payment": { "method": "card" }, "print": false })).unwrap();
        let id = r["ticket"]["id"].as_i64().unwrap();
        let text = dispatch(&st, "tickets.text", json!({ "id": id })).unwrap()["text"].as_str().unwrap().to_string();
        assert!(text.contains("Funda iPhone") && text.contains("12,50"), "{text}");
        assert!(text.lines().all(|l| l.chars().count() <= 32), "{text}");
        // never printed: it goes out as the original
        assert!(!text.contains("COPIA"));
        // the shop's name is centred, as on paper
        assert!(text.lines().next().unwrap().starts_with("  "), "{text}");
        let pdf = dispatch(&st, "tickets.pdf", json!({ "id": id })).unwrap();
        let bytes = std::fs::read(pdf["path"].as_str().unwrap()).unwrap();
        assert!(bytes.starts_with(b"%PDF") && bytes.ends_with(b"%%EOF\n"));
        let w = dispatch(&st, "tickets.whatsapp", json!({ "id": id, "phone": "612 345 678", "web": true })).unwrap();
        assert!(w["url"].as_str().unwrap().starts_with("https://web.whatsapp.com/send?phone=34612345678&text=%60%60%60"));
        assert_eq!(dispatch(&st, "tickets.whatsapp", json!({ "id": id, "phone": "12" })).unwrap_err(), "E_INVALID:phone");
        // no mail account: the mail app, with the PDF ready to attach
        assert_eq!(dispatch(&st, "tickets.email", json!({ "id": id, "to": "no es un email" })).unwrap_err(), "E_INVALID:email");
        let m = dispatch(&st, "tickets.email", json!({ "id": id, "to": "cliente@example.com" })).unwrap();
        assert_eq!(m["sent"], false);
        assert!(m["url"].as_str().unwrap().starts_with("mailto:cliente@example.com?subject=Ticket%20"));
        // an account with a wrong server reports the connection, not a crash
        let cfg = json!({ "config": { "host": "smtp.invalid", "port": 587, "user": "x@example.com" }, "password": "x" });
        dispatch(&st, "email.save", cfg).unwrap();
        let err = dispatch(&st, "tickets.email", json!({ "id": id, "to": "cliente@example.com" })).unwrap_err();
        assert!(err.starts_with("E_MAIL"), "{err}");
        let got = dispatch(&st, "email.get", json!({})).unwrap();
        assert_eq!((got["config"]["host"].as_str(), got["hasPassword"].as_bool()), (Some("smtp.invalid"), Some(true)));
        assert!(got.to_string().find("\"x\"").is_none());
    }

    #[test]
    fn a_printer_switched_off_on_purpose_is_not_picked_again() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        // the test printer is there: nothing to do
        assert_eq!(dispatch(&st, "printers.auto", json!({})).unwrap()["applied"], false);
        dispatch(&st, "settings.save", json!({ "printerName": "" })).unwrap();
        assert!(settings::get(&st.conn(), "printerOff").unwrap().is_some());
        assert_eq!(dispatch(&st, "printers.auto", json!({})).unwrap()["applied"], false);
        dispatch(&st, "settings.save", json!({ "printerName": "POS-80" })).unwrap();
        assert!(settings::get(&st.conn(), "printerOff").unwrap().is_none());
    }

    #[test]
    fn the_paper_follows_the_printer_and_is_58_mm_when_nothing_says() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let cols = |s: &Value| (s["columns"].as_u64().unwrap(), s["paperAuto"].as_bool().unwrap());
        assert_eq!(settings::load(&st.conn()).unwrap().columns, 48);
        // a printer whose name says nothing: 58 mm prints right on any roll
        assert_eq!(cols(&dispatch(&st, "settings.save", json!({ "printerName": "Disashop" })).unwrap()), (32, true));
        assert_eq!(cols(&dispatch(&st, "settings.save", json!({ "printerName": "POS-58", "columns": 48, "paperAuto": true })).unwrap()), (32, true));
        assert_eq!(cols(&dispatch(&st, "settings.save", json!({ "printerName": "POS-80" })).unwrap()), (48, true));
        // Bluetooth and network printers say nothing either
        assert_eq!(cols(&dispatch(&st, "settings.save", json!({ "printerKind": "serial", "printerCom": "COM5" })).unwrap()), (32, true));
        // a width chosen by hand stays, whatever the printer
        let s = dispatch(&st, "settings.save", json!({ "printerKind": "windows", "printerName": "POS-58", "columns": 48 })).unwrap();
        assert_eq!(cols(&s), (48, false));
        // an install from before Papel automático: 48 columns on a 58 mm
        // printer, put right as Tiquet opens, once
        let mut old = settings::load(&st.conn()).unwrap();
        old.paper_auto = true;
        settings::put(&st.conn(), "settings", &serde_json::to_string(&old).unwrap()).unwrap();
        let r = dispatch(&st, "printers.auto", json!({})).unwrap();
        assert_eq!((r["applied"].as_bool(), r["paper"].as_bool(), r["columns"].as_u64()), (Some(true), Some(true), Some(32)));
        assert_eq!(settings::load(&st.conn()).unwrap().columns, 32);
        assert_eq!(dispatch(&st, "printers.auto", json!({})).unwrap()["applied"], false);
        // and a ticket is laid out 32 characters wide, 16 in big letters
        let id = dispatch(&st, "sale.finalize", sale()).unwrap()["ticket"]["id"].clone();
        let lines = dispatch(&st, "tickets.get", json!({ "id": id })).unwrap()["lines"].clone();
        for l in lines.as_array().unwrap().iter().filter(|l| l["t"] == "text") {
            let most = if l["size"] == "big" { 16 } else { 32 };
            assert!(layout::len(l["text"].as_str().unwrap()) <= most, "{l}");
        }
    }

    #[test]
    fn every_print_works_as_a_document_too() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "settings.save", json!({ "printerKind": "document", "printerName": "Oficina" })).unwrap();
        let r = dispatch(&st, "sale.finalize", sale()).unwrap();
        assert_eq!(r["printed"], true);
        let s = settings::load(&st.conn()).unwrap();
        assert_eq!(printer::target(&s), Some(printer::Target::Doc("Oficina".into())));
        let serial = Settings { printer_kind: "serial".into(), printer_com: " com7 ".into(), printer_baud: 7, ..s }.clean();
        assert_eq!(printer::target(&serial), Some(printer::Target::Serial("COM7".into(), 9600)));
    }
    #[test]
    fn a_shop_without_nif_prints_simple_tickets_and_cannot_invoice() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        let args = json!({ "shopName": "Fundas Ya", "shopNif": "", "adminName": "Ana", "password": "123456", "lang": "es" });
        dispatch(&st, "setup", args).unwrap();
        dispatch(&st, "settings.save", json!({ "shopAddress": "Calle Mayor 1", "shopPhone": "600000000", "ticketAddress": false, "ticketCashier": false })).unwrap();
        let r = dispatch(&st, "sale.finalize", json!({ "lines": [{ "name": "Funda", "qty": 1, "unitCents": 900, "vatBp": 2100 }], "payment": { "method": "cash" }, "print": false })).unwrap();
        let t = dispatch(&st, "tickets.get", json!({ "id": r["ticket"]["id"] })).unwrap();
        let text: Vec<String> = t["lines"].as_array().unwrap().iter().filter_map(|l| l["text"].as_str().map(|s| s.trim().to_string())).collect();
        assert!(text.contains(&"TICKET".to_string()), "{text:?}");
        assert!(!text.iter().any(|l| l.contains("FACTURA") || l.contains("NIF") || l.contains("Calle Mayor") || l.contains("atendió")), "{text:?}");
        assert!(text.iter().any(|l| l.contains("600000000")));
        // an invoice made out to a customer needs the shop's own NIF
        let inv = json!({ "lines": [{ "name": "Funda", "qty": 1, "unitCents": 900, "vatBp": 2100 }], "payment": { "method": "cash" }, "print": false,
            "customer": { "name": "Cliente SL", "nif": "B12345674", "address": "" } });
        let err = dispatch(&st, "sale.finalize", inv).unwrap_err();
        assert_eq!(err, "E_SHOP_NIF");
    }

    #[test]
    fn a_repair_handed_back_prints_a_pickup_receipt() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let terms = dispatch(&st, "settings.get", json!({})).unwrap()["repairTerms"].as_str().unwrap().to_string();
        assert!(terms.contains("no se recoge en 3 meses"), "{terms}");
        let rep = json!({ "customerName": "Juan", "customerPhone": "612345678", "brand": "Apple", "model": "iPhone 13", "fault": "Pantalla rota", "estimateCents": 0, "requestId": "r1", "print": false });
        let r = dispatch(&st, "repairs.create", rep).unwrap();
        let id = r["repair"]["id"].as_i64().unwrap();
        dispatch(&st, "repairs.deliver", json!({ "id": id })).unwrap();
        let before = std::fs::read_dir(st.data_dir.join("prints")).map(|d| d.count()).unwrap_or(0);
        dispatch(&st, "repairs.print", json!({ "id": id, "what": "delivery" })).unwrap();
        let after = std::fs::read_dir(st.data_dir.join("prints")).unwrap().count();
        assert_eq!(after, before + 1);
        let rr = crate::repairs::load(&st.conn(), id).unwrap();
        let s = settings::load(&st.conn()).unwrap();
        let text: Vec<String> = layout::repair_delivery(&rr, &s, false).iter().filter_map(|l| match l { layout::Line::Text { text, .. } => Some(text.clone()), _ => None }).collect();
        assert!(text.iter().any(|l| l.contains("JUSTIFICANTE DE ENTREGA")) && text.iter().any(|l| l.contains("iPhone 13")), "{text:?}");
    }
    #[test]
    fn sales_recharges_and_expenses_add_up_and_a_sale_need_not_print() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "cash.open", json!({ "floatCents": 5000 })).unwrap();
        let charge = |method: &str, print: bool| {
            let args = json!({ "lines": sale()["lines"], "payment": { "method": method }, "print": print });
            dispatch(&st, "sale.finalize", args).unwrap()
        };
        charge("cash", true);
        // not printing is the only difference: it is a numbered sale like any other
        let quiet = charge("card", false);
        assert_eq!(quiet["printed"], false);
        assert!(quiet["ticket"]["number"].as_str().unwrap().starts_with('T'));
        let add = |kind: &str, concept: &str, cents: i64, method: &str| {
            dispatch(&st, "book.add", json!({ "kind": kind, "concept": concept, "cents": cents, "method": method })).unwrap()["entry"]["id"].as_i64().unwrap()
        };
        let recarga = add("recharge", "", 1000, "cash");
        add("expense", "Pantalla", 400, "cash");
        let d = dispatch(&st, "cash.state", json!({})).unwrap();
        assert_eq!((d["rechargeCents"].as_i64(), d["expenseCents"].as_i64()), (Some(1000), Some(400)));
        assert_eq!(d["expectedCents"].as_i64(), Some(5000 + 690 + 1000 - 400));
        let today = tickets::now()[..10].to_string();
        let list = dispatch(&st, "day.list", json!({ "from": today, "to": today })).unwrap();
        let sum = &list["summary"];
        assert_eq!((sum["cashCents"].as_i64(), sum["cardCents"].as_i64(), sum["rechargeCents"].as_i64()), (Some(690), Some(690), Some(1000)));
        assert_eq!((sum["totalCents"].as_i64(), sum["expenseCents"].as_i64()), (Some(2380), Some(1400)));
        assert_eq!(sum["netCents"].as_i64(), Some(980));
        assert_eq!(dispatch(&st, "day.totals", json!({})).unwrap()["totalCents"], 2380);
        let rows = list["rows"].as_array().unwrap();
        let printed: Vec<&str> = rows.iter().filter(|r| r["source"] == "ticket").map(|r| r["printed"].as_str().unwrap()).collect();
        assert_eq!(printed, ["none", "printed"]);
        dispatch(&st, "book.print", json!({ "id": recarga })).unwrap();
        dispatch(&st, "book.delete", json!({ "id": recarga })).unwrap();
        let d = dispatch(&st, "cash.state", json!({})).unwrap();
        assert_eq!(d["expectedCents"].as_i64(), Some(5000 + 690 - 400));
        let seen = dispatch(&st, "day.list", json!({ "from": today, "to": today, "deleted": true })).unwrap();
        assert!(seen["rows"].as_array().unwrap().iter().any(|r| r["id"] == recarga && r["gone"] == true && r["source"] == "recharge"));
        let path = dispatch(&st, "book.export", json!({ "from": today, "to": today })).unwrap()["path"].as_str().unwrap().to_string();
        let csv = std::fs::read_to_string(path).unwrap();
        assert!(csv.contains("Pantalla") && !csv.contains("Recarga;") && csv.contains(";Venta;"));
    }

    #[test]
    fn a_sale_charged_the_wrong_way_is_put_right_on_the_same_ticket() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        dispatch(&st, "cash.open", json!({ "floatCents": 1000 })).unwrap();
        let args = json!({ "lines": sale()["lines"], "payment": { "method": "cash", "tenderedCents": 1000 } });
        let t = dispatch(&st, "sale.finalize", args).unwrap()["ticket"].clone();
        assert_eq!(t["payment"]["changeCents"], 310);
        let r = dispatch(&st, "tickets.payment", json!({ "id": t["id"], "method": "card" })).unwrap()["ticket"].clone();
        // the same ticket: number, time and amount as they were, the money where it belongs
        assert_eq!((&r["number"], &r["issuedAt"], &r["totalCents"]), (&t["number"], &t["issuedAt"], &t["totalCents"]));
        assert_eq!((r["payment"]["method"].as_str(), r["payment"]["changeCents"].as_i64()), (Some("card"), Some(0)));
        assert_eq!((r["paymentChanges"][0]["from"].as_str(), r["paymentChanges"][0]["byName"].as_str()), (Some("cash"), Some("Ana")));
        assert_eq!(dispatch(&st, "cash.state", json!({})).unwrap()["expectedCents"], 1000);
        let sum = dispatch(&st, "day.totals", json!({})).unwrap();
        assert_eq!((sum["cashCents"].as_i64(), sum["cardCents"].as_i64()), (Some(0), Some(690)));
        let lines = dispatch(&st, "tickets.get", json!({ "id": t["id"] })).unwrap()["lines"].to_string();
        assert!(lines.contains("Tarjeta") && !lines.contains("Efectivo"));
        // only through here, and not on a sale that was deleted
        assert!(st.conn().execute("UPDATE tickets SET payment_method = 'bizum'", []).is_err());
        assert!(st.conn().execute("UPDATE tickets SET total_cents = 1", []).is_err());
        assert_eq!(dispatch(&st, "tickets.payment", json!({ "id": t["id"], "method": "oro" })).unwrap_err(), "E_INVALID:method");
        dispatch(&st, "tickets.void", json!({ "id": t["id"] })).unwrap();
        assert_eq!(dispatch(&st, "tickets.payment", json!({ "id": t["id"], "method": "cash" })).unwrap_err(), "E_NOT_FOUND");
    }

    #[test]
    fn a_sale_with_the_wrong_items_is_replaced_in_one_step() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let old = dispatch(&st, "sale.finalize", sale()).unwrap()["ticket"].clone();
        let new_lines = json!([{ "name": "Cable", "qty": 2, "unitCents": 690, "vatBp": 2100 }]);
        let args = json!({ "lines": new_lines, "payment": { "method": "card" }, "replaces": old["id"], "print": false });
        let new = dispatch(&st, "sale.finalize", args).unwrap()["ticket"].clone();
        assert_ne!(new["number"], old["number"]);
        let gone = dispatch(&st, "tickets.get", json!({ "id": old["id"] })).unwrap()["ticket"].clone();
        assert_eq!(gone["voidReason"].as_str().unwrap(), format!("Corregida en {}", new["number"].as_str().unwrap()));
        let sum = dispatch(&st, "day.totals", json!({})).unwrap();
        assert_eq!((sum["totalCents"].as_i64(), sum["saleCount"].as_i64()), (Some(1380), Some(1)));
        // a sale that cannot be corrected leaves everything as it was
        let args = json!({ "lines": new_lines, "payment": { "method": "cash" }, "replaces": old["id"] });
        assert!(dispatch(&st, "sale.finalize", args).is_err());
        assert_eq!(dispatch(&st, "day.totals", json!({})).unwrap()["totalCents"], 1380);
    }

    #[test]
    fn who_was_signed_in_carries_on_after_an_update() {
        let dir = std::env::temp_dir().join(format!("tiquet-resume-{}-{}", std::process::id(), tickets::now().replace(':', "")));
        let args = json!({ "shopName": "Tienda", "shopNif": "B12345674", "adminName": "Ana", "password": "123456", "samples": false, "lang": "es" });
        {
            let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
            dispatch(&st, "setup", args).unwrap();
            dispatch(&st, "session.keep", json!({})).unwrap();
        }
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        assert_eq!(dispatch(&st, "bootstrap", json!({})).unwrap()["session"]["name"], "Ana");
        assert!(settings::get(&st.conn(), RESUME).unwrap().is_none());
        // once only, and never from long ago
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        assert!(dispatch(&st, "bootstrap", json!({})).unwrap()["session"].is_null());
        settings::put(&st.conn(), RESUME, r#"{"id":1,"at":"2026-01-01T10:00:00"}"#).unwrap();
        let st = AppState::open(&dir, PrintSink::Dir(dir.join("prints"))).unwrap();
        assert!(dispatch(&st, "bootstrap", json!({})).unwrap()["session"].is_null());
    }

    #[test]
    fn a_whole_day_goes_from_every_total_and_stays_on_record() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let first = dispatch(&st, "sale.finalize", sale()).unwrap()["ticket"]["id"].as_i64().unwrap();
        dispatch(&st, "sale.finalize", sale()).unwrap();
        dispatch(&st, "book.add", json!({ "kind": "expense", "concept": "Pantalla", "cents": 400 })).unwrap();
        dispatch(&st, "book.add", json!({ "kind": "recharge", "cents": 1000 })).unwrap();
        let today = tickets::now()[..10].to_string();
        // only the expenses first: the sales and the recharge stay
        let gone = dispatch(&st, "day.delete", json!({ "date": today, "what": "expenses" })).unwrap();
        assert_eq!((gone["tickets"].as_i64(), gone["entries"].as_i64()), (Some(0), Some(1)));
        assert_eq!(dispatch(&st, "day.totals", json!({})).unwrap()["totalCents"], 690 * 2 + 1000);
        let gone = dispatch(&st, "day.delete", json!({ "date": today })).unwrap();
        assert_eq!((gone["tickets"].as_i64(), gone["entries"].as_i64(), gone["kept"].as_i64()), (Some(2), Some(1), Some(0)));
        let sum = dispatch(&st, "day.totals", json!({})).unwrap();
        assert_eq!((sum["totalCents"].as_i64(), sum["expenseCents"].as_i64()), (Some(0), Some(0)));
        // everything is still there for the owner, with who deleted it
        let seen = dispatch(&st, "day.list", json!({ "from": today, "to": today, "deleted": true })).unwrap();
        let rows = seen["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r["gone"] == true && r["deletedBy"] == "Ana"));
        assert_eq!(dispatch(&st, "day.list", json!({ "from": today, "to": today })).unwrap()["rows"].as_array().unwrap().len(), 0);
        // and the numbers were never freed: the next sale carries on after them
        let next = dispatch(&st, "sale.finalize", sale()).unwrap()["ticket"]["id"].as_i64().unwrap();
        assert!(next > first + 1);
        assert_eq!(dispatch(&st, "tickets.void", json!({ "id": next })).unwrap()["ticket"]["voidReason"], "Borrado por Ana");
        assert_eq!(dispatch(&st, "day.delete", json!({ "date": "no es una fecha" })).unwrap_err(), "E_INVALID:range");
    }

    #[test]
    fn a_cashier_deletes_only_what_they_sold_today_and_never_a_day() {
        let st = state(|d| PrintSink::Dir(d.join("prints")));
        setup(&st);
        let admins = dispatch(&st, "sale.finalize", sale()).unwrap()["ticket"]["id"].as_i64().unwrap();
        dispatch(&st, "users.save", json!({ "name": "Eva", "role": "cashier", "password": "1234", "active": true })).unwrap();
        dispatch(&st, "logout", json!({})).unwrap();
        let users = dispatch(&st, "bootstrap", json!({})).unwrap()["users"].clone();
        let eva = users.as_array().unwrap().iter().find(|u| u["name"] == "Eva").unwrap()["id"].as_i64().unwrap();
        dispatch(&st, "login", json!({ "userId": eva, "password": "1234" })).unwrap();
        let hers = dispatch(&st, "sale.finalize", sale()).unwrap()["ticket"]["id"].as_i64().unwrap();
        assert_eq!(dispatch(&st, "tickets.void", json!({ "id": admins })).unwrap_err(), "E_FORBIDDEN");
        assert_eq!(dispatch(&st, "tickets.void", json!({ "id": hers })).unwrap()["ticket"]["voidedBy"], "Eva");
        let today = tickets::now()[..10].to_string();
        assert_eq!(dispatch(&st, "day.delete", json!({ "date": today })).unwrap_err(), "E_FORBIDDEN");
    }
}
