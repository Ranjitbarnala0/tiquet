// The same check as nif.rs, so a mistyped NIF is flagged while typing; the
// core checks again before saving.
const LETTERS = 'TRWAGMYFPDXBNJZSQVHLCKE';

export function normalizeNif(s: string): string {
  const t = s.toUpperCase().replace(/[^A-Z0-9]/g, '');
  return t.startsWith('ES') && t.length === 11 ? t.slice(2) : t;
}

function cifOk(s: string): boolean {
  const body = s.slice(1, 8);
  if (!/^\d{7}$/.test(body)) return false;
  let sum = 0;
  [...body].forEach((c, i) => {
    const d = Number(c);
    if (i % 2 === 0) {
      const x = d * 2;
      sum += Math.floor(x / 10) + (x % 10);
    } else sum += d;
  });
  const check = (10 - (sum % 10)) % 10;
  const letter = 'JABCDEFGHI'[check];
  const last = s[8];
  if ('NPQRSW'.includes(s[0])) return last === letter;
  if ('ABEH'.includes(s[0])) return last === String(check);
  return last === letter || last === String(check);
}

export function spanishNif(id: string): boolean {
  const s = normalizeNif(id);
  if (s.length !== 9) return false;
  const last = s[8];
  if (/^\d{8}[A-Z]$/.test(s)) return LETTERS[Number(s.slice(0, 8)) % 23] === last;
  if (/^[XYZ]\d{7}[A-Z]$/.test(s)) return LETTERS[Number('XYZ'.indexOf(s[0]) + s.slice(1, 8)) % 23] === last;
  if (/^[KLM]\d{7}[A-Z]$/.test(s)) return LETTERS[Number(s.slice(1, 8)) % 23] === last;
  if (/^[ABCDEFGHJNPQRSUVW]\d{7}[0-9A-J]$/.test(s)) return cifOk(s);
  return false;
}

// a customer can also be a company elsewhere in the EU
export function customerNif(id: string): boolean {
  const s = normalizeNif(id);
  return spanishNif(s) || (/^[A-Z]{2}[A-Z0-9]{2,12}$/.test(s) && !s.startsWith('ES'));
}
