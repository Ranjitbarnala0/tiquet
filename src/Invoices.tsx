import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { Ban, Building2, Check, FileDown, FilePlus2, FileText, Pencil, Plus, Printer, ReceiptText, Search, Share2, Tag, Trash2, TriangleAlert, UserRound } from 'lucide-react';
import { call, toError } from './api';
import { dateTime, eur, fmt, isoDate, parseMoney, rate, VAT_RATES } from './format';
import { errorText, isPrinterSetup, isTransient, tr, warningText } from './i18n';
import { cardMachines } from './machines';
import { customerNif } from './nif';
import { Paper } from './Paper';
import { PrinterPick, type OneOff } from './PrinterPick';
import { ShareDialog } from './Share';
import { SheetView, type Sheet } from './Sheets';
import { useToast } from './toast';
import type { Finalized, Logo, Method, PLine, Product, SaleLine, Settings, TicketRecord, User } from './types';
import { Empty, Field, Modal, Segmented } from './ui';

// Facturas: bills made on their own page. A simple bill needs nothing from
// the customer (a factura simplificada); with the customer's name and NIF it
// is a full invoice. Either comes out on A4 or as a ticket, on the printer
// chosen. A mistake is put right by editing the bill: the corrected one
// prints clean, and Tiquet keeps which one it replaced.

interface BillRow {
  id: number;
  kind: 'sale' | 'refund';
  number: string;
  issuedAt: string;
  userName: string;
  totalCents: number;
  paymentMethod: Method;
  voided: boolean;
  customerName: string;
  customerNif: string;
  voidReason: string | null;
  replaces: string | null;
}

interface KnownCustomer {
  name: string;
  nif: string;
  address: string;
  last: string;
}

interface LineDraft {
  key: number;
  name: string;
  imei: string;
  qty: string;
  price: string;
  vatBp: number;
  productId?: number;
  // a discount: what is typed is taken off, whatever its sign
  discount?: boolean;
  // the exact price with VAT the line came with (the catalogue, the bill
  // being corrected, or typed with VAT before switching), used while its box
  // still shows `exactFor`: going to prices without VAT and back, or opening
  // a bill to fix a NIF, never moves a cent
  exact?: number;
  exactFor?: string;
}

type Format = 'a4' | 'ticket';

const IMEI = ' · IMEI ';
const SIMPLIFIED_LIMIT = 300_000;

const newId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

let keys = 0;
const blank = (vatBp: number): LineDraft => ({ key: ++keys, name: '', imei: '', qty: '1', price: '', vatBp });

const METHOD: Record<Method, () => string> = {
  cash: () => tr('Efectivo', 'Cash'),
  card: () => tr('Tarjeta', 'Card'),
  bizum: () => 'Bizum',
};

// the price with VAT a line is sold at, from what was typed
const gross = (cents: number, vatBp: number, net: boolean) => (net ? Math.round((cents * (10_000 + vatBp)) / 10_000) : cents);
const netOf = (cents: number, vatBp: number) => Math.round((cents * 10_000) / (10_000 + vatBp));
const unitOf = (l: LineDraft, cents: number, net: boolean) => {
  const unit = l.exact !== undefined && l.price === l.exactFor ? l.exact : gross(cents, l.vatBp, net);
  return l.discount ? -Math.abs(unit) : unit;
};
const nameOf = (l: LineDraft) => (l.imei.trim() ? `${l.name.trim()}${IMEI}${l.imei.trim()}` : l.name.trim());

// the format a bill starts in: the one chosen in Ajustes, or by itself A4
// when this PC has an A4 printer and a ticket when it has none
function startFormat(s: Settings): Format {
  if (s.invoiceFormat === 'a4' || s.invoiceFormat === 'ticket') return s.invoiceFormat;
  return (s.invoicePrinter ?? '').trim() ? 'a4' : 'ticket';
}

// A bill being typed, kept on this PC until it is issued: it comes back after
// an update, a restart or a power cut. Half a day old at most: yesterday's is
// nobody's bill any more.
interface Draft {
  at: number;
  // the bill's own id: sent again after a restart, a bill already saved is
  // answered with itself instead of being issued twice
  saleId?: string;
  full: boolean;
  customer: { name: string; nif: string; address: string };
  lines: LineDraft[];
  method: Method;
  showVat: boolean;
  // someone ticked or unticked the box: it no longer follows the bill type
  vatChosen?: boolean;
  net: boolean;
  note: string;
}

function readDraft(key?: string): Draft | null {
  if (!key) return null;
  try {
    const d = JSON.parse(localStorage.getItem(key) ?? 'null') as Draft | null;
    return d && Array.isArray(d.lines) && d.lines.length > 0 && Date.now() - d.at < 12 * 3_600_000 ? d : null;
  } catch {
    return null;
  }
}

// Whether a bill already issued shows its VAT. Since 1.13 it says so itself;
// before, an invoice with a customer did and a simple one followed Ajustes.
function shownVat(t: TicketRecord, s: Settings) {
  return t.shop.vat ? t.shop.vat === 'full' : !!t.customer || !!s.invoiceVatTable;
}

// A printer picked for an A4 bill on a PC that has none set up becomes that
// PC's A4 printer, so nobody has to find Ajustes for it.
async function keepA4(picked: OneOff | null, format: Format, settings: Settings, onSettings: ((s: Settings) => void) | undefined, say: (text: string) => void) {
  if (format !== 'a4' || !picked || (settings.invoicePrinter ?? '').trim()) return;
  try {
    const r = await call<{ applied: boolean; settings?: Settings }>('printers.a4', { name: picked.name });
    if (!r.applied || !r.settings) return;
    onSettings?.(r.settings);
    say(tr(`Desde ahora las facturas en A4 de este PC salen por «${picked.name}». Se cambia en Ajustes › Facturas.`, `From now on this PC's A4 invoices print on "${picked.name}". Change it in Settings › Invoices.`));
  } catch {
    // only a convenience: the bill itself is printed
  }
}

// What the core lets each one void (tickets.rs void_in): the owner any day
// while VERI*FACTU is off, otherwise today; a cashier their own of today
// when Ajustes lets them; never across practice and real.
function mayVoid(t: TicketRecord, user: User, settings: Settings) {
  if (t.voidedAt || !!t.training !== !!settings.training) return false;
  const today = t.issuedAt.slice(0, 10) === isoDate(new Date());
  if (user.role === 'admin') return today || settings.verifactu === 'off';
  return settings.bookStaffDelete && t.userName === user.name && today;
}

// and what a correction may replace (tickets.rs fixable): a sale of this
// year with no repair handed back and no live return on it. Anything else is
// put right with a return in Hoy.
function canFix(t: TicketRecord, user: User, settings: Settings) {
  return (
    mayVoid(t, user, settings) &&
    t.kind === 'sale' &&
    !t.lines.some((l) => l.repairId) &&
    !t.refunds.some((r) => !r.voided) &&
    t.issuedAt.slice(0, 4) === String(new Date().getFullYear())
  );
}

interface PageProps {
  user: User;
  settings: Settings;
  products: Product[];
  logo: Logo | null;
  onSetupPrinter?: () => void;
  // this PC's settings changed here (its A4 printer, kept from a pick)
  onSettings?: (s: Settings) => void;
  // where the bill being typed is kept
  draftKey?: string;
  // a bill to correct, sent from Hoy
  fix?: TicketRecord | null;
  onFixTaken?: () => void;
  // false while another screen shows: everything typed stays
  active?: boolean;
}

export function Invoices({ user, settings, products, logo, onSetupPrinter, onSettings, draftKey, fix, onFixTaken, active = true }: PageProps) {
  const [view, setView] = useState<'new' | 'list'>('new');
  const [fixing, setFixing] = useState<TicketRecord | null>(fix ?? null);
  useEffect(() => {
    if (!fix) return;
    setFixing(fix);
    setView('new');
    onFixTaken?.();
  }, [fix]);
  const [fresh, setFresh] = useState(0);
  const [selId, setSelId] = useState<number | null>(null);
  const [reload, setReload] = useState(0);
  const [count, setCount] = useState<number | null>(null);

  // back from another screen, the issued list is read again
  const wasActive = useRef(active);
  useEffect(() => {
    if (active && !wasActive.current) setReload((n) => n + 1);
    wasActive.current = active;
  }, [active]);

  const done = (t: TicketRecord) => {
    setFixing(null);
    setFresh((n) => n + 1);
    setSelId(t.id);
    setReload((n) => n + 1);
    setView('list');
  };

  return (
    <div className="bills" hidden={!active}>
      <div className="bills-head">
        <h1>{tr('Facturas', 'Invoices')}</h1>
        <div className="bills-views" role="tablist">
          <button role="tab" aria-selected={view === 'new'} className={view === 'new' ? 'on' : ''} onClick={() => setView('new')}>
            <FilePlus2 size={16} />
            {fixing ? tr(`Corrigiendo ${fixing.number}`, `Fixing ${fixing.number}`) : tr('Nueva factura', 'New invoice')}
          </button>
          <button role="tab" aria-selected={view === 'list'} className={view === 'list' ? 'on' : ''} onClick={() => setView('list')}>
            <FileText size={16} />
            {tr('Emitidas', 'Issued')}
            {count !== null && <span className="bills-count">{count}</span>}
          </button>
        </div>
      </div>
      <div className="bills-body">
        <div hidden={view !== 'new' || !!fixing} className="bills-pane">
          <BillEditor key={`new-${fresh}`} settings={settings} products={products} logo={logo} onDone={done} onSetupPrinter={onSetupPrinter} onSettings={onSettings} draftKey={draftKey} />
        </div>
        {fixing && (
          <div hidden={view !== 'new'} className="bills-pane">
            <BillEditor
              key={`fix-${fixing.id}`}
              from={fixing}
              settings={settings}
              products={products}
              logo={logo}
              onDone={done}
              onCancel={() => {
                setFixing(null);
                setView('list');
              }}
              onSetupPrinter={onSetupPrinter}
              onSettings={onSettings}
            />
          </div>
        )}
        <div hidden={view !== 'list'} className="bills-pane">
          <BillList
            user={user}
            settings={settings}
            selId={selId}
            setSelId={setSelId}
            reload={reload}
            onCount={setCount}
            onFix={(t) => {
              setFixing(t);
              setView('new');
            }}
            onNew={() => setView('new')}
            made={fresh}
            onSettings={onSettings}
            onSetupPrinter={onSetupPrinter}
          />
        </div>
      </div>
    </div>
  );
}

// ------------------------------------------------------------- the editor

interface EditorProps {
  settings: Settings;
  products: Product[];
  logo: Logo | null;
  from?: TicketRecord | null;
  onDone: (t: TicketRecord) => void;
  onCancel?: () => void;
  onSetupPrinter?: () => void;
  onSettings?: (s: Settings) => void;
  draftKey?: string;
}

function linesOf(t: TicketRecord, net: boolean): LineDraft[] {
  return t.lines
    .filter((l) => l.qty > 0)
    .map((l) => {
      const at = l.name.indexOf(IMEI);
      // a line that takes off shows its amount without the sign
      const shown = Math.abs(l.unitCents);
      const price = fmt(net ? netOf(shown, l.vatBp) : shown);
      return {
        key: ++keys,
        name: at < 0 ? l.name : l.name.slice(0, at),
        imei: at < 0 ? '' : l.name.slice(at + IMEI.length),
        qty: String(l.qty),
        price,
        vatBp: l.vatBp,
        productId: l.productId ?? undefined,
        discount: l.unitCents < 0 || undefined,
        exact: shown,
        exactFor: price,
      };
    });
}

function BillEditor({ settings, products, logo, from, onDone, onCancel, onSetupPrinter, onSettings, draftKey }: EditorProps) {
  const toast = useToast();
  const vat = settings.defaultVatBp;
  const [draft] = useState(() => (from ? null : readDraft(draftKey)));
  // The bill shows its VAT (base, rate and VAT of each rate) or says nothing
  // of it: then it is items, prices as typed and a total. Prices without VAT
  // only make sense on a bill that shows it.
  const [start] = useState(() => {
    if (draft) return { shown: !!draft.showVat, net: !!draft.showVat && !!draft.net };
    const shown = from ? shownVat(from, settings) : !!settings.invoiceShowVat;
    return { shown, net: shown && !!settings.invoiceNetPrices };
  });
  const [showVat, setShowVat] = useState(start.shown);
  // once someone has ticked or unticked the box, it is theirs
  const vatChosen = useRef(draft ? !!draft.vatChosen : !!from);
  const [net, setNet] = useState(start.net);
  const [note, setNote] = useState(draft?.note ?? from?.note ?? '');
  const [full, setFull] = useState(draft ? !!draft.full : !!from?.customer);
  const [customer, setCustomer] = useState(() => draft?.customer ?? from?.customer ?? { name: '', nif: '', address: '' });
  const [lines, setLines] = useState<LineDraft[]>(() => {
    const had = draft ? draft.lines.map((l) => ({ ...l, key: ++keys })) : from ? linesOf(from, start.net) : [];
    return had.length ? had : [blank(vat)];
  });
  const [method, setMethod] = useState<Method>(draft?.method ?? from?.payment.method ?? 'cash');
  const cards = useMemo(() => cardMachines(settings), [settings]);
  const [machine, setMachine] = useState<number>(() => from?.payment.machine ?? (cards.some((m) => m.id === settings.cardMachine) ? settings.cardMachine : (cards[0]?.id ?? 0)));
  // A4 or ticket: what Ajustes says (by itself, A4 as soon as this PC has an
  // A4 printer) until someone chooses here
  const [formatPick, setFormatPick] = useState<Format | null>(null);
  const format = formatPick ?? startFormat(settings);
  const [printer, setPrinter] = useState<OneOff | null>(null);
  // a printer picked for one paper is not for the other
  useEffect(() => setPrinter(null), [format]);
  const [print, setPrint] = useState(true);
  const [busy, setBusy] = useState(false);
  // one id per bill, so a retry after a lost answer never issues it twice
  const saleId = useRef(draft?.saleId ?? newId());
  // goes up when the bill takes another id, so the draft is kept with it
  const [idTaken, setIdTaken] = useState(0);
  const [sheets, setSheets] = useState<Sheet[]>([]);
  const [ticket, setTicket] = useState<PLine[]>([]);
  const [number, setNumber] = useState('');
  const [previewing, setPreviewing] = useState(false);
  const [known, setKnown] = useState<KnownCustomer[]>([]);
  const [suggest, setSuggest] = useState(false);
  const seq = useRef(0);
  const form = useRef<HTMLDivElement>(null);

  const catalogue = useMemo(() => new Map(products.filter((p) => p.active).map((p) => [p.name.toLowerCase(), p])), [products]);
  const set = (key: number, patch: Partial<LineDraft>) => setLines((cur) => cur.map((l) => (l.key === key ? { ...l, ...patch } : l)));
  // Another line, with the cursor in it: the button under the lines, or
  // Enter on the last price.
  const [fresh, setFresh] = useState<{ key: number; field: 'name' | 'price' } | null>(null);
  const add = (line?: Partial<LineDraft>, field: 'name' | 'price' = 'name') => {
    const l = { ...blank(vat), ...line };
    setLines((cur) => [...cur, l]);
    setFresh({ key: l.key, field });
  };
  useEffect(() => {
    if (!fresh) return;
    focus(`line-${fresh.key}-${fresh.field}`);
    setFresh(null);
  }, [fresh]);
  // Enter walks a line (to its price) and from the last price to the next
  // line, making one when there is none
  const onKey = (e: KeyboardEvent, l: LineDraft, field: 'name' | 'imei' | 'qty' | 'price') => {
    if (e.key !== 'Enter' || e.nativeEvent.isComposing) return;
    e.preventDefault();
    if (field !== 'price') return focus(`line-${l.key}-price`);
    const next = lines[lines.findIndex((x) => x.key === l.key) + 1];
    if (next) focus(`line-${next.key}-name`);
    else if (l.name.trim() || l.price.trim()) add();
  };
  const named = (key: number, name: string) => {
    const p = catalogue.get(name.trim().toLowerCase());
    if (!p) return set(key, { name, productId: undefined });
    const price = fmt(net ? netOf(p.priceCents, p.vatBp) : p.priceCents);
    // (a product is never a discount, whatever button made the line)
    set(key, { name, productId: p.id, price, vatBp: p.vatBp, exact: p.priceCents, exactFor: price, discount: undefined });
  };
  const remove = (key: number) => setLines((cur) => (cur.length > 1 ? cur.filter((l) => l.key !== key) : [blank(vat)]));

  // switching between prices with and without VAT keeps what each line
  // costs, to the cent
  const switchNet = (on: boolean) => {
    setLines((cur) =>
      cur.map((l) => {
        const c = parseMoney(l.price);
        if (c === null || !l.price.trim()) return l;
        // (a discount keeps showing its amount without the sign)
        const unit = unitOf(l, c, net);
        const exact = l.discount ? Math.abs(unit) : unit;
        const price = fmt(on ? netOf(exact, l.vatBp) : exact);
        return { ...l, price, exact, exactFor: price };
      }),
    );
    setNet(on);
  };
  const toggleVat = (on: boolean) => {
    if (!on && net) switchNet(false);
    setShowVat(on);
  };
  // An invoice made out to someone (a business, a self-employed customer)
  // starts with its VAT shown, which they need; a bill with no details
  // starts as Ajustes says. Only until the box is touched.
  const setKind = (withCustomer: boolean) => {
    setFull(withCustomer);
    if (!vatChosen.current) toggleVat(withCustomer || !!settings.invoiceShowVat);
  };

  // a line can be a discount (a negative price), never zero
  const parsed = lines.map((l) => ({ l, qty: Number.parseInt(l.qty, 10), cents: parseMoney(l.price) }));
  const filled = parsed.filter((p) => p.l.name.trim() || p.l.price.trim());
  const long = (p: (typeof parsed)[number]) => [...nameOf(p.l)].length > 120;
  const bad = (p: (typeof parsed)[number]) => !p.l.name.trim() || !Number.isFinite(p.qty) || p.qty < 1 || p.qty > 999 || p.cents === null || p.cents === 0 || long(p);
  const out: SaleLine[] = filled
    .filter((p) => !bad(p))
    .map((p) => ({
      name: nameOf(p.l),
      qty: p.qty,
      unitCents: unitOf(p.l, p.cents ?? 0, net),
      vatBp: p.l.vatBp,
      productId: p.l.productId,
    }));
  const total = out.reduce((s, l) => s + l.qty * l.unitCents, 0);
  const who = full && customer.name.trim() ? { name: customer.name.trim(), nif: customer.nif.trim(), address: customer.address.trim() } : undefined;

  // what is typed is kept until the bill is issued
  const typed = lines.some((l) => l.name.trim() || l.price.trim() || l.imei.trim()) || (full && !!(customer.name.trim() || customer.nif.trim() || customer.address.trim())) || !!note.trim();
  useEffect(() => {
    if (!draftKey || from) return;
    try {
      if (typed) localStorage.setItem(draftKey, JSON.stringify({ at: Date.now(), saleId: saleId.current, full, customer, lines, method, showVat, vatChosen: vatChosen.current, net, note } satisfies Draft));
      else localStorage.removeItem(draftKey);
    } catch {
      // a full disk loses only the draft
    }
  }, [draftKey, from, typed, full, customer, lines, method, showVat, net, note, idTaken]);

  // what is still needed, said in words next to the button
  const missing: { text: string; focus: string }[] = [];
  if (full) {
    if (!customer.name.trim()) missing.push({ text: tr('el nombre del cliente', "the customer's name"), focus: 'bill-name' });
    if (!customerNif(customer.nif)) missing.push({ text: customer.nif.trim() ? tr('un NIF válido del cliente', "a valid customer NIF") : tr('el NIF del cliente', "the customer's NIF"), focus: 'bill-nif' });
    if (!settings.shopNif.trim()) missing.push({ text: tr('el NIF de la tienda (Ajustes › Comercio)', "the shop's NIF (Settings › Shop)"), focus: '' });
  }
  if (filled.length === 0) missing.push({ text: tr('lo que se vende', 'what is sold'), focus: `line-${lines[0]?.key}-name` });
  filled.forEach((p, i) => {
    if (long(p)) missing.push({ text: tr(`la línea ${i + 1} más corta (nombre e IMEI, 120 letras)`, `line ${i + 1} shorter (name and IMEI, 120 letters)`), focus: `line-${p.l.key}-name` });
    else if (bad(p)) missing.push({ text: tr(`la línea ${i + 1} (nombre, cantidad y precio)`, `line ${i + 1} (name, quantity and price)`), focus: `line-${p.l.key}-${p.l.name.trim() ? 'price' : 'name'}` });
  });
  if (out.length > 0 && total <= 0) missing.push({ text: tr('un total mayor que cero', 'a total above zero'), focus: '' });
  if (!full && total > SIMPLIFIED_LIMIT) missing.push({ text: tr('los datos del cliente: pasa de 3.000 €', "the customer's details: over 3,000 EUR"), focus: '' });

  // the page as it will come out, a moment after each change
  const previewKey = JSON.stringify({ format, out, who, method, replaces: from?.id, showVat, note: note.trim() });
  useEffect(() => {
    const n = ++seq.current;
    setPreviewing(true);
    const t = window.setTimeout(() => {
      const args = JSON.parse(previewKey);
      const req =
        args.format === 'a4'
          ? call<{ sheets: Sheet[]; number: string }>('invoices.preview', { lines: args.out, customer: args.who, payment: { method: args.method }, replaces: args.replaces, showVat: args.showVat, note: args.note }).then((r) => {
              if (n !== seq.current) return;
              setSheets(r.sheets);
              setNumber(r.number);
            })
          : call<{ lines: PLine[]; number: string }>('sale.preview', { lines: args.out, customer: args.who, payment: { method: args.method }, replaces: args.replaces, showVat: args.showVat, note: args.note }).then((r) => {
              if (n !== seq.current) return;
              setTicket(r.lines);
              setNumber(r.number);
            });
      req.catch(() => {}).finally(() => n === seq.current && setPreviewing(false));
    }, 250);
    return () => window.clearTimeout(t);
  }, [previewKey]);

  // earlier customers, as the name is typed
  useEffect(() => {
    if (!full || !suggest) return;
    const q = customer.name.trim();
    const t = window.setTimeout(() => {
      call<KnownCustomer[]>('invoices.customers', { q })
        .then(setKnown)
        .catch(() => setKnown([]));
    }, 150);
    return () => window.clearTimeout(t);
  }, [full, suggest, customer.name]);

  const focus = (id: string) => {
    if (!id) return;
    const el = form.current?.querySelector<HTMLInputElement>(`[data-id="${id}"]`);
    el?.focus();
    el?.select?.();
    el?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  };

  const charge = async () => {
    if (busy) return;
    if (missing.length) {
      toast.error(`${tr('Falta', 'Missing')}: ${missing.map((m) => m.text).join(', ')}`);
      focus(missing.find((m) => m.focus)?.focus ?? '');
      return;
    }
    setBusy(true);
    try {
      const send = () =>
        call<Finalized>('sale.finalize', {
          lines: out,
          payment: { method, machine: method === 'card' ? machine : undefined },
          customer: full ? { name: customer.name.trim(), nif: customer.nif.trim(), address: customer.address.trim() } : undefined,
          saleId: saleId.current,
          expectedCents: total,
          print,
          bill: true,
          format,
          printer: printer ?? undefined,
          replaces: from?.id,
          showVat,
          note: note.trim() || undefined,
        });
      // changed after an answer that never came back: it is another bill,
      // and the one saved before is named so it can be voided if it is one
      // too many
      const r = await send().catch((e) => {
        const err = toError(e);
        if (err.code !== 'E_SALE_REUSED') throw e;
        saleId.current = newId();
        setIdTaken((n) => n + 1);
        toast.info(
          tr(
            `Antes se guardó ${err.detail || 'otra factura'} desde esta misma pantalla y su aviso no llegó. Mírala en Emitidas: si sobra, anúlala.`,
            `${err.detail || 'Another invoice'} was saved from this same screen earlier and its answer never arrived. Check it under Issued: void it if it is one too many.`,
          ),
          { sticky: true },
        );
        return send();
      });
      const what = from
        ? tr(`Guardada como ${r.ticket.number}. ${from.number} queda anotada en Tiquet como corregida.`, `Saved as ${r.ticket.number}. ${from.number} is noted in Tiquet as corrected.`)
        : r.duplicate
          ? tr(`Esta factura ya estaba guardada: ${r.ticket.number}`, `This invoice had already been saved: ${r.ticket.number}`)
          : `${tr('Factura', 'Invoice')} ${r.ticket.number}`;
      if (r.printError) {
        const e = toError(r.printError);
        const fix = isPrinterSetup(e) || e.code === 'E_NO_A4_PRINTER';
        toast.error(`${what} · ${tr('no se imprimió', 'not printed')}: ${errorText(e)}`, fix && onSetupPrinter ? { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } } : undefined);
      } else if (r.printWarning) toast.info(`${what} · ${warningText(r.printWarning)}`);
      else toast.ok(`${what}${print && !from ? ` · ${tr('impresa', 'printed')}` : ''}`);
      if (print && !r.printError && !r.printWarning) void keepA4(printer, format, settings, onSettings, toast.info);
      if (draftKey && !from) {
        try {
          localStorage.removeItem(draftKey);
        } catch {
          // nothing to do
        }
      }
      onDone(r.ticket);
    } catch (e) {
      toast.error(errorText(e));
      setBusy(false);
    }
  };

  const showImei = settings.invoiceImei;
  return (
    <div className="bill">
      <div className="bill-form" ref={form}>
        {from && (
          <div className="bill-fixing">
            <Pencil size={16} />
            <span>
              {tr(
                `Corrigiendo ${from.number}. Al guardar, la corregida sale limpia, sin ninguna marca, y en Tiquet queda anotado que sustituye a ${from.number}.`,
                `Fixing ${from.number}. When saved, the corrected one prints clean, with no mark, and Tiquet notes that it replaces ${from.number}.`,
              )}
            </span>
          </div>
        )}

        <section className="bill-card">
          <h3>{tr('¿Con datos del cliente?', "With the customer's details?")}</h3>
          <div className="bill-types">
            <button type="button" className={!full ? 'on' : ''} onClick={() => setKind(false)}>
              <ReceiptText size={20} />
              <span>
                <b>{tr('Sin datos', 'No details')}</b>
                <small>{tr('Factura simplificada. No pide nada al cliente.', 'Simplified invoice. Asks the customer for nothing.')}</small>
              </span>
              {!full && <Check size={18} className="bill-tick" />}
            </button>
            <button type="button" className={full ? 'on' : ''} onClick={() => setKind(true)}>
              <Building2 size={20} />
              <span>
                <b>{tr('Con nombre y NIF', 'With name and NIF')}</b>
                <small>{tr('Factura completa, para empresas o quien la pida.', 'Full invoice, for businesses or whoever asks.')}</small>
              </span>
              {full && <Check size={18} className="bill-tick" />}
            </button>
          </div>
          {full && (
            <div className="bill-customer">
              <div className="grid2">
                <Field label={tr('Nombre o razón social', 'Name or company')}>
                  <div className="bill-suggest">
                    <input
                      data-id="bill-name"
                      value={customer.name}
                      onChange={(e) => {
                        setCustomer({ ...customer, name: e.target.value });
                        setSuggest(true);
                      }}
                      onFocus={() => setSuggest(true)}
                      onBlur={() => window.setTimeout(() => setSuggest(false), 150)}
                      maxLength={120}
                      autoComplete="off"
                    />
                    {suggest && known.length > 0 && (
                      <div className="bill-known" role="listbox">
                        {known.map((k) => (
                          <button
                            key={`${k.nif}-${k.name}`}
                            type="button"
                            role="option"
                            aria-selected={false}
                            onMouseDown={(e) => e.preventDefault()}
                            onClick={() => {
                              setCustomer({ name: k.name, nif: k.nif, address: k.address });
                              setSuggest(false);
                            }}
                          >
                            <UserRound size={15} />
                            <span>
                              <b>{k.name}</b>
                              <small>
                                {k.nif} · {tr('última', 'last')} {dateTime(k.last)}
                              </small>
                            </span>
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                </Field>
                <Field label="NIF / CIF / NIE" error={customer.nif.trim() && !customerNif(customer.nif) ? tr('Revisa el NIF: la letra no coincide', 'Check the NIF: the letter does not match') : undefined}>
                  <input data-id="bill-nif" value={customer.nif} onChange={(e) => setCustomer({ ...customer, nif: e.target.value.toUpperCase() })} placeholder="B12345674" autoComplete="off" />
                </Field>
              </div>
              <Field label={tr('Dirección (opcional)', 'Address (optional)')}>
                <textarea rows={2} value={customer.address} onChange={(e) => setCustomer({ ...customer, address: e.target.value })} />
              </Field>
            </div>
          )}
        </section>

        <section className="bill-card">
          <div className="bill-card-head">
            <h3>{tr('Qué se vende', 'What is sold')}</h3>
            <label
              className="bill-check"
              title={tr('Marcada, la factura lleva la base, el tipo y la cuota de IVA. Sin marcar, solo artículos, precios y total.', 'Ticked, the invoice carries the net amount, the rate and the VAT. Unticked, only items, prices and the total.')}
            >
              <input
                data-id="bill-vat"
                type="checkbox"
                checked={showVat}
                onChange={(e) => {
                  vatChosen.current = true;
                  toggleVat(e.target.checked);
                }}
              />
              {tr('Mostrar el IVA en la factura', 'Show VAT on the invoice')}
            </label>
          </div>
          <datalist id="bill-products">
            {products
              .filter((p) => p.active)
              .slice(0, 400)
              .map((p) => (
                <option key={p.id} value={p.name} />
              ))}
          </datalist>
          <div className={`bill-lines${showImei ? ' imei' : ''}${showVat ? '' : ' novat'}`}>
            <div className="bill-line head">
              <span className="bl-name">{tr('Artículo', 'Item')}</span>
              <span className="bl-qty">{tr('Cant.', 'Qty')}</span>
              <span className="bl-price">{net ? tr('Precio sin IVA', 'Price excl. VAT') : tr('Precio', 'Price')}</span>
              {showVat && <span className="bl-vat">IVA</span>}
              <span className="bl-amount">{tr('Importe', 'Amount')}</span>
              <span className="bl-del" />
            </div>
            {lines.map((l, i) => {
              const p = parsed[i];
              const amount = p && !bad(p) ? p.qty * unitOf(l, p.cents ?? 0, net) : null;
              // the one line of an empty bill has nothing to remove
              const only = lines.length === 1 && !l.name.trim() && !l.price.trim() && !l.imei.trim();
              return (
                <div className="bill-line" key={l.key}>
                  <input
                    className="bl-name"
                    data-id={`line-${l.key}-name`}
                    list="bill-products"
                    value={l.name}
                    onChange={(e) => named(l.key, e.target.value)}
                    onKeyDown={(e) => onKey(e, l, 'name')}
                    placeholder={tr('Escribe o elige un artículo', 'Type or pick an item')}
                    maxLength={100}
                    aria-label={tr('Artículo', 'Item')}
                  />
                  {showImei && (
                    <input
                      className="bl-imei"
                      value={l.imei}
                      onChange={(e) => set(l.key, { imei: e.target.value.replace(/\s+/g, '') })}
                      onKeyDown={(e) => onKey(e, l, 'imei')}
                      placeholder={tr('IMEI o nº de serie (opcional)', 'IMEI or serial (optional)')}
                      maxLength={40}
                      aria-label="IMEI"
                    />
                  )}
                  <input className="num bl-qty" value={l.qty} onChange={(e) => set(l.key, { qty: e.target.value.replace(/\D/g, '') })} onKeyDown={(e) => onKey(e, l, 'qty')} inputMode="numeric" aria-label={tr('Cantidad', 'Quantity')} />
                  <div className="money-input bl-price">
                    <input data-id={`line-${l.key}-price`} className="num" value={l.price} onChange={(e) => set(l.key, { price: e.target.value })} onKeyDown={(e) => onKey(e, l, 'price')} inputMode="decimal" placeholder="0,00" aria-label={tr('Precio', 'Price')} />
                    <span>€</span>
                  </div>
                  {showVat && (
                    <select className="bl-vat" value={l.vatBp} onChange={(e) => set(l.key, { vatBp: Number(e.target.value), exact: undefined })} aria-label="IVA">
                      {VAT_RATES.map((v) => (
                        <option key={v} value={v}>
                          {rate(v)}
                        </option>
                      ))}
                    </select>
                  )}
                  <b className="num bl-amount">{amount === null ? (l.discount ? '−' : '') : fmt(amount)}</b>
                  {only ? (
                    <span className="bl-del" />
                  ) : (
                    <button type="button" className="icon-btn bl-del" onClick={() => remove(l.key)} aria-label={tr('Quitar la línea', 'Remove the line')} title={tr('Quitar la línea', 'Remove the line')}>
                      <Trash2 size={16} />
                    </button>
                  )}
                </div>
              );
            })}
          </div>
          <div className="bill-more">
            <button type="button" className="btn bill-add" data-id="bill-add" onClick={() => add()}>
              <Plus size={16} />
              {tr('Añadir artículo', 'Add item')}
            </button>
            <button
              type="button"
              className="btn btn-ghost"
              onClick={() => add({ name: tr('Descuento', 'Discount'), discount: true, vatBp: lines[lines.length - 1]?.vatBp ?? vat }, 'price')}
              title={tr('Una línea que resta: escribe cuánto', 'A line that takes off: type how much')}
            >
              <Tag size={15} />
              {tr('Descuento', 'Discount')}
            </button>
            {showVat && (
              <label className="bill-check">
                <input type="checkbox" checked={!net} onChange={(e) => switchNet(!e.target.checked)} />
                {tr('Precios con IVA incluido', 'Prices include VAT')}
              </label>
            )}
          </div>
          {showVat && (
            <p className="bill-hint">
              {net
                ? tr(`Los precios se escriben sin IVA: se añade el de cada línea (${rate(vat)} si no se cambia).`, `Prices are typed without VAT: each line's rate is added (${rate(vat)} unless changed).`)
                : tr(`Los precios ya llevan el IVA: ${rate(vat)} si no se cambia en la línea.`, `Prices already include VAT: ${rate(vat)} unless the line says otherwise.`)}
            </p>
          )}
          {full && !showVat && <p className="bill-hint">{tr('Para una empresa o un autónomo, marca «Mostrar el IVA»: lo necesita para deducirlo.', 'For a business or a self-employed customer, tick "Show VAT": they need it to deduct it.')}</p>}
          <Field label={tr('Nota en la factura (opcional)', 'Note on the invoice (optional)')}>
            <textarea rows={2} value={note} onChange={(e) => setNote(e.target.value)} maxLength={400} placeholder={tr('Garantía, estado del teléfono, nombre del cliente…', "Warranty, the phone's condition, the customer's name…")} />
          </Field>
        </section>

        <section className="bill-card">
          <h3>{tr('Cómo se paga', 'How it is paid')}</h3>
          <Segmented value={method} onChange={setMethod} options={(['cash', 'card', 'bizum'] as Method[]).map((m) => ({ value: m, label: METHOD[m]() }))} />
          {method === 'card' && cards.length > 1 && (
            <div className="picks">
              {cards.map((m) => (
                <button key={m.id} type="button" className={`pick${machine === m.id ? ' on' : ''}`} onClick={() => setMachine(m.id)}>
                  {m.name}
                </button>
              ))}
            </div>
          )}
        </section>
      </div>

      <aside className="bill-preview">
        <div className="bill-preview-head">
          <span>
            {tr('Así sale', 'This is how it comes out')}
            {number && <small className="mono"> · {number}</small>}
          </span>
          <Segmented
            value={format}
            onChange={setFormatPick}
            options={[
              { value: 'a4', label: tr('Hoja A4', 'A4 page') },
              { value: 'ticket', label: tr('Ticket', 'Receipt') },
            ]}
          />
        </div>
        <div className="bill-paper">{format === 'a4' ? <SheetView sheets={sheets} busy={previewing} /> : <Paper lines={ticket} logo={logo} cols={settings.columns} />}</div>
      </aside>

      <footer className="bill-foot">
        <div className="bill-total">
          <span>{tr('Total', 'Total')}</span>
          <b>{eur(total)}</b>
          {showVat && <small>{tr('IVA incluido', 'VAT included')}</small>}
        </div>
        <div className="bill-print">
          <label className="bill-check">
            <input type="checkbox" checked={print} onChange={(e) => setPrint(e.target.checked)} />
            <Printer size={15} />
            {format === 'a4' ? tr('Imprimir en A4 en', 'Print on A4 on') : tr('Imprimir ticket en', 'Print receipt on')}
          </label>
          <PrinterPick format={format} value={printer} onChange={setPrinter} again={settings} />
        </div>
        <div className="bill-go">
          {missing.length > 0 && (
            <span className="bill-missing">
              <TriangleAlert size={15} />
              {tr('Falta', 'Missing')}: {missing.map((m) => m.text).join(', ')}
            </span>
          )}
          {onCancel && (
            <button className="btn btn-ghost" onClick={onCancel} disabled={busy}>
              {tr('Cancelar', 'Cancel')}
            </button>
          )}
          <button className="btn btn-primary btn-lg" onClick={charge} disabled={busy}>
            <FileText size={18} />
            {busy ? tr('Guardando…', 'Saving…') : from ? tr('Guardar cambios', 'Save changes') : tr('Cobrar y emitir', 'Charge and issue')}
          </button>
        </div>
      </footer>
    </div>
  );
}

// ------------------------------------------------------------- issued bills

interface ListProps {
  user: User;
  settings: Settings;
  selId: number | null;
  setSelId: (id: number | null) => void;
  reload: number;
  onCount: (n: number) => void;
  onFix: (t: TicketRecord) => void;
  onNew: () => void;
  // goes up with each bill made here
  made: number;
  onSetupPrinter?: () => void;
  onSettings?: (s: Settings) => void;
}

function BillList({ user, settings, selId, setSelId, reload, onCount, onFix, onNew, onSetupPrinter, made, onSettings }: ListProps) {
  const toast = useToast();
  const [q, setQ] = useState('');
  // a bill just made always shows: a search typed before is cleared
  useEffect(() => {
    if (made) setQ('');
  }, [made]);
  const [list, setList] = useState<BillRow[] | null>(null);
  const [rec, setRec] = useState<TicketRecord | null>(null);
  const [paper, setPaper] = useState<{ lines: PLine[]; logo: Logo | null } | null>(null);
  const [sheets, setSheets] = useState<Sheet[]>([]);
  const [formatPick, setFormatPick] = useState<Format | null>(null);
  const format = formatPick ?? startFormat(settings);
  const [printer, setPrinter] = useState<OneOff | null>(null);
  useEffect(() => setPrinter(null), [format]);
  const [busy, setBusy] = useState('');
  const [sharing, setSharing] = useState(false);
  const [voiding, setVoiding] = useState(false);

  const load = useCallback(() => {
    call<BillRow[]>('invoices.list', { q: q.trim() })
      .then((l) => {
        setList(l);
        if (!q.trim()) onCount(l.length);
      })
      .catch((e) => toast.error(errorText(e)));
  }, [q, toast, onCount]);

  useEffect(() => {
    const t = window.setTimeout(load, 150);
    return () => window.clearTimeout(t);
  }, [load, reload]);

  // the newest is shown unless one is chosen; a bill just made is chosen
  // before the list that has it arrives, so only a fresh list decides
  const chosen = useRef(selId);
  chosen.current = selId;
  useEffect(() => {
    if (!list) return;
    const id = chosen.current;
    if (id === null || !list.some((r) => r.id === id)) setSelId(list[0]?.id ?? null);
  }, [list, setSelId]);

  // answers for a bill already left behind are dropped, and while the next
  // one loads nothing of the last stays on screen to be printed or voided
  const opened = useRef(selId);
  opened.current = selId;
  const recId = useRef<number | null>(null);
  recId.current = rec?.id ?? null;
  const open = useCallback(() => {
    if (selId === null) {
      setRec(null);
      return;
    }
    const id = selId;
    if (recId.current !== id) {
      setRec(null);
      setSheets([]);
      setPaper(null);
    }
    call<{ ticket: TicketRecord; lines: PLine[]; logo: Logo | null }>('tickets.get', { id })
      .then((r) => {
        if (opened.current !== id) return;
        setRec(r.ticket);
        setPaper({ lines: r.lines, logo: r.logo });
      })
      .catch((e) => opened.current === id && toast.error(errorText(e)));
    call<{ sheets: Sheet[] }>('invoices.sheets', { id })
      .then((r) => opened.current === id && setSheets(r.sheets))
      .catch(() => opened.current === id && setSheets([]));
  }, [selId, toast]);

  useEffect(() => {
    open();
  }, [open, reload]);

  const print = async () => {
    if (!rec || busy) return;
    setBusy('print');
    try {
      const r = await call<{ warning: string | null }>('tickets.print', { id: rec.id, format, printer: printer ?? undefined });
      if (r.warning) toast.info(warningText(r.warning));
      else {
        toast.ok(tr('Impresa', 'Printed'));
        void keepA4(printer, format, settings, onSettings, toast.info);
      }
      open();
    } catch (e) {
      const err = toError(e);
      if ((isPrinterSetup(err) || err.code === 'E_NO_A4_PRINTER') && onSetupPrinter) toast.error(errorText(err), { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } });
      else if (isTransient(err)) toast.error(errorText(err), { action: { label: tr('Reintentar', 'Retry'), run: print } });
      else toast.error(errorText(err));
    } finally {
      setBusy('');
    }
  };

  const pdf = async () => {
    if (!rec || busy) return;
    setBusy('pdf');
    try {
      const r = await call<{ path: string }>('tickets.pdf', { id: rec.id, reveal: true });
      toast.ok(`${tr('Guardada en', 'Saved in')} ${r.path}`);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const fixable = rec ? canFix(rec, user, settings) : false;
  const voidable = rec ? mayVoid(rec, user, settings) : false;
  return (
    <div className="split bills-split">
      <section className="list-pane">
        <div className="pane-head">
          <button className="btn btn-primary block" onClick={onNew}>
            <Plus size={16} />
            {tr('Nueva factura', 'New invoice')}
          </button>
          <div className="search-sm">
            <Search size={16} />
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={tr('Nº, cliente, NIF o importe', 'Number, customer, NIF or amount')} />
          </div>
        </div>
        <div className="list">
          {list?.map((r) => (
            <button key={r.id} className={`ticket-row${selId === r.id ? ' on' : ''}${r.voided ? ' voided' : ''}`} onClick={() => setSelId(r.id)}>
              <div className="ticket-row-main">
                <b className="mono">{r.number}</b>
                <span>
                  {r.customerName ? (
                    <>
                      {r.customerName} · <span className="mono">{r.customerNif}</span>
                    </>
                  ) : (
                    tr('Factura simplificada', 'Simplified invoice')
                  )}
                </span>
                <span>
                  {dateTime(r.issuedAt)}
                  {r.replaces && <> · {tr(`corrige ${r.replaces}`, `fixes ${r.replaces}`)}</>}
                </span>
              </div>
              <div className="ticket-row-side">
                {r.voided && <span className="badge red">{r.voidReason?.startsWith('Corregida') ? tr('Corregida', 'Corrected') : tr('Anulada', 'Voided')}</span>}
                {r.kind === 'refund' && <span className="badge amber">{tr('Rectificativa', 'Credit note')}</span>}
                <b>{eur(r.totalCents)}</b>
              </div>
            </button>
          ))}
          {list && list.length === 0 && (
            <Empty
              icon={<FileText size={28} />}
              title={q ? tr('Ninguna factura con eso', 'No invoice matches') : tr('Aún no hay facturas', 'No invoices yet')}
              text={tr('Las que se hagan aquí, y las de Venta con datos del cliente, salen en esta lista.', 'The ones made here, and those from Sale with customer details, show up in this list.')}
            />
          )}
        </div>
      </section>

      <section className="detail-pane">
        {rec ? (
          <div className="bill-detail">
            <div className="bill-detail-head">
              <div>
                <h2 className="mono">{rec.number}</h2>
                <p className="muted">
                  {dateTime(rec.issuedAt)} · {rec.userName} · {METHOD[rec.payment.method]()} · <b>{eur(rec.totalCents)}</b>
                </p>
                {rec.customer ? (
                  <p className="ref-note">
                    <b>{rec.customer.name}</b> · <span className="mono">{rec.customer.nif}</span>
                  </p>
                ) : (
                  <p className="ref-note">{tr('Factura simplificada, sin datos del cliente', 'Simplified invoice, no customer details')}</p>
                )}
                {rec.replaces && <p className="print-note">{tr(`Corrige a ${rec.replaces}, que queda anulada en Tiquet.`, `Fixes ${rec.replaces}, which stays voided in Tiquet.`)}</p>}
                {rec.voidedAt && (
                  <p className="void-note">
                    {rec.replacedBy ? tr(`Corregida: la buena es ${rec.replacedBy}.`, `Corrected: the right one is ${rec.replacedBy}.`) : `${tr('Anulada', 'Voided')}: ${rec.voidReason ?? ''}`} ({rec.voidedBy}, {dateTime(rec.voidedAt)})
                  </p>
                )}
              </div>
            </div>
            <div className="bill-actions">
              <div className="bill-actions-print">
                <Segmented
                  value={format}
                  onChange={setFormatPick}
                  options={[
                    { value: 'a4', label: 'A4' },
                    { value: 'ticket', label: tr('Ticket', 'Receipt') },
                  ]}
                />
                <PrinterPick format={format} value={printer} onChange={setPrinter} again={settings} />
                <button className="btn btn-primary" onClick={print} disabled={!!busy || !!rec.voidedAt}>
                  <Printer size={16} />
                  {busy === 'print' ? tr('Imprimiendo…', 'Printing…') : tr('Imprimir', 'Print')}
                </button>
              </div>
              <div className="bill-actions-more">
                <button className="btn" onClick={pdf} disabled={!!busy}>
                  <FileDown size={16} />
                  PDF
                </button>
                <button className="btn" onClick={() => setSharing(true)} disabled={!!busy}>
                  <Share2 size={16} />
                  {tr('Enviar', 'Send')}
                </button>
                {fixable && (
                  <button className="btn" onClick={() => onFix(rec)} disabled={!!busy}>
                    <Pencil size={16} />
                    {tr('Editar', 'Edit')}
                  </button>
                )}
                {voidable && (
                  <button className="btn btn-danger-ghost" onClick={() => setVoiding(true)} disabled={!!busy}>
                    <Ban size={16} />
                    {tr('Anular', 'Void')}
                  </button>
                )}
              </div>
            </div>
            <p className="bill-reprint">{tr('Así la recibió el cliente. Si se vuelve a imprimir, sale con «COPIA», como pide la ley.', 'As the customer got it. Printed again, it says "COPIA", as the law asks.')}</p>
            <div className="bill-paper">
              {format === 'ticket' ? paper && <Paper lines={paper.lines} logo={paper.logo} cols={settings.columns} /> : <SheetView sheets={sheets} />}
            </div>
          </div>
        ) : (
          <Empty icon={<FileText size={28} />} title={tr('Elige una factura', 'Pick an invoice')} />
        )}
      </section>

      {sharing && rec && <ShareDialog ticketId={rec.id} number={rec.number} onClose={() => setSharing(false)} />}
      {voiding && rec && (
        <VoidBill
          rec={rec}
          onClose={() => setVoiding(false)}
          onDone={() => {
            setVoiding(false);
            load();
            open();
          }}
        />
      )}
    </div>
  );
}

function VoidBill({ rec, onClose, onDone }: { rec: TicketRecord; onClose: () => void; onDone: () => void }) {
  const toast = useToast();
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const go = async () => {
    setBusy(true);
    try {
      await call('tickets.void', { id: rec.id, reason: reason.trim() || tr('Anulada', 'Voided') });
      toast.ok(`${rec.number} ${tr('anulada', 'voided')}`);
      onDone();
    } catch (e) {
      toast.error(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal
      title={`${tr('Anular', 'Void')} ${rec.number}`}
      onClose={onClose}
      actions={
        <>
          <button className="btn btn-ghost" onClick={onClose}>
            {tr('Cancelar', 'Cancel')}
          </button>
          <button className="btn btn-danger" onClick={go} disabled={busy}>
            <Ban size={16} />
            {tr('Anular', 'Void')}
          </button>
        </>
      }
    >
      <p>{tr('Sale de las listas y los totales. En Tiquet queda anotada, con quién y cuándo. Para cambiar algo de la factura usa mejor Editar.', 'It leaves every list and total. Tiquet keeps a note of it, with who and when. To change something on the invoice, use Edit instead.')}</p>
      <Field label={tr('Por qué (opcional)', 'Why (optional)')}>
        <input value={reason} onChange={(e) => setReason(e.target.value)} maxLength={200} autoFocus />
      </Field>
    </Modal>
  );
}
