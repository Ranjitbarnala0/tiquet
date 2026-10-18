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

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub product_id: Option<i64>,
    pub name: String,
    pub unit_cents: i64,
    pub vat_bp: i64,
    pub at: String,
    pub times: i64,
}

// What was sold lately, from the catalogue or typed by hand, newest first:
// one tap sells it again at the same price.
pub fn recent(conn: &Connection, limit: i64) -> Result<Vec<Recent>> {
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN p.active = 1 THEN l.product_id END, l.name, l.unit_cents, l.vat_bp, MAX(t.issued_at), COUNT(*)
         FROM ticket_lines l JOIN tickets t ON t.id = l.ticket_id LEFT JOIN products p ON p.id = l.product_id
         WHERE t.kind = 'sale' AND t.training = 0 AND t.voided_at IS NULL AND l.unit_cents > 0 AND l.repair_id IS NULL
         GROUP BY l.name, l.unit_cents, l.vat_bp
         ORDER BY MAX(t.id) DESC LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit.clamp(1, 60)], |r| {
            Ok(Recent { product_id: r.get(0)?, name: r.get(1)?, unit_cents: r.get(2)?, vat_bp: r.get(3)?, at: r.get(4)?, times: r.get(5)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// A product never sold goes for good. One that is on tickets is archived
// instead, so those tickets and the reports keep its name. Returns whether it
// was really deleted.
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    let sold: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM ticket_lines WHERE product_id = ?1)", [id], |r| r.get(0))?;
    let n = if sold {
        conn.execute("UPDATE products SET active = 0, favorite = 0 WHERE id = ?1", [id])?
    } else {
        conn.execute("DELETE FROM products WHERE id = ?1", [id])?
    };
    if n == 0 {
        return fail("E_NOT_FOUND");
    }
    Ok(!sold)
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

// one row of an imported sheet; a column the sheet does not have is None
// and leaves that field of an existing product as it was
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportRow {
    pub name: String,
    pub price_cents: i64,
    #[serde(default)]
    pub vat_bp: Option<i64>,
    #[serde(default)]
    pub barcode: Option<String>,
}

// re-importing the same sheet updates prices instead of duplicating rows:
// a row matches by barcode when it has one, otherwise by name
pub fn import(conn: &mut Connection, rows: Vec<ImportRow>, default_vat: i64) -> Result<Imported> {
    let tx = conn.transaction()?;
    let mut out = Imported::default();
    for row in rows {
        let (vat, code) = (row.vat_bp, row.barcode.clone());
        let mut p = Product {
            id: 0,
            name: row.name,
            price_cents: row.price_cents,
            vat_bp: row.vat_bp.unwrap_or(default_vat),
            barcode: row.barcode.unwrap_or_default(),
            favorite: false,
            active: true,
            sold_qty: 0,
        };
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
                let code = code.map(|_| p.barcode.clone());
                tx.execute(
                    "UPDATE products SET name = ?1, price_cents = ?2, vat_bp = COALESCE(?3, vat_bp), barcode = COALESCE(?4, barcode),
                     updated_at = strftime('%Y-%m-%dT%H:%M:%S', 'now', 'localtime') WHERE id = ?5",
                    params![p.name, p.price_cents, vat, code, id],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_and_price_sheet_keeps_barcodes_and_vat() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let mut conn = crate::db::open(&std::env::temp_dir().join(format!("tiquet-prod-{n}")).join("t.db")).unwrap();
        let p = Product { id: 0, name: "Pilas AA".into(), price_cents: 450, vat_bp: 1000, barcode: "8410000000017".into(), favorite: true, active: true, sold_qty: 0 };
        save(&conn, p).unwrap();
        let row = |name: &str, price, vat, code: Option<&str>| ImportRow { name: name.into(), price_cents: price, vat_bp: vat, barcode: code.map(Into::into) };
        let r = import(&mut conn, vec![row("pilas aa", 495, None, None), row("Funda", 995, None, None)], 2100).unwrap();
        assert_eq!((r.inserted, r.updated), (1, 1));
        let all = list(&conn, true).unwrap();
        let pilas = all.iter().find(|p| p.price_cents == 495).unwrap();
        assert_eq!((pilas.vat_bp, pilas.barcode.as_str(), pilas.favorite), (1000, "8410000000017", true));
        assert_eq!(all.iter().find(|p| p.name == "Funda").unwrap().vat_bp, 2100);
    }
}
