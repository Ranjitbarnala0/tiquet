import { ApiError } from './api';

export type Lang = 'es' | 'en';

let lang: Lang = 'es';

export function setLang(l: Lang) {
  lang = l;
  document.documentElement.lang = l;
}

export function getLang(): Lang {
  return lang;
}

// Two languages, so strings live inline next to the markup that uses them.
export function tr(es: string, en: string): string {
  return lang === 'es' ? es : en;
}

const errors: Record<string, [string, string]> = {
  E_AUTH: ['Contraseña incorrecta', 'Wrong password'],
  E_LOCKED: ['Demasiados intentos. Espera {s} s', 'Too many attempts. Wait {s} s'],
  E_SESSION: ['La sesión ha caducado. Vuelve a entrar.', 'Session expired. Please sign in again.'],
  E_FORBIDDEN: ['Solo un administrador puede hacer esto', 'Only an administrator can do this'],
  E_NOT_FOUND: ['No encontrado', 'Not found'],
  E_EMPTY: ['El ticket está vacío', 'The ticket is empty'],
  E_TENDERED: ['El importe entregado no cubre el total', 'The amount received does not cover the total'],
  E_NEGATIVE: ['El total no puede ser negativo', 'The total cannot be negative'],
  E_NO_PRINTER: ['No hay impresora configurada. Elígela en Ajustes.', 'No printer configured. Choose one in Settings.'],
  E_PRINT: ['La impresora no respondió', 'The printer did not respond'],
  E_BARCODE: ['Ese código de barras ya lo usa otro producto', 'That barcode is already used by another product'],
  E_USER_EXISTS: ['Ya existe un usuario con ese nombre', 'A user with that name already exists'],
  E_LAST_ADMIN: ['Debe quedar al menos un administrador activo', 'At least one active administrator must remain'],
  E_SETUP_DONE: ['La configuración inicial ya se hizo', 'Setup has already been completed'],
  E_DB: ['No se pudo abrir la base de datos', 'The database could not be opened'],
};

const fields: Record<string, [string, string]> = {
  name: ['nombre', 'name'],
  price: ['precio', 'price'],
  qty: ['cantidad', 'quantity'],
  vat: ['IVA', 'VAT'],
  password: ['contraseña (mínimo 4 caracteres)', 'password (at least 4 characters)'],
  barcode: ['código de barras', 'barcode'],
  shopName: ['nombre del comercio', 'shop name'],
  logo: ['imagen', 'image'],
  range: ['fechas', 'dates'],
};

export function errorText(e: unknown): string {
  const err = e instanceof ApiError ? e : new ApiError('E_INTERNAL', String(e));
  if (err.code === 'E_INVALID') {
    const f = fields[err.detail];
    return tr('Revisa el campo: ', 'Check the field: ') + (f ? tr(f[0], f[1]) : err.detail);
  }
  const known = errors[err.code];
  if (!known) return err.detail || err.code;
  let text = tr(known[0], known[1]).replace('{s}', err.detail);
  if (err.code === 'E_PRINT' || err.code === 'E_DB') text += ` (${err.detail})`;
  return text;
}
