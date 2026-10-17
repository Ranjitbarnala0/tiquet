import { norm, parseMoney } from './format';

// fields left out were not in the file: an update keeps what the product had
export interface ProductIn {
  name: string;
  priceCents: number;
  vatBp?: number;
  barcode?: string;
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

function vat(s: string | undefined): number | null {
  if (!s?.trim()) return null;
  const n = Number(s.replace('%', '').replace(',', '.').trim());
  if (!Number.isFinite(n) || n < 0 || n > 100) return null;
  return Math.round((n < 1 && n > 0 ? n * 100 : n) * 100);
}

const CODE = ['ean', 'barras', 'barcode', 'codigo', 'code', 'sku', 'referencia', 'ref'];
const isCode = (h: string) => CODE.some((w) => h.includes(w));

// The sale price wins over a generic "precio"; a cost or purchase price is
// never taken; a price without VAT is only used, grossed up, when nothing
// else is there.
function priceScore(h: string): { score: number; net: boolean } | null {
  if (/compra|coste|costo|\bcost|proveedor|purchase|mayorista|margen|stock|cantidad|unidades/.test(h)) return null;
  const net = /sin iva|sin impuesto|\bneto|\bbase|excl|without vat|net price/.test(h);
  if (/pvp|venta|con iva|iva incl|final|retail|sale|publico/.test(h)) return { score: 3, net };
  if (/precio|price|importe|tarifa|pvd/.test(h)) return { score: net ? 0 : 2, net };
  return null;
}

// Accepts exports from most tills and spreadsheets: finds the columns by
// header name in Spanish or English, or assumes name;price when there is none.
export function toProducts(rows: string[][]): { rows: ProductIn[]; skipped: number; columns: string[] } {
  if (!rows.length) return { rows: [], skipped: 0, columns: [] };
  const head = rows[0].map((h) => norm(h).trim());
  const find = (test: (h: string) => boolean) => head.findIndex(test);
  let iName = find((h) => /nombre|name|producto|product|descrip|articulo|concepto/.test(h) && !isCode(h));
  const prices = head
    .map((h, i) => ({ i, s: priceScore(h) }))
    .filter((x): x is { i: number; s: { score: number; net: boolean } } => x.s !== null)
    .sort((a, b) => b.s.score - a.s.score);
  let iPrice = prices[0]?.i ?? -1;
  const net = prices[0]?.s.net ?? false;
  let iVat = find((h) => /iva|vat|impuesto|tax/.test(h) && priceScore(h) === null);
  const codes = head.map((h, i) => ({ h, i })).filter((x) => isCode(x.h) && x.i !== iName && x.i !== iPrice);
  let iCode = (codes.find((x) => /ean|barras|barcode/.test(x.h)) ?? codes[0])?.i ?? -1;
  let body = rows.slice(1);
  if (iName < 0 || iPrice < 0) {
    [iName, iPrice, iVat, iCode] = [0, 1, -1, -1];
    body = rows;
  }
  const out: ProductIn[] = [];
  let skipped = 0;
  for (const r of body) {
    const name = (r[iName] ?? '').trim();
    let price = parseMoney(r[iPrice] ?? '');
    const rate = iVat >= 0 ? vat(r[iVat]) : null;
    if (net && price !== null) price = Math.round((price * (10000 + (rate ?? 2100))) / 10000);
    if (!name || price === null || price < 0 || price > 9_999_999) {
      skipped++;
      continue;
    }
    const p: ProductIn = { name: name.slice(0, 120), priceCents: price };
    if (rate !== null) p.vatBp = rate;
    if (iCode >= 0) p.barcode = (r[iCode] ?? '').trim().slice(0, 40);
    out.push(p);
  }
  const columns = [iName, iPrice, iVat, iCode].filter((i) => i >= 0 && body !== rows).map((i) => rows[0][i]);
  return { rows: out, skipped, columns };
}
