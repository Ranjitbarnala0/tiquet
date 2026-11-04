import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKey, type MouseEvent as ReactMouse } from 'react';
import {
  Banknote,
  Check,
  CornerDownLeft,
  CreditCard,
  Eye,
  History,
  LayoutGrid,
  Minus,
  Pencil,
  Plus,
  Printer,
  Repeat2,
  Save,
  ScanBarcode,
  Search,
  Send,
  ShoppingCart,
  Smartphone,
  SmartphoneCharging,
  Star,
  Trash2,
  Undo2,
  Wallet,
  X,
} from 'lucide-react';
import { ApiError, call, toError } from './api';
import { eur, fmt, norm, parseMoney, rate } from './format';
import { errorText, isPrinterSetup, isTransient, printerStateText, tr, warningText } from './i18n';
import { ClockModal } from './Clock';
import { ItemDialog, type ItemDraft } from './ItemDialog';
import { customerNif } from './nif';
import { Paper } from './Paper';
import { BIG, ExpenseModal, RechargeModal, useEntrySave } from './Quick';
import { ShareDialog } from './Share';
import { useToast } from './toast';
import type { CartLine, CashDrawer, CashSession, Customer, DaySummary, Finalized, Logo, Method, Preview, Product, Recent, Settings, TicketRecord } from './types';

type Shown = Preview & { forLines: string };
import { printerLabel, usePrinterStatus } from './usePrinter';
import { Field, Modal, useTrap } from './ui';

type Option =
  | { kind: 'product'; product: Product; qty: number }
  | { kind: 'custom'; name: string; cents: number; qty: number }
  | { kind: 'expense'; concept: string; cents: number }
  | { kind: 'recharge'; cents: number };

let seq = 0;
const uid = () => `l${Date.now().toString(36)}${(seq++).toString(36)}`;
const newSaleId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

// the sale this cart corrects: charging it replaces that ticket
type Fixing = { id: number; number: string };

// what survives a crash: the lines, the sale they correct, and the id the core
// uses to tell a retry of this same cart from a new sale
function storedCart(key: string): { saleId: string; lines: CartLine[]; fixing?: Fixing | null } {
  try {
    const v = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (Array.isArray(v)) return { saleId: newSaleId(), lines: v };
    if (v && Array.isArray(v.lines) && typeof v.saleId === 'string') return v;
  } catch {
    // unreadable: start empty
  }
  return { saleId: newSaleId(), lines: [] };
}

// buttons that must not steal focus from the search box (barcode scanners
// type into whatever has focus)
const keep = (e: ReactMouse) => e.preventDefault();

function parseQuery(q: string) {
  const m = q.match(/^(\d{1,3})\s*[*x×]\s*(.*)$/i);
  return m ? { qty: Math.min(999, Math.max(1, Number(m[1]))), text: m[2] } : { qty: 1, text: q };
}

function parseCustom(text: string): { name: string; cents: number } | null {
  const t = text.trim();
  if (!t || /^\d{6,}$/.test(t)) return null;
  if (/^-?[\d.,]+\s*€?$/.test(t)) {
    const c = parseMoney(t);
    return c === null || c === 0 ? null : { name: c < 0 ? 'Descuento' : 'Varios', cents: c };
  }
  const m = t.match(/^(.*\S)\s+(-?[\d.,]+)\s*€?$/);
  if (!m) return null;
  const c = parseMoney(m[2]);
  return c === null ? null : { name: m[1].slice(0, 120), cents: c };
}

// "-pantalla 45" is something bought and "recarga 10" a recharge: both are
// saved at once from the same box, without leaving the sale
function parseEntry(text: string): Option | null {
  const t = text.trim();
  const bought = t.match(/^-\s*(\S.*?)\s+([\d.,]+)\s*€?$/);
  if (bought) {
    const cents = parseMoney(bought[2]);
    return cents && cents > 0 ? { kind: 'expense', concept: bought[1].slice(0, 120), cents } : null;
  }
  const topUp = t.match(/^rec(?:arga)?\.?\s+([\d.,]+)\s*€?$/i);
  if (topUp) {
    const cents = parseMoney(topUp[1]);
    return cents && cents > 0 ? { kind: 'recharge', cents } : null;
  }
  return null;
}

type Indexed = { p: Product; name: string; words: string[] };

function index(products: Product[]): Indexed[] {
  return products.map((p) => {
    const name = norm(p.name);
    return { p, name, words: name.split(/\s+/) };
  });
}

function byCode(items: Indexed[], code: string): Product | undefined {
  return items.find((i) => i.p.barcode && i.p.barcode === code)?.p;
}

function search(items: Indexed[], q: string): Option[] {
  const raw = q.trim();
  const entry = parseEntry(raw);
  if (entry?.kind === 'expense') return [entry];
  // a scanned code is matched exactly as read, before "10x..." is taken as
  // a quantity
  const whole = raw ? byCode(items, raw) : undefined;
  if (whole) return [{ kind: 'product', product: whole, qty: 1 }];
  const { qty, text } = parseQuery(raw);
  const t = text.trim();
  const out: Option[] = [];
  if (t) {
    const code = byCode(items, t);
    if (code) out.push({ kind: 'product', product: code, qty });
    // a long run of digits is a code: a partial match would sell the wrong item
    if (/^\d{6,}$/.test(t)) return out;
    const tokens = norm(t).split(/\s+/).filter(Boolean);
    const scored: [number, Product][] = [];
    for (const { p, name, words } of items) {
      if (p === code) continue;
      if (!tokens.every((tok) => name.includes(tok) || (p.barcode && p.barcode.startsWith(tok)))) continue;
      let s = 0;
      if (name.startsWith(tokens[0])) s += 40;
      else if (words.some((w) => w.startsWith(tokens[0]))) s += 20;
      if (p.favorite) s += 8;
      s += Math.min(p.soldQty, 200) / 10;
      scored.push([s, p]);
    }
    scored.sort((a, b) => b[0] - a[0] || a[1].name.localeCompare(b[1].name));
    for (const [, p] of scored.slice(0, code ? 6 : 7)) out.push({ kind: 'product', product: p, qty });
  }
  const custom = parseCustom(t);
  if (custom) out.push({ kind: 'custom', qty, ...custom });
  return entry ? [entry, ...out] : out;
}

function quick(total: number, most = 4): number[] {
  const out = new Set<number>();
  for (const step of [100, 500, 1000, 2000, 5000, 10000]) {
    const v = Math.ceil(total / step) * step;
    if (v > total) out.add(v);
  }
  return [...out].sort((a, b) => a - b).slice(0, most);
}

// A barcode scanner types the whole code in a few milliseconds and ends it
// with Enter; a person cannot type six characters that fast.
const SCAN_GAP = 50;
const SCAN_MIN = 6;

const METHODS: { id: Method; key: string; icon: typeof Banknote; label: () => string }[] = [
  { id: 'cash', key: 'F1', icon: Banknote, label: () => tr('Efectivo', 'Cash') },
  { id: 'card', key: 'F2', icon: CreditCard, label: () => tr('Tarjeta', 'Card') },
  { id: 'bizum', key: 'F3', icon: Smartphone, label: () => 'Bizum' },
];

function PriceInput({ cents, onDone }: { cents: number; onDone: (c: number | null) => void }) {
  const [v, setV] = useState(cents ? fmt(cents) : '');
  const ref = useRef<HTMLInputElement>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const parsed = parseMoney(v);
  const finish = (c: number | null) => {
    if (done.current) return;
    done.current = true;
    onDone(c);
  };
  return (
    <input
      ref={ref}
      className={`cell-input${v && parsed === null ? ' bad' : ''}`}
      value={v}
      inputMode="decimal"
      aria-label={tr('Precio', 'Price')}
      onChange={(e) => setV(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          e.stopPropagation();
          finish(parsed);
        } else if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          finish(null);
        }
      }}
      onBlur={() => finish(parsed)}
    />
  );
}

function QtyInput({ qty, onChange, onExit, disabled }: { qty: number; onChange: (n: number) => void; onExit: () => void; disabled?: boolean }) {
  const [v, setV] = useState(String(qty));
  useEffect(() => setV(String(qty)), [qty]);
  return (
    <input
      className="qty-input"
      disabled={disabled}
      value={v}
      inputMode="numeric"
      aria-label={tr('Cantidad', 'Quantity')}
      onFocus={(e) => e.target.select()}
      onChange={(e) => setV(e.target.value.replace(/\D/g, '').slice(0, 3))}
      onBlur={() => {
        const n = Number(v);
        if (n >= 1 && n !== qty) onChange(n);
        else setV(String(qty));
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          if (e.key === 'Escape') setV(String(qty));
          onExit();
        }
      }}
    />
  );
}

type Tab = 'recent' | 'fav' | 'all';
const TAB_KEY = 'tiquet.saleTab';
// printing is a switch by the pay buttons, remembered on this PC
const PRINT_KEY = 'tiquet.print';

function storedPrint(): boolean {
  try {
    return localStorage.getItem(PRINT_KEY) !== '0';
  } catch {
    return true;
  }
}
// a long catalogue is found with the search box; the grid shows this many
const ALL_SHOWN = 200;

function storedTab(): Tab {
  try {
    const t = localStorage.getItem(TAB_KEY);
    return t === 'recent' || t === 'fav' || t === 'all' ? t : 'fav';
  } catch {
    return 'fav';
  }
}

interface Props {
  cartKey: string;
  active: boolean;
  admin: boolean;
  // items handed over from another screen: a ticket repeated from Hoy, or one
  // to correct, which the new sale then replaces
  incoming: { lines: ItemDraft[]; from: string; n: number; replaces?: Fixing } | null;
  onCatalogue: () => void;
  settings: Settings;
  logo: Logo | null;
  products: Product[];
  onSold: () => void;
  onSetupPrinter?: () => void;
  // whether a ticket is on the screen or being charged
  onBusy: (busy: boolean) => void;
  onDay: () => void;
}

type Charge = { method: Method; tendered: number | null; print: boolean; invoice: boolean };

function Cell({ label, cents, dot, strong, minus }: { label: string; cents: number; dot?: 'cash' | 'card' | 'bizum' | 'recharge' | 'expense'; strong?: boolean; minus?: boolean }) {
  return (
    <span className={`strip-cell${strong ? ' strong' : ''}`}>
      <small>
        {dot && <i className={`dot dot-${dot}`} aria-hidden="true" />}
        {label}
      </small>
      <b>{eur(minus ? -cents : cents)}</b>
    </span>
  );
}

// The day at a glance, under the products: each way of paying apart, the
// total of all, what went out and what is left. A tap opens the whole day.
function DayStrip({ s, onOpen }: { s: DaySummary; onOpen: () => void }) {
  return (
    <button className="strip" onMouseDown={keep} onClick={onOpen} title={tr('Ver las ventas y gastos de hoy', 'See today’s sales and expenses')}>
      <Cell label={tr('Efectivo', 'Cash')} cents={s.cashCents} dot="cash" />
      <Cell label={tr('Tarjeta', 'Card')} cents={s.cardCents} dot="card" />
      {s.bizumCents !== 0 && <Cell label="Bizum" cents={s.bizumCents} dot="bizum" />}
      <Cell label={tr('Recargas', 'Recharges')} cents={s.rechargeCents} dot="recharge" />
      <Cell label={tr('Total', 'Total')} cents={s.totalCents} strong />
      <Cell label={tr('Gastos', 'Expenses')} cents={s.expenseCents} dot="expense" minus />
      <Cell label={tr('Queda', 'Left')} cents={s.netCents} strong />
    </button>
  );
}

export function Sale({ cartKey, active, admin, incoming, onCatalogue, settings, logo, products, onSold, onSetupPrinter, onBusy, onDay }: Props) {
  const toast = useToast();
  const [restored] = useState(() => storedCart(cartKey));
  const [lines, setLines] = useState<CartLine[]>(restored.lines);
  const [fixing, setFixing] = useState<Fixing | null>(restored.fixing ?? null);
  const saleId = useRef(restored.saleId);
  const [clock, setClock] = useState<{ last: string; charge: Charge } | null>(null);
  const history = useRef<CartLine[][]>([]);
  const [query, setQuery] = useState('');
  const [hi, setHi] = useState(0);
  const [sel, setSel] = useState<string | null>(null);
  const [priceEdit, setPriceEdit] = useState<string | null>(null);
  const [flash, setFlash] = useState<{ key: string; n: number } | null>(null);
  const [paying, setPaying] = useState(false);
  const [method, setMethod] = useState<Method>('cash');
  const [given, setGiven] = useState('');
  // a full invoice: the customer's details
  const [invoice, setInvoice] = useState(false);
  const [customer, setCustomer] = useState<Customer>({ name: '', nif: '', address: '' });
  const [preview, setPreview] = useState<Shown | null>(null);
  const [previewErr, setPreviewErr] = useState('');
  const [busy, setBusy] = useState(false);
  // the last sale, shown until the next one starts
  const [last, setLast] = useState<{ ticket: TicketRecord; printed: boolean } | null>(null);
  const [sharing, setSharing] = useState<TicketRecord | null>(null);
  const [printOn, setPrintOn] = useState(storedPrint);
  const [day, setDay] = useState<DaySummary | null>(null);
  const [recharge, setRecharge] = useState<{ cents: number; fromCart: boolean } | null>(null);
  const [expense, setExpense] = useState<string | null>(null);
  const [seeTicket, setSeeTicket] = useState(false);
  // the add/change dialog: 'add', or the key of the line being changed
  const [item, setItem] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>(storedTab);
  const [recent, setRecent] = useState<Recent[]>([]);
  const searchRef = useRef<HTMLInputElement>(null);
  const givenRef = useRef<HTMLInputElement>(null);
  const linesRef = useRef<HTMLDivElement>(null);
  const payRef = useRef<HTMLDivElement>(null);
  const sureBig = useRef('');
  const reqSeq = useRef(0);
  const openedAt = useRef(0);
  const burst = useRef({ text: '', last: 0, fast: 0, given: '' });
  const pendingScans = useRef<string[]>([]);
  const printing = useRef(false);
  const noPrinter = !printerLabel(settings);
  const { status: printer, refresh: refreshPrinter } = usePrinterStatus(active, printerLabel(settings));
  // the till: undefined until known, null while closed
  const [till, setTill] = useState<CashSession | null | undefined>(undefined);
  const [float, setFloat] = useState('');
  const [opening, setOpening] = useState(false);

  const loadDay = useCallback(() => {
    call<DaySummary | null>('day.totals')
      .then(setDay)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!active) return;
    loadDay();
    const t = setInterval(loadDay, 30_000);
    return () => clearInterval(t);
  }, [active, loadDay]);

  const saveEntry = useEntrySave(loadDay);

  // survives a crash or a power cut mid-sale
  useEffect(() => {
    try {
      localStorage.setItem(cartKey, JSON.stringify({ saleId: saleId.current, lines, fixing }));
    } catch {
      // storage unavailable: the cart simply isn't restored
    }
  }, [lines, fixing, cartKey]);

  // Tiquet closed between charging and clearing the screen: the cart came
  // back, but the core already has its ticket
  useEffect(() => {
    if (!restored.lines.length) return;
    call<{ ticket: { number: string } | null }>('sale.status', { saleId: restored.saleId })
      .then((r) => {
        if (!r.ticket || saleId.current !== restored.saleId) return;
        saleId.current = newSaleId();
        history.current = [];
        setLines([]);
        setFixing(null);
        toast.info(`${tr('Esta venta ya se había cobrado', 'This sale had already been charged')} (${r.ticket.number}). ${tr('Se ha vaciado el ticket.', 'The ticket has been cleared.')}`, {
          sticky: true,
        });
      })
      .catch(() => {});
  }, [restored, toast]);

  const loadRecent = useCallback(() => {
    call<Recent[]>('products.recent')
      .then(setRecent)
      .catch(() => {});
  }, []);

  useEffect(loadRecent, [loadRecent, products]);

  const loadTill = useCallback(() => {
    call<CashDrawer>('cash.state')
      .then((d) => setTill(d.session))
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!active) return;
    loadTill();
    const t = setInterval(loadTill, 30_000);
    return () => clearInterval(t);
  }, [active, loadTill]);

  useEffect(() => onBusy(lines.length > 0 || paying || busy), [lines.length, paying, busy, onBusy]);

  useEffect(() => {
    try {
      localStorage.setItem(TAB_KEY, tab);
      localStorage.setItem(PRINT_KEY, printOn ? '1' : '0');
    } catch {
      // the choice just will not be remembered
    }
  }, [tab, printOn]);

  const catalogue = useMemo(() => index(products), [products]);
  const options = useMemo(() => {
    // "-10%": that much off what is on the ticket
    const pct = query.trim().match(/^-\s*(\d{1,2}(?:[.,]\d)?)\s*%$/);
    const sum = lines.reduce((s, l) => s + l.qty * l.unitCents, 0);
    if (pct && sum > 0) {
      const off = Math.round((sum * Number(pct[1].replace(',', '.'))) / 100);
      return off > 0 ? [{ kind: 'custom' as const, name: `Descuento ${pct[1]}%`, cents: -off, qty: 1 }] : [];
    }
    return search(catalogue, query);
  }, [catalogue, query, lines]);
  useEffect(() => setHi(0), [query]);

  const tendered = method === 'cash' ? parseMoney(given, 999_999_999) : null;
  const payload = lines.map(({ productId, name, qty, unitCents, vatBp, repairId }) => ({ productId, name, qty, unitCents, vatBp, repairId }));
  const linesKey = JSON.stringify(payload);
  const customerOk = !invoice || (customer.name.trim().length > 0 && customerNif(customer.nif));
  const floatCents = parseMoney(float || '0', 999_999_999);
  const previewArgs = JSON.stringify({
    lines: payload,
    payment: paying ? { method, tenderedCents: tendered ?? undefined } : undefined,
    customer: paying && invoice && customerOk ? customer : undefined,
  });

  // totals, VAT and the ticket itself always come from the core; requests can
  // overtake each other, so only the newest answer is applied, and each one
  // remembers which cart it was worked out for
  useEffect(() => {
    const n = ++reqSeq.current;
    call<Preview>('sale.preview', JSON.parse(previewArgs))
      .then((p) => {
        if (n !== reqSeq.current) return;
        setPreview({ ...p, forLines: linesKey });
        setPreviewErr('');
      })
      .catch((e) => n === reqSeq.current && setPreviewErr(errorText(e)));
  }, [previewArgs, settings, logo]);

  useEffect(() => {
    const el = linesRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
  }, [lines.length]);

  useEffect(() => {
    if (!active) setPaying(false);
  }, [active]);

  useTrap(payRef, paying);

  useEffect(() => {
    if (active && !paying) searchRef.current?.focus();
  }, [active, paying]);

  // The box is always ready for the next key: a click anywhere on the page,
  // coming back to the window or a window closing hands the focus back.
  useEffect(() => {
    if (!active || paying) return;
    const back = () =>
      requestAnimationFrame(() => {
        if (document.querySelector('.modal-back')) return;
        const el = document.activeElement;
        if (!el || el === document.body || el.tagName === 'BUTTON') searchRef.current?.focus();
      });
    window.addEventListener('focus', back);
    document.addEventListener('mouseup', back);
    document.addEventListener('focusout', back);
    return () => {
      window.removeEventListener('focus', back);
      document.removeEventListener('mouseup', back);
      document.removeEventListener('focusout', back);
    };
  }, [active, paying]);

  // the queue can look ready while the last ticket did not come out
  const lastFailed = !!printer && printer.state === 'ready' && (printer.last?.status === 'failed' || printer.last?.status === 'unknown');
  // the total on screen belongs to the cart on screen; until the core answers
  // it is worked out here, so the buttons never wait for it
  const fresh = !!preview && preview.forLines === linesKey;
  const total = fresh ? preview.totals.totalCents : lines.reduce((sum, l) => sum + l.qty * l.unitCents, 0);
  const count = fresh ? preview.totals.itemCount : lines.reduce((sum, l) => sum + (l.unitCents >= 0 ? l.qty : 0), 0);
  const short = method === 'cash' && tendered !== null && tendered < total;
  const badAmount = method === 'cash' && given.trim() !== '' && tendered === null;
  const tooMuch = method === 'cash' && !short && preview?.paymentError === 'E_TENDERED_HIGH';
  const negative = total < 0;
  // a simplified invoice stops at 3.000 EUR, or 400 EUR with a repair (a
  // service) on it; past that it needs the customer
  const service = lines.some((l) => l.repairId);
  const limit = service ? 40_000 : 300_000;
  const overLimit = total > limit;
  const blocked = short || badAmount || tooMuch || negative || !customerOk || (overLimit && !invoice);

  const commit = (next: CartLine[]) => {
    history.current.push(lines);
    if (history.current.length > 60) history.current.shift();
    setLines(next);
  };
  const pulse = (key: string) => setFlash((f) => ({ key, n: (f?.n ?? 0) + 1 }));
  const target = () => lines.find((l) => l.key === sel) ?? lines[lines.length - 1];

  const undo = () => {
    const prev = history.current.pop();
    if (!prev) return;
    setLines(prev);
    setSel(prev[prev.length - 1]?.key ?? null);
  };

  const addProduct = (p: Product, qty = 1) => {
    const same = p.priceCents !== 0 ? lines.find((l) => l.productId === p.id && l.unitCents === p.priceCents) : undefined;
    if (same) {
      commit(lines.map((l) => (l.key === same.key ? { ...l, qty: Math.min(999, l.qty + qty) } : l)));
      setSel(same.key);
      pulse(same.key);
      return;
    }
    const line: CartLine = { key: uid(), productId: p.id, name: p.name, qty, unitCents: p.priceCents, vatBp: p.vatBp };
    commit([...lines, line]);
    setSel(line.key);
    pulse(line.key);
    if (p.priceCents === 0) setPriceEdit(line.key);
  };

  // adds items, merging with a line that is the same thing at the same price;
  // a repair is one line of its own and goes on the ticket once
  const addMany = (items: ItemDraft[]) => {
    let next = lines;
    let lastKey: string | null = null;
    for (const d of items) {
      const same = next.find((l) =>
        d.repairId
          ? l.repairId === d.repairId
          : !l.repairId && l.name === d.name && l.unitCents === d.unitCents && l.vatBp === d.vatBp && (l.productId ?? null) === (d.productId ?? null),
      );
      if (same && d.repairId) {
        lastKey = same.key;
      } else if (same) {
        next = next.map((l) => (l.key === same.key ? { ...l, qty: Math.min(999, l.qty + d.qty) } : l));
        lastKey = same.key;
      } else {
        const line: CartLine = { key: uid(), name: d.name, qty: d.qty, unitCents: d.unitCents, vatBp: d.vatBp, productId: d.productId, repairId: d.repairId };
        next = [...next, line];
        lastKey = line.key;
      }
    }
    if (!lastKey) return;
    commit(next);
    setSel(lastKey);
    pulse(lastKey);
  };

  const changeLine = (key: string, d: ItemDraft) => commit(lines.map((l) => (l.key === key ? { ...l, ...d } : l)));

  const toDraft = (l: { name: string; qty: number; unitCents: number; vatBp: number; productId?: number | null }): ItemDraft => ({
    name: l.name,
    qty: l.qty,
    unitCents: l.unitCents,
    vatBp: l.vatBp,
    productId: l.productId ?? undefined,
  });

  // the same items again, e.g. a customer buying what they bought last time
  const repeat = (t: TicketRecord) => {
    addMany(t.lines.filter((l) => l.qty > 0 && !l.repairId).map(toDraft));
    toast.ok(`${tr('Añadidos los artículos de', 'Added the items from')} ${t.number}`);
  };
  // a repair is handed back once; there is nothing in it to sell again
  const repeatable = (t: TicketRecord) => t.lines.some((l) => l.qty > 0 && !l.repairId);

  const handled = useRef(0);
  useEffect(() => {
    if (!incoming || incoming.n === handled.current) return;
    handled.current = incoming.n;
    const repair = incoming.lines.find((l) => l.repairId);
    if (repair && lines.some((l) => l.repairId === repair.repairId)) {
      toast.info(tr('Esa reparación ya está en el ticket', 'That repair is already on the ticket'));
      return;
    }
    if (incoming.replaces) {
      if (lines.length) {
        toast.error(tr('Termina o vacía la venta en curso para corregir otra.', 'Finish or clear the sale in progress to correct another one.'));
        return;
      }
      setFixing(incoming.replaces);
      addMany(incoming.lines);
      toast.info(`${tr('Cambia lo que haga falta y cóbrala: sustituirá a', 'Change what you need and charge it: it will replace')} ${incoming.from}`);
      return;
    }
    addMany(incoming.lines);
    toast.ok(
      repair
        ? `${tr('Reparación', 'Repair')} ${incoming.from} ${tr('en el ticket: cóbrala con F12', 'is on the ticket: charge it with F12')}`
        : `${tr('Añadidos los artículos de', 'Added the items from')} ${incoming.from}`,
    );
  }, [incoming]);

  const addCustom = (name: string, cents: number, qty: number) => {
    // a discount takes the VAT rate of the line it discounts
    const vatBp = cents < 0 ? (target()?.vatBp ?? settings.defaultVatBp) : settings.defaultVatBp;
    const line: CartLine = { key: uid(), name, qty, unitCents: cents, vatBp };
    commit([...lines, line]);
    setSel(line.key);
    pulse(line.key);
  };

  const pick = (o: Option) => {
    if (o.kind === 'expense' || o.kind === 'recharge') {
      // a typo of 1000 for 10,00 is easy: a big amount takes a second Enter
      const key = `${o.kind}${o.cents}`;
      if (o.cents >= BIG && sureBig.current !== key) {
        sureBig.current = key;
        toast.info(`${tr('Importe alto', 'Large amount')}: ${eur(o.cents)}. ${tr('Pulsa Enter otra vez para apuntarlo.', 'Press Enter again to save it.')}`);
        return;
      }
      sureBig.current = '';
      saveEntry(o.kind === 'expense' ? { kind: 'expense', concept: o.concept, cents: o.cents, method: 'cash' } : { kind: 'recharge', cents: o.cents, method: 'cash' });
    } else if (o.kind === 'product') {
      addProduct(o.product, o.qty);
    } else {
      addCustom(o.name, o.cents, o.qty);
    }
    setQuery('');
  };

  // a repair is always one unit
  const setQty = (key: string, qty: number) =>
    commit(lines.map((l) => (l.key === key && !l.repairId ? { ...l, qty: Math.min(999, Math.max(1, Math.round(qty))) } : l)));

  const setPrice = (key: string, cents: number) => commit(lines.map((l) => (l.key === key ? { ...l, unitCents: cents } : l)));

  const remove = (key: string) => {
    const i = lines.findIndex((l) => l.key === key);
    commit(lines.filter((l) => l.key !== key));
    setSel(lines[i + 1]?.key ?? lines[i - 1]?.key ?? null);
  };

  const dropFix = () => {
    history.current = [];
    setFixing(null);
    setLines([]);
    setSel(null);
  };

  const clear = () => {
    if (fixing) {
      dropFix();
      return;
    }
    if (!lines.length) return;
    const cleared = lines;
    commit([]);
    setSel(null);
    // brings back exactly what was cleared, ahead of anything added since
    const back = () => setLines((cur) => [...cleared, ...cur.filter((l) => !cleared.some((c) => c.key === l.key))]);
    toast.info(tr('Ticket vaciado', 'Ticket cleared'), { action: { label: tr('Deshacer', 'Undo'), run: back } });
  };

  const addScan = (code: string) => {
    const p = products.find((x) => x.barcode && x.barcode === code);
    if (p) addProduct(p);
    else toast.error(tr(`Ningún producto tiene el código ${code}`, `No product has the code ${code}`));
  };

  // scans that arrived while the last sale was printing start the next cart
  const cartFromScans = (codes: string[]): CartLine[] => {
    const out: CartLine[] = [];
    for (const code of codes) {
      const p = products.find((x) => x.barcode && x.barcode === code);
      if (!p) {
        toast.error(tr(`Ningún producto tiene el código ${code}`, `No product has the code ${code}`));
        continue;
      }
      const same = out.find((l) => l.productId === p.id && l.unitCents === p.priceCents);
      if (same) same.qty = Math.min(999, same.qty + 1);
      else out.push({ key: uid(), productId: p.id, name: p.name, qty: 1, unitCents: p.priceCents, vatBp: p.vatBp });
    }
    return out;
  };

  const bump = (d: number) => {
    const t = target();
    if (!t) return;
    setQty(t.key, t.qty + d);
    setSel(t.key);
  };

  const moveSel = (d: number) => {
    if (!lines.length) return;
    const i = lines.findIndex((l) => l.key === sel);
    const j = i < 0 ? (d > 0 ? 0 : lines.length - 1) : Math.min(lines.length - 1, Math.max(0, i + d));
    setSel(lines[j].key);
  };

  const ready = () => {
    if (!lines.length || busy) return false;
    if (negative) {
      toast.error(errorText(new ApiError('E_NEGATIVE', '')));
      return false;
    }
    const unpriced = lines.find((l) => l.unitCents === 0);
    if (unpriced) {
      toast.error(`${tr('Falta el precio de', 'Missing price for')} «${unpriced.name}»`);
      setSel(unpriced.key);
      setPriceEdit(unpriced.key);
      return false;
    }
    return true;
  };

  const openPay = () => {
    if (!ready()) return;
    searchRef.current?.blur();
    setQuery('');
    setMethod('cash');
    setGiven('');
    openedAt.current = performance.now();
    burst.current = { text: '', last: 0, fast: 0, given: '' };
    setInvoice(total > limit);
    setCustomer({ name: '', nif: '', address: '' });
    setPaying(true);
  };

  // one tap, one sale: printing is the only thing that differs, and that is
  // the switch above the buttons. Past the simplified invoice limit it takes
  // the customer's details, so the window opens.
  const payNow = (m: Method, tenderedCents?: number) => {
    if (!ready()) return;
    if (overLimit) openPay();
    else finish({ method: m, tendered: tenderedCents ?? null, print: printOn && !noPrinter, invoice: false });
  };

  const openRecharge = () => {
    const typed = parseMoney(query.trim());
    // an amount typed on the ticket is what the recharge is for
    const fromCart = !typed && lines.length > 0 && lines.every((l) => !l.productId && !l.repairId && l.unitCents > 0);
    setQuery('');
    setRecharge({ cents: typed && typed > 0 ? typed : fromCart ? total : 0, fromCart });
  };

  const openExpense = () => {
    setExpense(query.trim().replace(/^-\s*/, ''));
    setQuery('');
  };

  // a printer that is not set up gets a way to set it up; a print that
  // failed in the spooler gets a retry; a job queued for an offline printer
  // gets neither, since retrying would print it twice
  const printFailed = (prefix: string, err: unknown, retry: () => void) => {
    const text = `${prefix}${errorText(err)}`;
    if (isTransient(err)) toast.error(text, { sticky: true, action: { label: tr('Reintentar', 'Retry'), run: retry } });
    else if (isPrinterSetup(err) && onSetupPrinter) toast.error(text, { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } });
    else toast.error(text);
    refreshPrinter();
  };

  const reprint = async (id: number, copy: boolean) => {
    if (printing.current) return;
    printing.current = true;
    try {
      const r = await call<{ warning: string | null }>('tickets.print', { id, copy });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Impreso', 'Printed'));
      refreshPrinter();
    } catch (e) {
      printFailed('', e, () => reprint(id, copy));
    } finally {
      printing.current = false;
    }
  };

  // a cash sale with the till closed: opened here with the float typed in,
  // and the sale carries on in the same window
  const openTill = async () => {
    if (opening || floatCents === null || floatCents < 0) return;
    setOpening(true);
    try {
      const d = await call<CashDrawer>('cash.open', { floatCents });
      setTill(d.session);
      setFloat('');
      toast.ok(tr('Caja abierta', 'Till opened'));
      givenRef.current?.focus();
    } catch (e) {
      toast.error(errorText(e));
      loadTill();
    } finally {
      setOpening(false);
    }
  };

  const finish = async (c: Charge, clockOk = false) => {
    if (busy || !lines.length) return;
    setBusy(true);
    try {
      const r = await call<Finalized>('sale.finalize', {
        lines: payload,
        payment: { method: c.method, tenderedCents: c.tendered ?? undefined },
        print: c.print && !noPrinter,
        saleId: saleId.current,
        clockOk,
        expectedCents: total,
        customer: c.invoice ? customer : undefined,
        replaces: fixing?.id,
      });
      history.current = [];
      saleId.current = newSaleId();
      const next = cartFromScans(pendingScans.current.splice(0));
      setLines(next);
      setSel(next[next.length - 1]?.key ?? null);
      setPaying(false);
      setGiven('');
      setLast({ ticket: r.ticket, printed: r.printed });
      if (fixing) toast.ok(`${fixing.number} ${tr('corregida', 'corrected')}: ${r.ticket.number}`);
      setFixing(null);
      if (r.duplicate) {
        toast.info(`${tr('Esta venta ya estaba cobrada', 'This sale had already been charged')}: ${r.ticket.number}`, { sticky: true });
      } else if (r.printError) {
        printFailed(`${r.ticket.number} ${tr('guardado, pero no se imprimió', 'saved, but not printed')}: `, toError(r.printError), () =>
          reprint(r.ticket.id, false),
        );
      } else {
        if (payload.some((l) => l.repairId)) toast.ok(`${r.ticket.number} · ${tr('reparación entregada', 'repair handed back')}`);
        if (r.printWarning) toast.info(warningText(r.printWarning));
        if (r.printed) refreshPrinter();
      }
      onSold();
      loadRecent();
      loadDay();
    } catch (e) {
      if (e instanceof ApiError && e.code === 'E_CLOCK') setClock({ last: e.detail, charge: c });
      else {
        if (e instanceof ApiError && (e.code === 'E_OVER_LIMIT' || e.code === 'E_OVER_LIMIT_SERVICE')) setInvoice(true);
        toast.error(errorText(e));
      }
    } finally {
      setBusy(false);
    }
  };

  // charged the wrong way: it is noticed right here, and put right on the
  // same ticket with one tap
  const changePay = async (m: Method) => {
    if (!last || last.ticket.payment.method === m) return;
    try {
      const r = await call<{ ticket: TicketRecord }>('tickets.payment', { id: last.ticket.id, method: m });
      setLast((cur) => (cur ? { ...cur, ticket: r.ticket } : cur));
      loadDay();
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  // the window's Enter and its buttons, once everything in it adds up
  const confirmPay = (print: boolean) => {
    if (!blocked) finish({ method, tendered, print, invoice });
  };

  const onSearchKey = (e: ReactKey<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (query) setHi((h) => Math.min(h + 1, options.length - 1));
      else moveSel(1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (query) setHi((h) => Math.max(h - 1, 0));
      else moveSel(-1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const q = query.trim();
      if (!q) {
        openPay();
        return;
      }
      const o = options[hi] ?? options[0];
      if (o) {
        pick(o);
        return;
      }
      // keep what was typed or scanned visible, but selected, so the next
      // scan replaces it instead of appending to it
      e.currentTarget.select();
      toast.error(
        /^\d{6,}$/.test(q)
          ? tr(`Ningún producto tiene el código ${q}`, `No product has the code ${q}`)
          : tr('Sin resultados. Para un artículo libre escribe nombre y precio: «funda 12,50»', 'No results. For a custom item type a name and a price: "case 12.50"'),
      );
    } else if (e.key === 'Escape' && query) {
      e.preventDefault();
      setQuery('');
    } else if (!query && (e.code === 'NumpadAdd' || e.code === 'NumpadSubtract')) {
      e.preventDefault();
      bump(e.code === 'NumpadAdd' ? 1 : -1);
    } else if (!query && e.key === 'Delete') {
      e.preventDefault();
      const t = target();
      if (t) remove(t.key);
    } else if (!query && e.ctrlKey && e.key.toLowerCase() === 'z') {
      e.preventDefault();
      undo();
    }
  };

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (document.querySelector('.modal-back')) return;
      if (paying) {
        const now = performance.now();
        const b = burst.current;
        if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
          if (now - b.last < SCAN_GAP) {
            b.text += e.key;
            b.fast += 1;
          } else {
            burst.current = { text: e.key, last: now, fast: 0, given };
          }
          burst.current.last = now;
        }
        if (e.key === 'Enter') {
          e.preventDefault();
          // a code scanned while the dialog is open is an item, not money:
          // it never confirms a payment
          if (b.text.length >= SCAN_MIN && b.fast >= SCAN_MIN - 1 && now - b.last < SCAN_GAP * 2) {
            const code = b.text.trim();
            burst.current = { text: '', last: 0, fast: 0, given: '' };
            setGiven(b.given);
            if (busy) {
              pendingScans.current.push(code);
            } else {
              setPaying(false);
              addScan(code);
            }
            return;
          }
          // the Enter that opened the dialog, or a scanner's trailing one
          if (e.repeat || busy || now - openedAt.current < 400) return;
          confirmPay(e.shiftKey ? !printOn : printOn);
          return;
        }
        if (busy) {
          if (e.key === 'Escape') e.preventDefault();
          return;
        }
        if (e.key === 'Escape') {
          e.preventDefault();
          setPaying(false);
        } else if (e.key === 'F1' || e.key === 'F2' || e.key === 'F3') {
          e.preventDefault();
          setMethod(e.key === 'F1' ? 'cash' : e.key === 'F2' ? 'card' : 'bizum');
        } else if (method === 'cash' && !(e.target as HTMLElement).closest?.('input, textarea') && /^[\d.,]$/.test(e.key)) {
          givenRef.current?.focus();
        }
        return;
      }
      if (e.key === 'F12' || (e.ctrlKey && e.key === 'Enter')) {
        e.preventDefault();
        openPay();
        return;
      }
      if (e.key === 'F4') {
        e.preventDefault();
        setItem('add');
        return;
      }
      if (e.key === 'F6') {
        e.preventDefault();
        setPrintOn((on) => !on);
        return;
      }
      if (e.key === 'F7') {
        e.preventDefault();
        openRecharge();
        return;
      }
      if (e.key === 'F8') {
        e.preventDefault();
        openExpense();
        return;
      }
      if (e.key === 'F9' || e.key === 'F10') {
        e.preventDefault();
        payNow(e.key === 'F9' ? 'cash' : 'card');
        return;
      }
      if (e.key === 'F2') {
        e.preventDefault();
        searchRef.current?.focus();
        return;
      }
      const typing = !!(e.target as HTMLElement).closest?.('input, textarea, select');
      if (typing) return;
      if (e.ctrlKey && e.key.toLowerCase() === 'z') {
        e.preventDefault();
        undo();
      } else if (e.key.length === 1 && !e.ctrlKey && !e.altKey && !e.metaKey) {
        searchRef.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  // tiles keep their place between sales so cashiers can rely on muscle memory
  const favs = useMemo(() => products.filter((p) => p.favorite).sort((a, b) => a.name.localeCompare(b.name, 'es')), [products]);
  const everything = useMemo(() => [...products].sort((a, b) => a.name.localeCompare(b.name, 'es')), [products]);
  const tabs: { id: Tab; label: string; icon: typeof Star; count: number }[] = [
    { id: 'recent', label: tr('Recientes', 'Recent'), icon: History, count: recent.length },
    { id: 'fav', label: tr('Favoritos', 'Favourites'), icon: Star, count: favs.length },
    { id: 'all', label: tr('Todos', 'All'), icon: LayoutGrid, count: products.length },
  ];
  const editing = item && item !== 'add' ? lines.find((l) => l.key === item) : undefined;

  const paid = last?.ticket;
  const change = paid && paid.payment.method === 'cash' ? paid.payment.changeCents : 0;
  const printerTrouble = !!printer && printer.state !== 'ready' && printer.state !== 'none';

  return (
    <div className="sale" hidden={!active}>
      <section className="sale-main">
        <div className="bar-row">
          <div className="bar">
            <div className="bar-box">
              <Search size={22} />
              <input
                ref={searchRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={onSearchKey}
                role="combobox"
                aria-expanded={!!query.trim()}
                aria-controls="search-options"
                aria-activedescendant={query.trim() && options[hi] ? `opt-${hi}` : undefined}
                aria-autocomplete="list"
                placeholder={tr('Escribe o escanea:  funda 10  ·  -pantalla 45  ·  recarga 10', 'Type or scan:  case 10  ·  -screen 45  ·  recarga 10')}
                spellCheck={false}
                autoComplete="off"
                aria-label={tr('Buscar producto', 'Search product')}
              />
              {query && (
                <button className="icon-btn" onMouseDown={keep} onClick={() => setQuery('')} aria-label={tr('Borrar', 'Clear')}>
                  <X size={18} />
                </button>
              )}
            </div>
            {query.trim() && (
              <div className="dropdown" role="listbox" id="search-options">
                {options.length === 0 && (
                  <div className="opt-empty">
                    {/^\d{6,}$/.test(query.trim())
                      ? tr('Ningún producto con ese código', 'No product with that code')
                      : tr('Sin resultados. Añade un precio para venderlo: «funda 10»', 'No results. Add a price to sell it: "case 10"')}
                  </div>
                )}
                {options.map((o, i) => (
                  <div
                    key={o.kind === 'product' ? `p${o.product.id}` : o.kind}
                    id={`opt-${i}`}
                    role="option"
                    aria-selected={i === hi}
                    className={`opt${i === hi ? ' hi' : ''}`}
                    onMouseDown={(e) => {
                      e.preventDefault();
                      pick(o);
                    }}
                    onMouseEnter={() => setHi(i)}
                  >
                    {o.kind === 'product' && (
                      <>
                        {o.qty > 1 && <span className="opt-qty">{o.qty}×</span>}
                        <span className="opt-name">{o.product.name}</span>
                        {o.product.favorite && <Star size={13} className="star" />}
                        {o.product.barcode && <span className="opt-meta mono">{o.product.barcode}</span>}
                        <span className="opt-price">{o.product.priceCents ? eur(o.product.priceCents) : tr('Precio libre', 'Open price')}</span>
                      </>
                    )}
                    {o.kind === 'custom' && (
                      <>
                        <span className="opt-plus">
                          <Plus size={14} />
                        </span>
                        <span className="opt-name">
                          {tr('Añadir', 'Add')} «{o.name}»{o.qty > 1 ? ` ×${o.qty}` : ''}
                        </span>
                        <span className="opt-price">{eur(o.cents)}</span>
                      </>
                    )}
                    {o.kind === 'expense' && (
                      <>
                        <span className="opt-plus expense">
                          <ShoppingCart size={14} />
                        </span>
                        <span className="opt-name">
                          {tr('Gasto', 'Expense')}: {o.concept}
                        </span>
                        <span className="opt-price">{eur(-o.cents)}</span>
                      </>
                    )}
                    {o.kind === 'recharge' && (
                      <>
                        <span className="opt-plus recharge">
                          <SmartphoneCharging size={14} />
                        </span>
                        <span className="opt-name">{tr('Recarga en efectivo', 'Recharge in cash')}</span>
                        <span className="opt-price">{eur(o.cents)}</span>
                      </>
                    )}
                    <CornerDownLeft size={14} className="opt-enter" />
                  </div>
                ))}
              </div>
            )}
          </div>
          <button className="btn-add" onMouseDown={keep} onClick={() => setItem('add')} title={tr('Añadir un artículo a mano', 'Add an item by hand')}>
            <Plus size={20} />
            <span>{tr('Artículo', 'Item')}</span>
            <kbd>F4</kbd>
          </button>
        </div>

        <div className="browse">
          <div className="browse-tabs" role="tablist" aria-label={tr('Productos', 'Products')}>
            {tabs.map((t) => (
              <button key={t.id} role="tab" aria-selected={tab === t.id} className={tab === t.id ? 'on' : ''} onMouseDown={keep} onClick={() => setTab(t.id)}>
                <t.icon size={15} />
                {t.label}
                {t.count > 0 && <small>{t.count}</small>}
              </button>
            ))}
          </div>
          <div className="tiles" role="tabpanel">
            {tab === 'recent' &&
              recent.map((r) => (
                <button
                  key={`${r.name}-${r.unitCents}-${r.vatBp}`}
                  className="tile"
                  onMouseDown={keep}
                  onClick={() => addMany([toDraft({ ...r, qty: 1 })])}
                  title={`${tr('Vendido', 'Sold')} ${r.times} ${r.times === 1 ? tr('vez', 'time') : tr('veces', 'times')}`}
                >
                  <span className="tile-name">{r.name}</span>
                  <span className="tile-meta">
                    <b>{eur(r.unitCents)}</b>
                    {!r.productId && <small>{tr('libre', 'custom')}</small>}
                  </span>
                </button>
              ))}
            {tab !== 'recent' &&
              (tab === 'fav' ? favs : everything.slice(0, ALL_SHOWN)).map((p) => (
                <button key={p.id} className={`tile${p.favorite ? ' fav' : ''}`} onMouseDown={keep} onClick={() => addProduct(p)}>
                  <span className="tile-name">{p.name}</span>
                  <span className="tile-meta">
                    {p.priceCents ? <b>{eur(p.priceCents)}</b> : <small>{tr('Precio libre', 'Open price')}</small>}
                  </span>
                </button>
              ))}
            {tab === 'recent' && recent.length === 0 && <p className="tiles-empty">{tr('Aquí saldrá lo último que vendas, para repetirlo con un toque.', 'What you sell shows up here, to sell it again with one tap.')}</p>}
            {tab === 'fav' && favs.length === 0 && (
              <p className="tiles-empty">{tr('Marca productos con la estrella en Productos, o al añadir un artículo, para tenerlos aquí.', 'Star products in Products, or when adding an item, to have them here.')}</p>
            )}
            {tab === 'all' && products.length === 0 && <p className="tiles-empty">{tr('Aún no hay productos: añade un artículo y guárdalo como producto.', 'No products yet: add an item and save it as a product.')}</p>}
            {tab === 'all' && products.length > ALL_SHOWN && (
              <p className="tiles-empty">{tr(`Y ${products.length - ALL_SHOWN} más: búscalos arriba.`, `And ${products.length - ALL_SHOWN} more: search above.`)}</p>
            )}
          </div>
        </div>

        {day && <DayStrip s={day} onOpen={onDay} />}
      </section>

      <aside className="till">
        <div className="till-head">
          <h2>{lines.length ? `${count} ${count === 1 ? tr('artículo', 'item') : tr('artículos', 'items')}` : tr('Venta', 'Sale')}</h2>
          <div className="till-tools">
            <button className="icon-btn" onMouseDown={keep} onClick={() => setSeeTicket(true)} disabled={!lines.length} title={tr('Ver el ticket', 'See the ticket')} aria-label={tr('Ver el ticket', 'See the ticket')}>
              <Eye size={17} />
            </button>
            <button className="icon-btn" onMouseDown={keep} onClick={undo} disabled={!history.current.length} title={`${tr('Deshacer', 'Undo')} (Ctrl+Z)`} aria-label={tr('Deshacer', 'Undo')}>
              <Undo2 size={17} />
            </button>
            <button className="icon-btn" onMouseDown={keep} onClick={clear} disabled={!lines.length} title={tr('Vaciar', 'Clear')} aria-label={tr('Vaciar', 'Clear')}>
              <Trash2 size={17} />
            </button>
          </div>
        </div>

        {fixing && (
          <div className="fixing">
            <Pencil size={15} />
            <span>
              {tr('Corrigiendo', 'Correcting')} <b className="mono">{fixing.number}</b>
            </span>
            <button className="link" onMouseDown={keep} onClick={dropFix}>
              {tr('Cancelar', 'Cancel')}
            </button>
          </div>
        )}

        <div className="till-lines" ref={linesRef}>
          {lines.map((l) => (
            <div key={l.key} className={`ln${sel === l.key ? ' sel' : ''}`} onMouseDown={() => setSel(l.key)}>
              {flash?.key === l.key && <i className="row-flash" key={flash.n} />}
              <button
                className="ln-name"
                onMouseDown={keep}
                onClick={() => (l.repairId ? setPriceEdit(l.key) : setItem(l.key))}
                title={l.repairId ? tr('Cambiar precio', 'Change price') : tr('Cambiar nombre, precio o IVA', 'Change name, price or VAT')}
              >
                <span>{l.name}</span>
                <Pencil size={12} aria-hidden="true" />
              </button>
              <b className="ln-total">{fmt(l.qty * l.unitCents)}</b>
              <button className="ln-x" onMouseDown={keep} onClick={() => remove(l.key)} aria-label={tr('Quitar', 'Remove')}>
                <X size={15} />
              </button>
              <div className="stepper">
                <button onMouseDown={keep} onClick={() => setQty(l.key, l.qty - 1)} disabled={l.qty <= 1 || !!l.repairId} aria-label={tr('Uno menos', 'One less')}>
                  <Minus size={15} />
                </button>
                <QtyInput qty={l.qty} onChange={(n) => setQty(l.key, n)} onExit={() => searchRef.current?.focus()} disabled={!!l.repairId} />
                <button onMouseDown={keep} onClick={() => setQty(l.key, l.qty + 1)} disabled={l.qty >= 999 || !!l.repairId} aria-label={tr('Uno más', 'One more')}>
                  <Plus size={15} />
                </button>
              </div>
              <div className="ln-price">
                {priceEdit === l.key ? (
                  <PriceInput
                    cents={l.unitCents}
                    onDone={(c) => {
                      setPriceEdit(null);
                      if (c !== null && c !== l.unitCents) setPrice(l.key, c);
                      searchRef.current?.focus();
                    }}
                  />
                ) : (
                  <button className={`price-btn${l.unitCents === 0 ? ' zero' : ''}`} onClick={() => setPriceEdit(l.key)} title={tr('Toca para cambiar el precio', 'Tap to change the price')}>
                    {fmt(l.unitCents)}
                    <Pencil size={12} className="price-edit" aria-hidden="true" />
                  </button>
                )}
              </div>
              {(l.repairId || l.vatBp !== settings.defaultVatBp) && (
                <small className="ln-note">
                  {l.repairId && tr('Reparación', 'Repair')}
                  {l.repairId && l.vatBp !== settings.defaultVatBp && ' · '}
                  {l.vatBp !== settings.defaultVatBp && `IVA ${rate(l.vatBp)}`}
                </small>
              )}
            </div>
          ))}

          {!lines.length && paid && (
            <div className="done">
              <span className="done-check">
                <Check size={26} strokeWidth={3} />
              </span>
              <strong>{eur(paid.totalCents)}</strong>
              <span className="muted mono">{paid.number}</span>
              <div className="done-pay" role="radiogroup" aria-label={tr('Pagado con', 'Paid with')}>
                {METHODS.filter((m) => m.id !== 'bizum' || settings.payBizum || paid.payment.method === 'bizum').map((m) => (
                  <button key={m.id} role="radio" aria-checked={paid.payment.method === m.id} className={paid.payment.method === m.id ? 'on' : ''} onMouseDown={keep} onClick={() => changePay(m.id)}>
                    {m.label()}
                  </button>
                ))}
              </div>
              {change > 0 && (
                <div className="done-change">
                  <span>{tr('Cambio', 'Change')}</span>
                  <strong>{eur(change)}</strong>
                </div>
              )}
              <div className="done-actions">
                {!noPrinter && (
                  <button className="btn" onMouseDown={keep} onClick={() => reprint(paid.id, !!last?.printed).then(() => setLast((cur) => (cur ? { ...cur, printed: true } : cur)))}>
                    <Printer size={16} />
                    {last?.printed ? tr('Copia', 'Copy') : tr('Imprimir', 'Print')}
                  </button>
                )}
                <button className="btn" onMouseDown={keep} onClick={() => setSharing(paid)} title={tr('Enviar por WhatsApp, email o PDF', 'Send by WhatsApp, email or PDF')}>
                  <Send size={16} />
                  {tr('Enviar', 'Send')}
                </button>
                {repeatable(paid) && (
                  <button className="btn" onMouseDown={keep} onClick={() => repeat(paid)} title={tr('Añadir los mismos artículos a una venta nueva', 'Add the same items to a new sale')}>
                    <Repeat2 size={16} />
                    {tr('Repetir', 'Repeat')}
                  </button>
                )}
              </div>
            </div>
          )}

          {!lines.length && !paid && (
            <div className="till-empty">
              <ScanBarcode size={30} />
              <p>{tr('Escribe, escanea o toca un producto.', 'Type, scan or tap a product.')}</p>
            </div>
          )}
        </div>

        <div className="till-foot">
          <div className="till-total">
            <span>{tr('Total', 'Total')}</span>
            <strong>{eur(total)}</strong>
          </div>
          {negative && <p className="form-error">{tr('El total es negativo: revisa los descuentos.', 'The total is negative: check the discounts.')}</p>}

          {noPrinter ? (
            <button className="print-row" onMouseDown={keep} onClick={onSetupPrinter} disabled={!onSetupPrinter}>
              <Printer size={17} />
              <span>{tr('Sin impresora: las ventas se guardan sin imprimir', 'No printer: sales are saved without printing')}</span>
            </button>
          ) : (
            <button className={`print-row${printOn ? ' on' : ''}`} role="switch" aria-checked={printOn} onMouseDown={keep} onClick={() => setPrintOn(!printOn)} title="F6">
              <Printer size={17} />
              <span>
                {printOn ? tr('Imprimir ticket', 'Print the ticket') : tr('Sin imprimir', 'No printing')}
                {(printerTrouble || lastFailed) && <small> · {lastFailed ? tr('el último no salió', 'the last one failed') : printerStateText(printer!.state)}</small>}
              </span>
              <i className="switch" aria-hidden="true" />
            </button>
          )}

          <div className={`tender${lines.length && !negative ? '' : ' off'}`}>
            <span>{tr('Entrega', 'Given')}</span>
            {quick(total, 3).map((c) => (
              <button key={c} onMouseDown={keep} onClick={() => payNow('cash', c)} disabled={!lines.length || busy || negative} title={tr('Con este billete: se ve el cambio', 'With this note: shows the change')}>
                {fmt(c).replace(',00', '')} €
              </button>
            ))}
            <button className="tender-more" onMouseDown={keep} onClick={openPay} disabled={!lines.length || busy || negative} title={tr('Otra cantidad, factura con datos del cliente…', 'Another amount, invoice with customer details…')}>
              {tr('Más', 'More')}
              <kbd>F12</kbd>
            </button>
          </div>

          <div className={`pay-grid${settings.payBizum ? ' three' : ''}`}>
            <button className="pay-btn pay-cash" onMouseDown={keep} onClick={() => payNow('cash')} disabled={!lines.length || busy || negative}>
              <Banknote size={26} />
              <span>{tr('Efectivo', 'Cash')}</span>
              <kbd>F9</kbd>
            </button>
            <button className="pay-btn pay-card" onMouseDown={keep} onClick={() => payNow('card')} disabled={!lines.length || busy || negative}>
              <CreditCard size={26} />
              <span>{tr('Tarjeta', 'Card')}</span>
              <kbd>F10</kbd>
            </button>
            {settings.payBizum && (
              <button className="pay-btn pay-bizum" onMouseDown={keep} onClick={() => payNow('bizum')} disabled={!lines.length || busy || negative}>
                <Smartphone size={26} />
                <span>Bizum</span>
              </button>
            )}
          </div>
          <div className="pay-grid low">
            <button className="pay-btn pay-recharge" onMouseDown={keep} onClick={openRecharge}>
              <SmartphoneCharging size={21} />
              <span>{tr('Recarga', 'Recharge')}</span>
              <kbd>F7</kbd>
            </button>
            <button className="pay-btn pay-expense" onMouseDown={keep} onClick={openExpense}>
              <ShoppingCart size={21} />
              <span>{tr('Gasto', 'Expense')}</span>
              <kbd>F8</kbd>
            </button>
          </div>
        </div>
      </aside>

      {paying && (
        <div className="pay-back" onMouseDown={() => !busy && setPaying(false)}>
          <div className="pay" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label={tr('Cobrar', 'Charge')} ref={payRef}>
            <div className="pay-head">
              <h2>{tr('Cobrar', 'Charge')}</h2>
              {method === 'cash' && till === null && (
                <div className="till-note">
                  <Wallet size={17} />
                  <span>{tr('Caja cerrada', 'Till closed')}</span>
                  <div className="money-input">
                    <input
                      value={float}
                      onChange={(e) => setFloat(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key !== 'Enter') return;
                        e.preventDefault();
                        e.stopPropagation();
                        openTill();
                      }}
                      inputMode="decimal"
                      placeholder={tr('Fondo', 'Float')}
                      aria-label={tr('Fondo inicial', 'Starting float')}
                    />
                    <span>€</span>
                  </div>
                  <button className="btn sm" onMouseDown={keep} onClick={openTill} disabled={opening || floatCents === null || floatCents < 0}>
                    {tr('Abrir caja', 'Open till')}
                  </button>
                </div>
              )}
              <button className="icon-btn" onMouseDown={keep} onClick={() => setPaying(false)} disabled={busy} aria-label={tr('Cerrar', 'Close')}>
                <X size={18} />
              </button>
            </div>
            <div className="pay-body">
              <div className="pay-total">
                <span>{tr('Total a cobrar', 'Amount due')}</span>
                <strong>{eur(total)}</strong>
              </div>
              <div className="methods">
                {METHODS.map((m) => (
                  <button key={m.id} className={`method${method === m.id ? ' on' : ''}`} onMouseDown={keep} onClick={() => setMethod(m.id)}>
                    <m.icon size={20} />
                    <span>{m.label()}</span>
                    <kbd>{m.key}</kbd>
                  </button>
                ))}
              </div>
              {method === 'cash' ? (
                <div className="cash">
                  <label className="field">
                    <span className="field-label">{tr('Entregado por el cliente', 'Received from the customer')}</span>
                    <div className="money-input big">
                      <input ref={givenRef} autoFocus value={given} onChange={(e) => setGiven(e.target.value)} placeholder={fmt(total)} inputMode="decimal" />
                      <span>€</span>
                    </div>
                  </label>
                  <div className="quick">
                    <button className={given === '' ? 'on' : ''} onMouseDown={keep} onClick={() => setGiven('')}>
                      {tr('Exacto', 'Exact')}
                    </button>
                    {quick(total).map((c) => (
                      <button key={c} className={tendered === c ? 'on' : ''} onMouseDown={keep} onClick={() => setGiven(fmt(c))}>
                        {eur(c)}
                      </button>
                    ))}
                  </div>
                  {badAmount || tooMuch ? (
                    <div className="change short">
                      <span>{tooMuch ? tr('Importe demasiado alto', 'Amount too high') : tr('Importe no válido', 'Invalid amount')}</span>
                      <strong>{given}</strong>
                    </div>
                  ) : (
                    <div className={`change${short ? ' short' : ''}`}>
                      <span>{short ? tr('Falta', 'Still due') : tr('Cambio', 'Change')}</span>
                      <strong>{eur(short ? total - (tendered ?? 0) : preview?.payment?.changeCents ?? 0)}</strong>
                    </div>
                  )}
                </div>
              ) : (
                <p className="pay-note">
                  {method === 'card'
                    ? tr('Cobra en el datáfono y confirma aquí.', 'Take the payment on the card terminal, then confirm here.')
                    : tr('Comprueba que el Bizum ha llegado y confirma.', 'Check that the Bizum payment has arrived, then confirm.')}
                </p>
              )}
              {invoice ? (
                <div className="customer">
                  <div className="customer-head">
                    <b>{tr('Factura con datos del cliente', 'Invoice with customer details')}</b>
                    {!overLimit && (
                      <button className="link" onMouseDown={keep} onClick={() => setInvoice(false)}>
                        {tr('Quitar', 'Remove')}
                      </button>
                    )}
                  </div>
                  {overLimit && (
                    <p className="pay-hint">
                      {service
                        ? tr('Con una reparación, más de 400 €: hace falta una factura con los datos del cliente.', 'With a repair, over 400 EUR: this needs an invoice with the customer details.')
                        : tr('Más de 3.000 €: hace falta una factura con los datos del cliente.', 'Over 3,000 EUR: this needs an invoice with the customer details.')}
                    </p>
                  )}
                  <div className="grid2">
                    <Field label={tr('Nombre o razón social', 'Name or company')}>
                      <input value={customer.name} onChange={(e) => setCustomer({ ...customer, name: e.target.value })} maxLength={120} />
                    </Field>
                    <Field label="NIF" error={customer.nif.trim() && !customerNif(customer.nif) ? tr('Revisa el NIF', 'Check the NIF') : undefined}>
                      <input value={customer.nif} onChange={(e) => setCustomer({ ...customer, nif: e.target.value.toUpperCase() })} maxLength={20} />
                    </Field>
                  </div>
                  <Field label={tr('Dirección', 'Address')}>
                    <input value={customer.address} onChange={(e) => setCustomer({ ...customer, address: e.target.value })} maxLength={200} />
                  </Field>
                </div>
              ) : (
                <button className="link invoice-link" onMouseDown={keep} onClick={() => setInvoice(true)}>
                  {tr('Factura con datos del cliente', 'Invoice with customer details')}
                </button>
              )}
            </div>
            <div className="pay-actions">
              {noPrinter ? (
                <>
                  <button className="btn btn-primary btn-xl" disabled={busy || blocked} onMouseDown={keep} onClick={() => confirmPay(false)}>
                    <Save size={20} />
                    {busy ? tr('Guardando…', 'Saving…') : tr('Guardar venta', 'Save the sale')}
                    <kbd className="on-dark">Enter</kbd>
                  </button>
                  <p className="pay-hint">
                    {tr('No hay impresora configurada: la venta se guarda y se puede imprimir después desde Hoy.', 'No printer is set up: the sale is saved and can be printed later from Today.')}
                    {onSetupPrinter && (
                      <>
                        {' '}
                        <button className="link" onMouseDown={keep} onClick={onSetupPrinter}>
                          {tr('Configurar impresora', 'Set up printer')}
                        </button>
                      </>
                    )}
                  </p>
                </>
              ) : (
                <>
                  <button className="btn btn-primary btn-xl" disabled={busy || blocked} onMouseDown={keep} onClick={() => confirmPay(printOn)}>
                    {printOn ? <Printer size={20} /> : <Save size={20} />}
                    {busy ? tr('Guardando…', 'Saving…') : printOn ? tr('Cobrar e imprimir', 'Charge and print') : tr('Cobrar sin imprimir', 'Charge without printing')}
                    <kbd className="on-dark">Enter</kbd>
                  </button>
                  <button className="btn btn-ghost" disabled={busy || blocked} onMouseDown={keep} onClick={() => confirmPay(!printOn)}>
                    {printOn ? tr('Cobrar sin imprimir', 'Charge without printing') : tr('Cobrar e imprimir', 'Charge and print')}
                    <kbd>Shift+Enter</kbd>
                  </button>
                </>
              )}
            </div>
          </div>
        </div>
      )}

      {sharing && <ShareDialog ticketId={sharing.id} number={sharing.number} onClose={() => setSharing(null)} />}
      {seeTicket && preview && (
        <Modal title={tr('Así saldrá el ticket', 'This is how the ticket will look')} onClose={() => setSeeTicket(false)}>
          <div className="see-ticket">
            <Paper lines={preview.lines} logo={logo} cols={settings.columns} />
          </div>
          {previewErr && <p className="form-error">{previewErr}</p>}
        </Modal>
      )}
      {recharge && (
        <RechargeModal
          settings={settings}
          cents={recharge.cents}
          onSave={saveEntry}
          onClose={(saved) => {
            if (saved && recharge.fromCart) {
              history.current = [];
              setLines([]);
              setSel(null);
            }
            setRecharge(null);
            searchRef.current?.focus();
          }}
        />
      )}
      {expense !== null && (
        <ExpenseModal
          initial={expense}
          settings={settings}
          onSave={saveEntry}
          onClose={() => {
            setExpense(null);
            searchRef.current?.focus();
          }}
        />
      )}
      {clock && (
        <ClockModal
          last={clock.last}
          onCancel={() => setClock(null)}
          onConfirm={() => {
            const c = clock.charge;
            setClock(null);
            finish(c, true);
          }}
        />
      )}
      {item && (item === 'add' || editing) && (
        <ItemDialog
          line={editing ? toDraft(editing) : undefined}
          products={products}
          defaultVat={settings.defaultVatBp}
          admin={admin}
          onCatalogue={onCatalogue}
          onClose={() => {
            setItem(null);
            searchRef.current?.focus();
          }}
          onRemove={
            editing
              ? () => {
                  remove(editing.key);
                  setItem(null);
                  searchRef.current?.focus();
                }
              : undefined
          }
          onDone={(d) => {
            if (editing) changeLine(editing.key, d);
            else addMany([d]);
            setItem(null);
            searchRef.current?.focus();
          }}
        />
      )}
    </div>
  );
}
