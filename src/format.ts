// Same rules as money::fmt on the Rust side, so screen and paper agree.
export function fmt(cents: number): string {
  const abs = Math.abs(Math.round(cents));
  const euros = Math.floor(abs / 100)
    .toString()
    .replace(/\B(?=(\d{3})+(?!\d))/g, '.');
  return `${cents < 0 ? '-' : ''}${euros},${String(abs % 100).padStart(2, '0')}`;
}

export const eur = (cents: number) => `${fmt(cents)} €`;

// the core's limit for one unit price (99.999,99 EUR)
export const MAX_PRICE = 9_999_999;

// Accepts "12,5", "12.50", "1.234,56", "1234" and "-2". Returns cents, or
// null for anything unreadable or above `max`.
export function parseMoney(input: string, max = MAX_PRICE): number | null {
  let s = input.replace(/[€\s]/g, '');
  if (!s) return null;
  const neg = s.startsWith('-');
  if (neg) s = s.slice(1);
  if (!/^[\d.,]+$/.test(s)) return null;
  let int = s;
  let frac = '';
  const sep = Math.max(s.lastIndexOf(','), s.lastIndexOf('.'));
  if (sep >= 0) {
    const after = s.slice(sep + 1);
    if (after.length <= 2) {
      int = s.slice(0, sep);
      frac = after;
    }
  }
  int = int.replace(/[.,]/g, '');
  if (!int && !frac) return null;
  const cents = Number(int || '0') * 100 + Number(frac.padEnd(2, '0'));
  if (!Number.isFinite(cents) || cents > max) return null;
  return neg ? -cents : cents;
}

// Spanish VAT rates, general first; the core accepts anything 0-100 %
export const VAT_RATES = [2100, 1000, 500, 400, 0];

export function rate(bp: number): string {
  return `${(bp / 100).toLocaleString('es-ES')}%`;
}

export function isoDate(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function addDays(d: Date, n: number): Date {
  const c = new Date(d);
  c.setDate(c.getDate() + n);
  return c;
}

export function shortDate(iso: string): string {
  return `${iso.slice(8, 10)}/${iso.slice(5, 7)}/${iso.slice(0, 4)}`;
}

export function dateTime(iso: string): string {
  return `${shortDate(iso)} ${iso.slice(11, 16)}`;
}

export const norm = (s: string) =>
  s
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase();

export function initials(name: string): string {
  const parts = name.trim().split(/\s+/);
  return ((parts[0]?.[0] ?? '') + (parts.length > 1 ? parts[parts.length - 1][0] : '')).toUpperCase();
}
