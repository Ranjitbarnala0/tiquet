import { useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKey, type MouseEvent as ReactMouse } from 'react';
import {
  Banknote,
  CornerDownLeft,
  CreditCard,
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
  Smartphone,
  Star,
  Trash2,
  Undo2,
  X,
} from 'lucide-react';
import { ApiError, call, toError } from './api';
import { eur, fmt, norm, parseMoney, rate } from './format';
import { errorText, isPrinterSetup, isTransient, printerStateText, tr, warningText } from './i18n';
import { ClockModal } from './Clock';
import { ItemDialog, type ItemDraft } from './ItemDialog';
import { customerNif } from './nif';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { CartLine, Customer, Finalized, Logo, Method, Preview, Product, Recent, Settings, TicketRecord } from './types';

type Shown = Preview & { forLines: string };
import { usePrinterStatus } from './usePrinter';
import { Field, useTrap } from './ui';

type Option =
  | { kind: 'product'; product: Product; qty: number }
  | { kind: 'custom'; name: string; cents: number; qty: number };

let seq = 0;
const uid = () => `l${Date.now().toString(36)}${(seq++).toString(36)}`;
const newSaleId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

// what survives a crash: the lines, and the id the core uses to tell a retry
// of this same cart from a new sale
function storedCart(key: string): { saleId: string; lines: CartLine[] } {
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
  return out;
}

function quick(total: number): number[] {
  const out = new Set<number>();
  for (const step of [100, 500, 1000, 2000, 5000, 10000]) {
    const v = Math.ceil(total / step) * step;
    if (v > total) out.add(v);
  }
  return [...out].sort((a, b) => a - b).slice(0, 4);
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

function QtyInput({ qty, onChange, onExit }: { qty: number; onChange: (n: number) => void; onExit: () => void }) {
  const [v, setV] = useState(String(qty));
  useEffect(() => setV(String(qty)), [qty]);
  return (
    <input
      className="qty-input"
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
  // items handed over from another screen, e.g. a ticket repeated from Tickets
  incoming: { lines: ItemDraft[]; from: string; n: number } | null;
  onCatalogue: () => void;
  settings: Settings;
  logo: Logo | null;
  products: Product[];
  onSold: () => void;
  onSetupPrinter?: () => void;
}

export function Sale({ cartKey, active, admin, incoming, onCatalogue, settings, logo, products, onSold, onSetupPrinter }: Props) {
  const toast = useToast();
  const [restored] = useState(() => storedCart(cartKey));
  const [lines, setLines] = useState<CartLine[]>(restored.lines);
  const saleId = useRef(restored.saleId);
  const [clock, setClock] = useState<{ last: string; print: boolean } | null>(null);
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
  const [last, setLast] = useState<TicketRecord | null>(null);
  // the add/change dialog: 'add', or the key of the line being changed
  const [item, setItem] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>(storedTab);
  const [recent, setRecent] = useState<Recent[]>([]);
  const searchRef = useRef<HTMLInputElement>(null);
  const givenRef = useRef<HTMLInputElement>(null);
  const paperRef = useRef<HTMLDivElement>(null);
  const payRef = useRef<HTMLDivElement>(null);
  const reqSeq = useRef(0);
  const openedAt = useRef(0);
  const burst = useRef({ text: '', last: 0, fast: 0, given: '' });
  const pendingScans = useRef<string[]>([]);
  const printing = useRef(false);
  const noPrinter = !settings.printerName.trim();
  const { status: printer, refresh: refreshPrinter } = usePrinterStatus(active, settings.printerName);

  // survives a crash or a power cut mid-sale
  useEffect(() => {
    try {
      localStorage.setItem(cartKey, JSON.stringify({ saleId: saleId.current, lines }));
    } catch {
      // storage unavailable: the cart simply isn't restored
    }
  }, [lines, cartKey]);

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

  useEffect(() => {
    try {
      localStorage.setItem(TAB_KEY, tab);
    } catch {
      // the tab just will not be remembered
    }
  }, [tab]);

  const catalogue = useMemo(() => index(products), [products]);
  const options = useMemo(() => search(catalogue, query), [catalogue, query]);
  useEffect(() => setHi(0), [query]);

  const tendered = method === 'cash' ? parseMoney(given, 999_999_999) : null;
  const payload = lines.map(({ productId, name, qty, unitCents, vatBp }) => ({ productId, name, qty, unitCents, vatBp }));
  const linesKey = JSON.stringify(payload);
  const customerOk = !invoice || (customer.name.trim().length > 0 && customerNif(customer.nif));
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
    const el = paperRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
  }, [preview?.lines.length, paying]);

  useEffect(() => {
    if (!active) setPaying(false);
  }, [active]);

  useTrap(payRef, paying);

  useEffect(() => {
    if (active && !paying) searchRef.current?.focus();
  }, [active, paying]);

  // the queue can look ready while the last ticket did not come out
  const lastFailed = !!printer && printer.state === 'ready' && (printer.last?.status === 'failed' || printer.last?.status === 'unknown');
  const total = preview?.totals.totalCents ?? 0;
  const count = preview?.totals.itemCount ?? 0;
  // the total on screen belongs to the cart on screen
  const fresh = !!preview && preview.forLines === linesKey;
  const short = method === 'cash' && tendered !== null && tendered < total;
  const badAmount = method === 'cash' && given.trim() !== '' && tendered === null;
  const tooMuch = method === 'cash' && !short && preview?.paymentError === 'E_TENDERED_HIGH';
  const negative = total < 0;
  // a simplified invoice stops at 3.000 EUR; past that it needs the customer
  const overLimit = total > 300_000;
  const blocked = short || badAmount || tooMuch || negative || !fresh || !customerOk || (overLimit && !invoice);

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

  // adds items, merging with a line that is the same thing at the same price
  const addMany = (items: ItemDraft[]) => {
    let next = lines;
    let lastKey: string | null = null;
    for (const d of items) {
      const same = next.find(
        (l) => l.name === d.name && l.unitCents === d.unitCents && l.vatBp === d.vatBp && (l.productId ?? null) === (d.productId ?? null),
      );
      if (same) {
        next = next.map((l) => (l.key === same.key ? { ...l, qty: Math.min(999, l.qty + d.qty) } : l));
        lastKey = same.key;
      } else {
        const line: CartLine = { key: uid(), name: d.name, qty: d.qty, unitCents: d.unitCents, vatBp: d.vatBp, productId: d.productId };
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
    addMany(t.lines.filter((l) => l.qty > 0).map(toDraft));
    toast.ok(`${tr('Añadidos los artículos de', 'Added the items from')} ${t.number}`);
  };

  const handled = useRef(0);
  useEffect(() => {
    if (!incoming || incoming.n === handled.current) return;
    handled.current = incoming.n;
    addMany(incoming.lines);
    toast.ok(`${tr('Añadidos los artículos de', 'Added the items from')} ${incoming.from}`);
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
    if (o.kind === 'product') addProduct(o.product, o.qty);
    else addCustom(o.name, o.cents, o.qty);
    setQuery('');
  };

  const setQty = (key: string, qty: number) =>
    commit(lines.map((l) => (l.key === key ? { ...l, qty: Math.min(999, Math.max(1, Math.round(qty))) } : l)));

  const setPrice = (key: string, cents: number) => commit(lines.map((l) => (l.key === key ? { ...l, unitCents: cents } : l)));

  const remove = (key: string) => {
    const i = lines.findIndex((l) => l.key === key);
    commit(lines.filter((l) => l.key !== key));
    setSel(lines[i + 1]?.key ?? lines[i - 1]?.key ?? null);
  };

  const clear = () => {
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

  const openPay = () => {
    if (!lines.length || busy) return;
    if (negative) {
      toast.error(errorText(new ApiError('E_NEGATIVE', '')));
      return;
    }
    const unpriced = lines.find((l) => l.unitCents === 0);
    if (unpriced) {
      toast.error(`${tr('Falta el precio de', 'Missing price for')} «${unpriced.name}»`);
      setSel(unpriced.key);
      setPriceEdit(unpriced.key);
      return;
    }
    searchRef.current?.blur();
    setQuery('');
    setMethod('cash');
    setGiven('');
    openedAt.current = performance.now();
    burst.current = { text: '', last: 0, fast: 0, given: '' };
    setInvoice(total > 300_000);
    setCustomer({ name: '', nif: '', address: '' });
    setPaying(true);
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

  const finish = async (print: boolean, clockOk = false) => {
    if (busy || !lines.length || blocked) return;
    setBusy(true);
    try {
      const r = await call<Finalized>('sale.finalize', {
        lines: payload,
        payment: { method, tenderedCents: tendered ?? undefined },
        print: print && !noPrinter,
        saleId: saleId.current,
        clockOk,
        expectedCents: total,
        customer: invoice ? customer : undefined,
      });
      history.current = [];
      saleId.current = newSaleId();
      const next = cartFromScans(pendingScans.current.splice(0));
      setLines(next);
      setSel(next[next.length - 1]?.key ?? null);
      setPaying(false);
      setGiven('');
      setLast(r.ticket);
      const p = r.ticket.payment;
      const change = p.method === 'cash' && p.changeCents > 0 ? ` · ${tr('Cambio', 'Change')} ${eur(p.changeCents)}` : '';
      if (r.duplicate) {
        toast.info(`${tr('Esta venta ya estaba cobrada', 'This sale had already been charged')}: ${r.ticket.number}`, { sticky: true });
      } else if (r.printError) {
        printFailed(`${r.ticket.number} ${tr('guardado, pero no se imprimió', 'saved, but not printed')}: `, toError(r.printError), () =>
          reprint(r.ticket.id, false),
        );
      } else {
        toast.ok(`${tr('Ticket', 'Ticket')} ${r.ticket.number}${change}`);
        if (r.printWarning) toast.info(warningText(r.printWarning));
        if (r.printed) refreshPrinter();
      }
      onSold();
      loadRecent();
    } catch (e) {
      if (e instanceof ApiError && e.code === 'E_CLOCK') setClock({ last: e.detail, print });
      else {
        if (e instanceof ApiError && e.code === 'E_OVER_LIMIT') setInvoice(true);
        toast.error(errorText(e));
      }
    } finally {
      setBusy(false);
    }
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
          finish(!e.shiftKey);
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

  return (
    <div className="sale" hidden={!active}>
      <section className="sale-main">
        <div className="search-row">
        <div className="search">
          <div className="search-box">
            <Search size={20} />
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
              placeholder={tr('Busca, escanea o escribe «funda 12,50»', 'Search, scan or type "case 12.50"')}
              spellCheck={false}
              autoComplete="off"
              aria-label={tr('Buscar producto', 'Search product')}
            />
            <kbd>F2</kbd>
          </div>
          {query.trim() && (
            <div className="dropdown" role="listbox" id="search-options">
              {options.length === 0 && (
                <div className="opt-empty">
                  {/^\d{6,}$/.test(query.trim())
                    ? tr('Ningún producto con ese código', 'No product with that code')
                    : tr('Sin resultados. Añade un precio para venderlo como artículo libre.', 'No results. Add a price to sell it as a custom item.')}
                </div>
              )}
              {options.map((o, i) => (
                <div
                  key={o.kind === 'product' ? `p${o.product.id}` : 'custom'}
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
                  {o.kind === 'product' ? (
                    <>
                      {o.qty > 1 && <span className="opt-qty">{o.qty}×</span>}
                      <span className="opt-name">{o.product.name}</span>
                      {o.product.favorite && <Star size={13} className="star" />}
                      {o.product.barcode && <span className="opt-meta mono">{o.product.barcode}</span>}
                      <span className="opt-price">{o.product.priceCents ? eur(o.product.priceCents) : tr('Precio libre', 'Open price')}</span>
                    </>
                  ) : (
                    <>
                      <span className="opt-plus">
                        <Plus size={14} />
                      </span>
                      <span className="opt-name">
                        {tr('Añadir', 'Add')} «{o.name}»{o.qty > 1 ? ` ×${o.qty}` : ''}
                      </span>
                      <span className="opt-meta">{tr('artículo libre', 'custom item')}</span>
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

        <div className="cart-bar">
          <span className="muted">
            {lines.length
              ? `${lines.length} ${lines.length === 1 ? tr('línea', 'line') : tr('líneas', 'lines')}`
              : tr('Ticket nuevo', 'New ticket')}
          </span>
          <div className="cart-actions">
            <button className="btn btn-ghost sm" onMouseDown={keep} onClick={undo} disabled={!history.current.length} title="Ctrl+Z">
              <Undo2 size={15} />
              {tr('Deshacer', 'Undo')}
            </button>
            <button className="btn btn-ghost sm" onMouseDown={keep} onClick={clear} disabled={!lines.length}>
              <Trash2 size={15} />
              {tr('Vaciar', 'Clear')}
            </button>
          </div>
        </div>

        <div className="cart">
          {lines.length === 0 ? (
            <div className="cart-empty">
              <button className="add-tile" onMouseDown={keep} onClick={() => setItem('add')}>
                <span className="add-plus">
                  <Plus size={34} strokeWidth={2.2} />
                </span>
                <b>{tr('Añadir artículo', 'Add an item')}</b>
                <span>{tr('Escribe nombre y precio. Si quieres, se guarda como producto.', 'Type a name and a price. It can be saved as a product.')}</span>
              </button>
              <p className="cart-empty-hint">
                <ScanBarcode size={16} />
                {tr('O escanea un código, busca arriba o toca un producto de abajo.', 'Or scan a barcode, search above or tap a product below.')}
              </p>
            </div>
          ) : (
            <>
              <div className="cart-head">
                <span>{tr('Artículo', 'Item')}</span>
                <span className="center">{tr('Cantidad', 'Qty')}</span>
                <span className="num">{tr('Precio', 'Price')}</span>
                <span className="num">{tr('Importe', 'Amount')}</span>
                <span />
              </div>
              {lines.map((l) => (
                <div key={l.key} className={`cart-row${sel === l.key ? ' sel' : ''}`} onMouseDown={() => setSel(l.key)}>
                  {flash?.key === l.key && <i className="row-flash" key={flash.n} />}
                  <button className="cart-name" onMouseDown={keep} onClick={() => setItem(l.key)} title={tr('Cambiar nombre, precio o IVA', 'Change name, price or VAT')}>
                    <span>
                      {l.name}
                      <Pencil size={13} className="cart-edit" aria-hidden="true" />
                    </span>
                    {(!l.productId || l.vatBp !== settings.defaultVatBp) && (
                      <small>
                        {!l.productId && tr('Artículo libre', 'Custom item')}
                        {!l.productId && l.vatBp !== settings.defaultVatBp && ' · '}
                        {l.vatBp !== settings.defaultVatBp && `IVA ${rate(l.vatBp)}`}
                      </small>
                    )}
                  </button>
                  <div className="stepper">
                    <button onMouseDown={keep} onClick={() => setQty(l.key, l.qty - 1)} disabled={l.qty <= 1} aria-label={tr('Uno menos', 'One less')}>
                      <Minus size={15} />
                    </button>
                    <QtyInput qty={l.qty} onChange={(n) => setQty(l.key, n)} onExit={() => searchRef.current?.focus()} />
                    <button onMouseDown={keep} onClick={() => setQty(l.key, l.qty + 1)} disabled={l.qty >= 999} aria-label={tr('Uno más', 'One more')}>
                      <Plus size={15} />
                    </button>
                  </div>
                  <div className="num">
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
                      <button className={`price-btn${l.unitCents === 0 ? ' zero' : ''}`} onClick={() => setPriceEdit(l.key)} title={tr('Cambiar precio', 'Change price')}>
                        {fmt(l.unitCents)}
                      </button>
                    )}
                  </div>
                  <div className="num strong">{fmt(l.qty * l.unitCents)}</div>
                  <button className="icon-btn" onMouseDown={keep} onClick={() => remove(l.key)} aria-label={tr('Quitar', 'Remove')}>
                    <X size={16} />
                  </button>
                </div>
              ))}
              <button className="add-row" onMouseDown={keep} onClick={() => setItem('add')}>
                <Plus size={16} />
                {tr('Añadir artículo', 'Add an item')}
                <kbd>F4</kbd>
              </button>
            </>
          )}
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

        {paying && (
          <div className="pay-back" onMouseDown={() => !busy && setPaying(false)}>
            <div className="pay" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label={tr('Cobrar', 'Charge')} ref={payRef}>
              <div className="pay-head">
                <h2>{tr('Cobrar', 'Charge')}</h2>
                <button className="icon-btn" onMouseDown={keep} onClick={() => setPaying(false)} disabled={busy} aria-label={tr('Cerrar', 'Close')}>
                  <X size={18} />
                </button>
              </div>
              <div className="pay-body">
                <div className="pay-total">
                  <span>{tr('Total a cobrar', 'Amount due')}</span>
                  <strong>{fresh ? eur(total) : '…'}</strong>
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
                    {overLimit && <p className="pay-hint">{tr('Más de 3.000 €: hace falta una factura con los datos del cliente.', 'Over 3,000 EUR: this needs an invoice with the customer details.')}</p>}
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
                    <button className="btn btn-primary btn-xl" disabled={busy || blocked} onMouseDown={keep} onClick={() => finish(false)}>
                      <Save size={20} />
                      {busy ? tr('Guardando…', 'Saving…') : tr('Guardar ticket', 'Save ticket')}
                      <kbd className="on-dark">Enter</kbd>
                    </button>
                    <p className="pay-hint">
                      {tr(
                        'No hay impresora configurada: el ticket se guarda y se puede imprimir después desde Tickets.',
                        'No printer is set up: the ticket is saved and can be printed later from Tickets.',
                      )}
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
                    <button className="btn btn-primary btn-xl" disabled={busy || blocked} onMouseDown={keep} onClick={() => finish(true)}>
                      <Printer size={20} />
                      {busy ? tr('Imprimiendo…', 'Printing…') : tr('Imprimir ticket', 'Print ticket')}
                      <kbd className="on-dark">Enter</kbd>
                    </button>
                    <button className="btn btn-ghost" disabled={busy || blocked} onMouseDown={keep} onClick={() => finish(false)}>
                      {tr('Guardar sin imprimir', 'Save without printing')}
                      <kbd>Shift+Enter</kbd>
                    </button>
                  </>
                )}
              </div>
            </div>
          </div>
        )}
      </section>

      <aside className="side">
        <div className="side-head">
          <span>{paying ? tr('Así se imprimirá', 'This is what will print') : tr('Vista previa del ticket', 'Ticket preview')}</span>
          {preview && <span className="mono">{preview.number}</span>}
        </div>
        {printer && (
          <button
            className={`printer-pill ${lastFailed ? 'paper' : printer.state}`}
            onMouseDown={keep}
            onClick={onSetupPrinter}
            disabled={!onSetupPrinter}
            title={printer.name || tr('Ajustes > Impresora', 'Settings > Printer')}
          >
            <i aria-hidden="true" />
            <span>{lastFailed ? tr('Último ticket sin imprimir', 'Last ticket not printed') : printerStateText(printer.state)}</span>
            {printer.name && <small>{printer.name}</small>}
            {printer.jobs > 0 && (
              <small>
                {printer.jobs} {tr('en cola', 'queued')}
              </small>
            )}
          </button>
        )}
        <div className="paper-scroll" ref={paperRef}>
          {preview && <Paper lines={preview.lines} logo={logo} cols={settings.columns} />}
          {previewErr && <p className="form-error">{previewErr}</p>}
        </div>
        <div className="checkout">
          {last && (
            <div className="last">
              <span>
                {tr('Último', 'Last')} <b className="mono">{last.number}</b> · {eur(last.totalCents)}
              </span>
              <span className="last-actions">
                <button className="link" onMouseDown={keep} onClick={() => repeat(last)} title={tr('Añadir los mismos artículos a este ticket', 'Add the same items to this ticket')}>
                  <Repeat2 size={14} />
                  {tr('Repetir', 'Repeat')}
                </button>
                {!noPrinter && (
                  <button className="link" onMouseDown={keep} onClick={() => reprint(last.id, true)}>
                    <Printer size={14} />
                    {tr('Copia', 'Copy')}
                  </button>
                )}
              </span>
            </div>
          )}
          <div className="checkout-total">
            <span>
              {tr('Total', 'Total')}
              {count > 0 && ` · ${count} ${count === 1 ? tr('unidad', 'unit') : tr('unidades', 'units')}`}
            </span>
            <strong>{eur(total)}</strong>
          </div>
          {negative && (
            <p className="form-error">{tr('El total es negativo: revisa los descuentos.', 'The total is negative: check the discounts.')}</p>
          )}
          <button className="btn-charge" disabled={!lines.length || busy || paying || negative} onMouseDown={keep} onClick={openPay}>
            <span>{tr('Cobrar', 'Charge')}</span>
            <kbd>F12</kbd>
          </button>
        </div>
      </aside>
      {clock && (
        <ClockModal
          last={clock.last}
          onCancel={() => setClock(null)}
          onConfirm={() => {
            const p = clock.print;
            setClock(null);
            finish(p, true);
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
