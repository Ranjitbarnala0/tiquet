use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::{fail, Result};
use crate::tickets::MAX_PRICE;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Product {
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub price_cents: i64,
    pub vat_bp: i64,
    #[serde(default)]
    pub barcode: String,
    #[serde(default)]
    pub favorite: bool,
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub sold_qty: i64,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Debug, Default)]
pub struct Imported {
    pub inserted: usize,
    pub updated: usize,
}

pub fn list(conn: &Connection, all: bool) -> Result<Vec<Product>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, price_cents, vat_bp, barcode, favorite, active, sold_qty FROM products
         WHERE active = 1 OR ?1 = 1
         ORDER BY active DESC, favorite DESC, sold_qty DESC, name COLLATE NOCASE",
    )?;
    let rows = stmt
        .query_map([all], |r| {
            Ok(Product {
                id: r.get(0)?,
                name: r.get(1)?,
                price_cents: r.get(2)?,
                vat_bp: r.get(3)?,
                barcode: r.get(4)?,
                favorite: r.get(5)?,
                active: r.get(6)?,
                sold_qty: r.get(7)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn check(p: &mut Product) -> Result<()> {
    p.name = p.name.split_whitespace().collect::<Vec<_>>().join(" ");
    p.barcode = p.barcode.trim().to_string();
    if p.name.is_empty() || p.name.chars().count() > 120 {
        return fail("E_INVALID:name");
    }
    if !(0..=MAX_PRICE).contains(&p.price_cents) {
        return fail("E_INVALID:price");
    }
    if !(0..=10_000).contains(&p.vat_bp) {
        return fail("E_INVALID:vat");
    }
    if p.barcode.chars().count() > 40 {
        return fail("E_INVALID:barcode");
    }
    Ok(())
}

pub fn save(conn: &Connection, mut p: Product) -> Result<Product> {
    check(&mut p)?;
    if !p.barcode.is_empty() && p.active {
        let clash: Option<i64> = conn
            .query_row(
                "SELECT id FROM products WHERE barcode = ?1 AND active = 1 AND id <> ?2",
                params![p.barcode, p.id],
                |r| r.get(0),
            )
            .optional()?;
        if clash.is_some() {
            return fail("E_BARCODE");
        }
    }
    if p.id == 0 {
        conn.execute(
            "INSERT INTO products (name, price_cents, vat_bp, barcode, favorite, active) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![p.name, p.price_cents, p.vat_bp, p.barcode, p.favorite, p.active],
        )?;
        p.id = conn.last_insert_rowid();
    } else {
        let n = conn.execute(
            "UPDATE products SET name = ?1, price_cents = ?2, vat_bp = ?3, barcode = ?4, favorite = ?5, active = ?6,
             updated_at = strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime') WHERE id = ?7",
            params![p.name, p.price_cents, p.vat_bp, p.barcode, p.favorite, p.active, p.id],
        )?;
        if n == 0 {
            return fail("E_NOT_FOUND");
        }
    }
    Ok(p)
}

// re-importing the same sheet updates prices instead of duplicating rows:
// a row matches by barcode when it has one, otherwise by name
pub fn import(conn: &mut Connection, rows: Vec<Product>) -> Result<Imported> {
    let tx = conn.transaction()?;
    let mut out = Imported::default();
    for mut p in rows {
        check(&mut p)?;
        let existing: Option<i64> = if p.barcode.is_empty() {
            tx.query_row(
                "SELECT id FROM products WHERE active = 1 AND name = ?1 COLLATE NOCASE",
                [&p.name],
                |r| r.get(0),
            )
            .optional()?
        } else {
            tx.query_row(
                "SELECT id FROM products WHERE active = 1 AND barcode = ?1",
                [&p.barcode],
                |r| r.get(0),
            )
            .optional()?
        };
        match existing {
            Some(id) => {
                tx.execute(
                    "UPDATE products SET name = ?1, price_cents = ?2, vat_bp = ?3, barcode = ?4,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime') WHERE id = ?5",
                    params![p.name, p.price_cents, p.vat_bp, p.barcode, id],
                )?;
                out.updated += 1;
            }
            None => {
                tx.execute(
                    "INSERT INTO products (name, price_cents, vat_bp, barcode, favorite) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![p.name, p.price_cents, p.vat_bp, p.barcode, p.favorite],
                )?;
                out.inserted += 1;
            }
        }
    }
    tx.commit()?;
    Ok(out)
}

pub fn seed(conn: &Connection) -> Result<()> {
    let rows: [(&str, i64, bool); 12] = [
        ("Cable USB-C a USB-C 1 m", 690, true),
        ("Cable Lightning 1 m", 790, true),
        ("Cargador USB-C 20 W", 1490, true),
        ("Funda de silicona", 995, true),
        ("Protector de pantalla cristal templado", 795, true),
        ("Instalación de protector", 300, true),
        ("Auriculares Bluetooth", 1990, true),
        ("Reparación", 0, true),
        ("Cargador de coche doble USB", 990, false),
        ("Batería externa 10.000 mAh", 2290, false),
        ("Soporte de móvil para coche", 1190, false),
        ("Tarjeta microSD 64 GB", 1290, false),
    ];
    for (name, price, fav) in rows {
        conn.execute(
            "INSERT INTO products (name, price_cents, vat_bp, favorite) VALUES (?1, ?2, 2100, ?3)",
            params![name, price, fav],
        )?;
    }
    Ok(())
}
