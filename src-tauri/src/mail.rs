// Tickets sent by email straight from Tiquet through the shop's own mail
// account (SMTP): Gmail or Outlook with an app password, or the mail that
// comes with a web hosting. The password is kept encrypted for this Windows
// user (DPAPI) and never travels to the screen.

use std::time::Duration;

use base64::Engine;
use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{Message, SmtpTransport, Transport};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{fail, Error, Result};
use crate::settings;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub host: String,
    pub port: u16,
    // "starttls" (port 587), "tls" (465) or "none"
    pub security: String,
    pub user: String,
    // the address the customer sees; the account's own when empty
    pub from: String,
}

impl Config {
    pub fn clean(mut self) -> Config {
        self.host = self.host.trim().to_lowercase();
        self.user = self.user.trim().into();
        self.from = self.from.trim().into();
        if !["starttls", "tls", "none"].contains(&self.security.as_str()) {
            self.security = "starttls".into();
        }
        if self.port == 0 {
            self.port = if self.security == "tls" { 465 } else { 587 };
        }
        self
    }

    pub fn ready(&self) -> bool {
        !self.host.is_empty() && !self.user.is_empty()
    }
}

pub fn load(conn: &Connection) -> Result<(Config, bool)> {
    let cfg = settings::get(conn, "mail")?.and_then(|j| serde_json::from_str::<Config>(&j).ok()).unwrap_or_default().clean();
    Ok((cfg, settings::get(conn, "mailPass")?.is_some()))
}

// `pass`: None keeps the saved one, "" forgets it
pub fn save(conn: &Connection, cfg: Config, pass: Option<&str>) -> Result<Config> {
    let cfg = cfg.clean();
    if !cfg.host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') {
        return fail("E_INVALID:mailHost");
    }
    if !cfg.from.is_empty() && cfg.from.parse::<lettre::Address>().is_err() {
        return fail("E_INVALID:mailFrom");
    }
    settings::put(conn, "mail", &serde_json::to_string(&cfg)?)?;
    match pass {
        Some("") => {
            conn.execute("DELETE FROM settings WHERE key = 'mailPass'", [])?;
        }
        Some(p) => settings::put(conn, "mailPass", &protect(p)?)?,
        None => {}
    }
    Ok(cfg)
}

pub fn valid(address: &str) -> bool {
    address.trim().parse::<lettre::Address>().is_ok()
}

// The account ready to send, read while the database is held; the sending
// itself happens after, so a slow mail server never holds up the till.
pub struct Account {
    cfg: Config,
    pass: String,
}

pub fn account(conn: &Connection) -> Result<Account> {
    let (cfg, _) = load(conn)?;
    if !cfg.ready() {
        return fail("E_MAIL_SETUP");
    }
    let pass = settings::get(conn, "mailPass")?.and_then(|p| unprotect(&p)).ok_or_else(|| Error("E_MAIL_PASS".into()))?;
    Ok(Account { cfg, pass })
}

impl Account {
    pub fn send(&self, from_name: &str, to: &str, subject: &str, body: &str, attachment: Option<(String, Vec<u8>)>) -> Result<()> {
        let to: Mailbox = to.trim().parse().map_err(|_| Error("E_INVALID:email".into()))?;
        let address = if self.cfg.from.is_empty() { &self.cfg.user } else { &self.cfg.from };
        let address = address.parse().map_err(|_| Error("E_INVALID:mailFrom".into()))?;
        let name = Some(from_name.trim().to_string()).filter(|n| !n.is_empty());
        let builder = Message::builder().from(Mailbox::new(name, address)).to(to).subject(subject);
        let text = SinglePart::plain(body.to_string());
        let msg = match attachment {
            Some((file, bytes)) => {
                let pdf = ContentType::parse("application/pdf").map_err(|e| Error(format!("E_MAIL:{e}")))?;
                builder.multipart(MultiPart::mixed().singlepart(text).singlepart(Attachment::new(file).body(bytes, pdf)))
            }
            None => builder.singlepart(text),
        }
        .map_err(|e| Error(format!("E_MAIL:{e}")))?;
        let relay = match self.cfg.security.as_str() {
            "tls" => SmtpTransport::relay(&self.cfg.host),
            "none" => Ok(SmtpTransport::builder_dangerous(&self.cfg.host)),
            _ => SmtpTransport::starttls_relay(&self.cfg.host),
        }
        .map_err(|e| Error(format!("E_MAIL_CONNECT:{e}")))?;
        let mailer = relay
            .port(self.cfg.port)
            .credentials(Credentials::new(self.cfg.user.clone(), self.pass.clone()))
            .timeout(Some(Duration::from_secs(20)))
            .build();
        mailer.send(&msg).map_err(|e| Error(why(&e)))?;
        Ok(())
    }
}

fn why(e: &lettre::transport::smtp::Error) -> String {
    let text = e.to_string();
    let lower = text.to_lowercase();
    if lower.contains("535") || lower.contains("534") || lower.contains("authentication") || lower.contains("credentials") {
        "E_MAIL_AUTH".into()
    } else if e.is_timeout() || e.is_tls() || lower.contains("connection") || lower.contains("resolve") || lower.contains("network") {
        format!("E_MAIL_CONNECT:{text}")
    } else {
        format!("E_MAIL:{text}")
    }
}

#[cfg(windows)]
fn protect(p: &str) -> Result<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    let mut data = p.as_bytes().to_vec();
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_mut_ptr() };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    if ok == 0 || out.pbData.is_null() {
        return fail("E_MAIL_PASS");
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    Ok(format!("dpapi:{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[cfg(windows)]
fn unprotect(stored: &str) -> Option<String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB};

    let mut data = base64::engine::general_purpose::STANDARD.decode(stored.strip_prefix("dpapi:")?).ok()?;
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_mut_ptr() };
    let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    if ok == 0 || out.pbData.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    unsafe {
        LocalFree(out.pbData as _);
    }
    String::from_utf8(bytes).ok()
}

#[cfg(not(windows))]
fn protect(p: &str) -> Result<String> {
    Ok(format!("plain:{}", base64::engine::general_purpose::STANDARD.encode(p)))
}

#[cfg(not(windows))]
fn unprotect(stored: &str) -> Option<String> {
    String::from_utf8(base64::engine::general_purpose::STANDARD.decode(stored.strip_prefix("plain:")?).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_password_is_kept_encrypted_and_never_shown() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let conn = crate::db::open(&std::env::temp_dir().join(format!("tiquet-mail-{n}")).join("t.db")).unwrap();
        let cfg = Config { host: " SMTP.Gmail.com ".into(), user: "tienda@gmail.com".into(), ..Config::default() };
        let saved = save(&conn, cfg, Some("abcd efgh ijkl mnop")).unwrap();
        assert_eq!((saved.host.as_str(), saved.port, saved.security.as_str()), ("smtp.gmail.com", 587, "starttls"));
        let stored = settings::get(&conn, "mailPass").unwrap().unwrap();
        assert!(!stored.contains("abcd"));
        assert_eq!(account(&conn).unwrap().pass, "abcd efgh ijkl mnop");
        // saving the rest again keeps the password; an empty one forgets it
        save(&conn, saved.clone(), None).unwrap();
        assert!(load(&conn).unwrap().1);
        save(&conn, saved, Some("")).unwrap();
        assert_eq!(account(&conn).err().map(|e| e.0), Some("E_MAIL_PASS".into()));
        assert!(save(&conn, Config { host: "smtp.gmail.com; rm".into(), ..Config::default() }, None).is_err());
    }
}
