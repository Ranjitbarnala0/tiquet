import { norm, parseMoney } from './format';

export interface ProductIn {
  name: string;
  priceCents: number;
  vatBp: number;
  barcode: string;
  favorite: boolean;
}

export function parseCsv(text: string): string[][] {
  text = text.replace(/^﻿/, '');
  const first = text.split(/\r?\n/, 1)[0] ?? '';
  const delim = [';', '\t', ','].map((d) => [d, first.split(d).length] as const).sort((a, b) => b[1] - a[1])[0][0];
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = '';
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"' && text[i + 1] === '"') {
        cell += '"';
        i++;
      } else if (c === '"') quoted = false;
      else cell += c;
    } else if (c === '"') quoted = true;
    else if (c === delim) {
      row.push(cell);
      cell = '';
    } else if (c === '\n' || c === '\r') {
      if (c === '\r' && text[i + 1] === '\n') i++;
      row.push(cell);
      rows.push(row);
      row = [];
      cell = '';
    } else cell += c;
  }
  if (cell || row.length) {
    row.push(cell);
    rows.push(row);
  }
  return rows.filter((r) => r.some((c) => c.trim()));
}

function vat(s: string | undefined, fallback: number): number {
  if (!s?.trim()) return fallback;
  const n = Number(s.replace('%', '').replace(',', '.').trim());
  if (!Number.isFinite(n) || n < 0) return fallback;
  return Math.round((n < 1 && n > 0 ? n * 100 : n) * 100);
}

// Accepts exports from most tills and spreadsheets: finds the columns by
// header name in Spanish or English, or assumes name;price when there is none.
export function toProducts(rows: string[][], defaultVat: number): { rows: ProductIn[]; skipped: number } {
  if (!rows.length) return { rows: [], skipped: 0 };
  const head = rows[0].map(norm);
  const col = (...names: string[]) => head.findIndex((h) => names.some((n) => h.includes(n)));
  let iName = col('nombre', 'name', 'producto', 'product', 'descrip', 'articulo');
  let iPrice = col('precio', 'price', 'pvp', 'importe');
  let iVat = col('iva', 'vat', 'impuesto', 'tax');
  let iCode = col('codigo', 'barcode', 'ean', 'sku', 'referencia', 'code');
  let body = rows.slice(1);
  if (iName < 0 || iPrice < 0) {
    [iName, iPrice, iVat, iCode] = [0, 1, -1, -1];
    body = rows;
  }
  const out: ProductIn[] = [];
  let skipped = 0;
  for (const r of body) {
    const name = (r[iName] ?? '').trim();
    const price = parseMoney(r[iPrice] ?? '');
    if (!name || price === null || price < 0 || price > 9_999_999) {
      skipped++;
      continue;
    }
    out.push({
      name: name.slice(0, 120),
      priceCents: price,
      vatBp: vat(iVat >= 0 ? r[iVat] : undefined, defaultVat),
      barcode: iCode >= 0 ? (r[iCode] ?? '').trim().slice(0, 40) : '',
      favorite: false,
    });
  }
  return { rows: out, skipped };
}
