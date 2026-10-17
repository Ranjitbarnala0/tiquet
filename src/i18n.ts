import { ApiError } from './api';
import { dateTime } from './format';

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
  E_SESSION_IDLE: ['La caja se bloqueó porque nadie la usaba. Vuelve a entrar.', 'The till locked because nobody was using it. Sign in again.'],
  E_FORBIDDEN: ['Solo un administrador puede hacer esto', 'Only an administrator can do this'],
  E_NOT_FOUND: ['No encontrado', 'Not found'],
  E_EMPTY: ['El ticket está vacío', 'The ticket is empty'],
  E_TENDERED: ['El importe entregado no cubre el total', 'The amount received does not cover the total'],
  E_TENDERED_HIGH: ['El importe entregado es demasiado alto: revisa la cantidad', 'The amount received is too high: check it'],
  E_ZERO_PRICE: ['Hay un artículo sin precio', 'An item has no price'],
  E_NEGATIVE_VAT: [
    'El descuento es mayor que lo vendido con su tipo de IVA. Ponlo junto al artículo que rebaja.',
    'The discount is bigger than what was sold at its VAT rate. Put it next to the item it reduces.',
  ],
  E_OVER_LIMIT: [
    'Más de 3.000 €: hace falta una factura con los datos del cliente.',
    'Over 3,000 EUR: this needs an invoice with the customer details.',
  ],
  E_TRAINING: [
    'En modo prueba solo se devuelven ventas de prueba, y fuera de él solo ventas reales.',
    'In training mode only practice sales can be returned, and outside it only real ones.',
  ],
  E_CASH_OPEN: ['La caja ya está abierta', 'The till is already open'],
  E_CASH_CLOSED: ['La caja no está abierta', 'The till is not open'],
  E_TOTAL_CHANGED: ['El ticket ha cambiado mientras cobrabas: revisa el total', 'The ticket changed while you were charging: check the total'],
  E_NEGATIVE: ['El total no puede ser negativo', 'The total cannot be negative'],
  E_REFUND_INVALID: ['Este ticket no admite devolución', 'This ticket cannot be returned'],
  E_REFUND_QTY: ['No se pueden devolver más unidades de las que quedan en el ticket', 'You cannot return more units than are left on the ticket'],
  E_REFUND_EMPTY: ['Elige al menos un artículo para devolver', 'Pick at least one item to return'],
  E_HAS_REFUNDS: [
    'Este ticket tiene devoluciones: anula antes las devoluciones o haz otra devolución.',
    'This ticket has returns: void those returns first, or make another return.',
  ],
  E_VOID_OLD: [
    'Solo se anulan tickets de hoy. Para un ticket de otro día, haz una devolución.',
    'Only today\'s tickets can be voided. For an earlier ticket, make a return.',
  ],
  E_REASON: ['Escribe el motivo', 'Enter a reason'],
  E_NO_PRINTER: ['No hay impresora configurada. Elígela en Ajustes.', 'No printer configured. Choose one in Settings.'],
  E_PRINTER_MISSING: ['La impresora «{s}» no está instalada en este equipo', 'The printer "{s}" is not installed on this computer'],
  E_PRINTER_VIRTUAL: [
    '«{s}» es una impresora virtual (PDF, XPS…) y no imprime tickets. Elige la impresora de tickets en Ajustes.',
    '"{s}" is a virtual printer (PDF, XPS…) and cannot print tickets. Choose the receipt printer in Settings.',
  ],
  E_PRINT: ['No se imprimió', 'Not printed'],
  E_BARCODE: ['Ese código de barras ya lo usa otro producto', 'That barcode is already used by another product'],
  E_USER_EXISTS: ['Ya existe un usuario con ese nombre', 'A user with that name already exists'],
  E_LAST_ADMIN: ['Debe quedar al menos un administrador activo', 'At least one active administrator must remain'],
  E_SETUP_DONE: ['La configuración inicial ya se hizo', 'Setup has already been completed'],
  E_RECOVERY: ['El código de recuperación no es correcto', 'The recovery code is not correct'],
  E_SELF: [
    'No puedes quitarte tu propio acceso de administrador. Pídeselo a otro administrador.',
    'You cannot remove your own administrator access. Ask another administrator.',
  ],
  E_DB: ['Error de la base de datos', 'Database error'],
  E_IO: ['No se pudo leer o escribir un archivo', 'A file could not be read or written'],
  E_INTERNAL: ['Error interno de Tiquet', 'Tiquet internal error'],
  E_DB_CORRUPT: ['La base de datos está dañada. Restaura una copia de seguridad.', 'The database is damaged. Restore a backup.'],
  E_DB_NEWER: [
    'Estos datos son de una versión más nueva de Tiquet. Instala la última versión.',
    'This data comes from a newer version of Tiquet. Install the latest version.',
  ],
  E_DATA_DIR: ['Windows no indica dónde guardar los datos (carpeta AppData).', 'Windows does not say where to keep data (AppData folder).'],
  E_BACKUP_INVALID: ['Ese archivo no es una copia de Tiquet que se pueda restaurar', 'That file is not a Tiquet backup that can be restored'],
  E_CLOCK: ['La fecha del ordenador es anterior al último ticket ({s})', 'The computer date is earlier than the last ticket ({s})'],
};

const fields: Record<string, [string, string]> = {
  name: ['nombre', 'name'],
  price: ['precio', 'price'],
  qty: ['cantidad', 'quantity'],
  vat: ['IVA', 'VAT'],
  password: ['contraseña (mínimo 4 caracteres)', 'password (at least 4 characters)'],
  adminPassword: ['contraseña de administrador (mínimo 6 caracteres)', 'administrator password (at least 6 characters)'],
  barcode: ['código de barras', 'barcode'],
  shopName: ['nombre del comercio', 'shop name'],
  logo: ['imagen', 'image'],
  range: ['fechas', 'dates'],
  role: ['rol', 'role'],
  method: ['forma de pago', 'payment method'],
  lines: ['líneas del ticket (máximo 500)', 'ticket lines (500 at most)'],
  saleId: ['identificador de la venta', 'sale identifier'],
  nif: ['NIF del comercio (revisa la letra)', 'shop NIF (check the letter)'],
  customerName: ['nombre del cliente', 'customer name'],
  customerNif: ['NIF del cliente (revisa la letra)', 'customer NIF (check the letter)'],
  amount: ['importe', 'amount'],
};

export function errorText(e: unknown): string {
  const err = e instanceof ApiError ? e : new ApiError('E_INTERNAL', String(e));
  if (err.code === 'E_INVALID') {
    const f = fields[err.detail];
    return tr('Revisa el campo: ', 'Check the field: ') + (f ? tr(f[0], f[1]) : err.detail);
  }
  const known = errors[err.code];
  if (!known) return err.detail || err.code;
  if (err.code === 'E_PRINT') return printText(err.detail);
  const detail = err.code === 'E_CLOCK' ? dateTime(err.detail) : err.detail;
  let text = tr(known[0], known[1]).replace('{s}', detail);
  if (['E_DB', 'E_IO', 'E_INTERNAL'].includes(err.code) && err.detail) text += ` (${err.detail})`;
  return text;
}

const printWhy: Record<string, [string, string]> = {
  offline: ['la impresora está apagada o desconectada', 'the printer is off or disconnected'],
  paper: ['falta papel o hay un atasco', 'it is out of paper or jammed'],
  paused: ['la impresora está en pausa en Windows', 'the printer is paused in Windows'],
  error: ['la impresora indica un error', 'the printer reports an error'],
  stalled: ['la impresora no empezó a imprimir', 'the printer never started printing'],
  cancelled: ['se canceló en Windows', 'it was cancelled in Windows'],
  busy: ['la impresora estaba ocupada con otro ticket', 'the printer was busy with another ticket'],
};

// What happened to a print, and whether anything may be on paper. Nothing is
// ever left waiting in the Windows queue: a job that could not print is
// cancelled, so it cannot come out later on its own.
export function printText(detail: string): string {
  if (detail === 'timeout') {
    return tr(
      'No se sabe si se imprimió: Windows no contesta. Mira si salió antes de repetir; si se repite, saldrá como COPIA.',
      'Unknown whether it printed: Windows is not answering. Check before trying again; a repeat prints as a COPY.',
    );
  }
  if (detail.startsWith('partial-')) {
    const why = printWhy[detail.slice(8)];
    return `${tr('Se cortó a medias', 'It stopped halfway')}${why ? ` (${tr(why[0], why[1])})` : ''}. ${tr('Al repetir saldrá como COPIA.', 'A repeat prints as a COPY.')}`;
  }
  const why = printWhy[detail];
  return why ? `${tr('No se imprimió', 'Not printed')}: ${tr(why[0], why[1])}.` : `${tr('No se imprimió', 'Not printed')} (${detail}).`;
}

// a failed print worth retrying, as opposed to a printer that is not set up
export function isTransient(e: unknown): boolean {
  return e instanceof ApiError && e.code === 'E_PRINT';
}

export function isPrinterSetup(e: unknown): boolean {
  return e instanceof ApiError && ['E_NO_PRINTER', 'E_PRINTER_MISSING', 'E_PRINTER_VIRTUAL'].includes(e.code);
}

const warnings: Record<string, [string, string]> = {
  W_PRINT_SLOW: ['La impresora va lenta: el ticket sigue saliendo.', 'The printer is slow: the ticket is still printing.'],
};

export function warningText(code: string): string {
  const w = warnings[code];
  return w ? tr(w[0], w[1]) : code;
}

export type PrinterState = 'none' | 'missing' | 'virtual' | 'ready' | 'offline' | 'paper' | 'paused' | 'error' | 'unknown';

export function printerStateText(s: PrinterState): string {
  const t: Record<PrinterState, [string, string]> = {
    none: ['Sin impresora', 'No printer'],
    missing: ['Impresora no encontrada', 'Printer not found'],
    virtual: ['Impresora no válida', 'Not a receipt printer'],
    ready: ['Impresora lista', 'Printer ready'],
    offline: ['Impresora desconectada', 'Printer offline'],
    paper: ['Revisa el papel', 'Check the paper'],
    paused: ['Impresora en pausa', 'Printer paused'],
    error: ['Error de impresora', 'Printer error'],
    unknown: ['Impresora sin respuesta', 'Printer not answering'],
  };
  return tr(t[s][0], t[s][1]);
}
