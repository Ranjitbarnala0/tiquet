import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKey, type MouseEvent as ReactMouse } from 'react';
import { Banknote, CornerDownLeft, CreditCard, Minus, Plus, Printer, ScanBarcode, Search, Smartphone, Star, Trash2, Undo2, X } from 'lucide-react';
import { call, toError } from './api';
import { eur, fmt, norm, parseMoney, rate } from './format';
import { errorText, tr } from './i18n';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { CartLine, Finalized, Logo, Method, Preview, Product, Settings, TicketRecord, User } from './types';

type Option =
  | { kind: 'product'; product: Product; qty: number }
  | { kind: 'custom'; name: string; cents: number; qty: number };

let seq = 0;
const uid = () => `l${Date.now().toString(36)}${(seq++).toString(36)}`;

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
    return c === null || c === 0 ? null : { name: 'Varios', cents: c };
  }
  const m = t.match(/^(.*\S)\s+(-?[\d.,]+)\s*€?$/);
  if (!m) return null;
  const c = parseMoney(m[2]);
  return c === null ? null : { name: m[1].slice(0, 120), cents: c };
}

function search(products: Product[], q: string): Option[] {
  const { qty, text } = parseQuery(q.trim());
  const t = text.trim();
  const out: Option[] = [];
  if (t) {
    const code = products.find((p) => p.barcode && p.barcode === t);
    if (code) out.push({ kind: 'product', product: code, qty });
    const tokens = norm(t).split(/\s+/).filter(Boolean);
    const scored: [number, Product][] = [];
    for (const p of products) {
      if (p === code) continue;
      const name = norm(p.name);
      if (!tokens.every((tok) => name.includes(tok) || (p.barcode && p.barcode.startsWith(tok)))) continue;
      let s = 0;
      if (name.startsWith(tokens[0])) s += 40;
      else if (name.split(/\s+/).some((w) => w.startsWith(tokens[0]))) s += 20;
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

interface Props {
  active: boolean;
  user: User;
  settings: Settings;
  logo: Logo | null;
  products: Product[];
  onSold: () => void;
}

export function Sale({ active, user, settings, logo, products, onSold }: Props) {
  const toast = useToast();
  const cartKey = `tiquet.cart.${user.id}`;
  const [lines, setLines] = useState<CartLine[]>(() => {
    try {
      const v = JSON.parse(localStorage.getItem(cartKey) ?? '[]');
      return Array.isArray(v) ? v : [];
    } catch {
      return [];
    }
  });
  const history = useRef<CartLine[][]>([]);
  const [query, setQuery] = useState('');
  const [hi, setHi] = useState(0);
  const [sel, setSel] = useState<string | null>(null);
  const [priceEdit, setPriceEdit] = useState<string | null>(null);
  const [flash, setFlash] = useState<{ key: string; n: number } | null>(null);
  const [paying, setPaying] = useState(false);
  const [method, setMethod] = useState<Method>('cash');
  const [given, setGiven] = useState('');
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewErr, setPreviewErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [last, setLast] = useState<TicketRecord | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const givenRef = useRef<HTMLInputElement>(null);
  const paperRef = useRef<HTMLDivElement>(null);
  const reqSeq = useRef(0);

  // survives a crash or a power cut mid-sale
  useEffect(() => {
    try {
      localStorage.setItem(cartKey, JSON.stringify(lines));
    } catch {
      // storage unavailable: the cart simply isn't restored
    }
  }, [lines, cartKey]);

  const options = useMemo(() => search(products, query), [products, query]);
  useEffect(() => setHi(0), [query]);

  const tendered = method === 'cash' ? parseMoney(given) : null;
  const payload = lines.map(({ productId, name, qty, unitCents, vatBp }) => ({ productId, name, qty, unitCents, vatBp }));
  const previewArgs = JSON.stringify({
    lines: payload,
    payment: paying ? { method, tenderedCents: tendered ?? undefined } : undefined,
  });

  // totals, VAT and the ticket itself always come from the core; requests can
  // overtake each other, so only the newest answer is applied
  useEffect(() => {
    const n = ++reqSeq.current;
    call<Preview>('sale.preview', JSON.parse(previewArgs))
      .then((p) => {
        if (n !== reqSeq.current) return;
        setPreview(p);
        setPreviewErr('');
      })
      .catch((e) => n === reqSeq.current && setPreviewErr(errorText(e)));
  }, [previewArgs, settings, logo]);

  useEffect(() => {
    const el = paperRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
  }, [preview?.lines.length, paying]);

  useEffect(() => {
    if (active && !paying) searchRef.current?.focus();
  }, [active, paying]);

  const total = preview?.totals.totalCents ?? 0;
  const count = preview?.totals.itemCount ?? 0;
  const short = method === 'cash' && tendered !== null && tendered < total;

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

  const addCustom = (name: string, cents: number, qty: number) => {
    const line: CartLine = { key: uid(), name, qty, unitCents: cents, vatBp: settings.defaultVatBp };
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
    commit([]);
    setSel(null);
    toast.info(tr('Ticket vaciado', 'Ticket cleared'), { action: { label: tr('Deshacer', 'Undo'), run: undo } });
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
    searchRef.current?.blur();
    setQuery('');
    setMethod('cash');
    setGiven('');
    setPaying(true);
  };

  const reprint = async (id: number, copy: boolean) => {
    try {
      await call('tickets.print', { id, copy });
      toast.ok(copy ? tr('Copia enviada a la impresora', 'Copy sent to the printer') : tr('Ticket enviado a la impresora', 'Ticket sent to the printer'));
    } catch (e) {
      toast.error(errorText(e), { action: { label: tr('Reintentar', 'Retry'), run: () => reprint(id, copy) } });
    }
  };

  const finish = async (print: boolean) => {
    if (busy || !lines.length || short) return;
    setBusy(true);
    try {
      const r = await call<Finalized>('sale.finalize', {
        lines: payload,
        payment: { method, tenderedCents: tendered ?? undefined },
        print,
      });
      history.current = [];
      setLines([]);
      setSel(null);
      setPaying(false);
      setGiven('');
      setLast(r.ticket);
      const p = r.ticket.payment;
      const change = p.method === 'cash' && p.changeCents > 0 ? ` · ${tr('Cambio', 'Change')} ${eur(p.changeCents)}` : '';
      if (r.printError) {
        toast.error(`${r.ticket.number} ${tr('guardado, pero no se imprimió', 'saved, but not printed')}: ${errorText(toError(r.printError))}`, {
          sticky: true,
          action: { label: tr('Reintentar', 'Retry'), run: () => reprint(r.ticket.id, false) },
        });
      } else {
        toast.ok(`${tr('Ticket', 'Ticket')} ${r.ticket.number}${change}`);
      }
      onSold();
    } catch (e) {
      toast.error(errorText(e));
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
        if (e.key === 'Escape') {
          e.preventDefault();
          setPaying(false);
        } else if (e.key === 'F1' || e.key === 'F2' || e.key === 'F3') {
          e.preventDefault();
          setMethod(e.key === 'F1' ? 'cash' : e.key === 'F2' ? 'card' : 'bizum');
        } else if (e.key === 'Enter' && !e.repeat) {
          e.preventDefault();
          finish(!e.shiftKey);
        } else if (method === 'cash' && e.target !== givenRef.current && /^[\d.,]$/.test(e.key)) {
          givenRef.current?.focus();
        }
        return;
      }
      if (e.key === 'F12' || (e.ctrlKey && e.key === 'Enter')) {
        e.preventDefault();
        openPay();
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
  const favs = useMemo(() => {
    const f = products.filter((p) => p.favorite);
    return (f.length ? f : products.slice(0, 12)).slice(0, 24).sort((a, b) => a.name.localeCompare(b.name, 'es'));
  }, [products]);

  return (
    <div className="sale" hidden={!active}>
      <section className="sale-main">
        <div className="search">
          <div className="search-box">
            <Search size={20} />
            <input
              ref={searchRef}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onSearchKey}
              placeholder={tr('Busca, escanea o escribe «funda 12,50»', 'Search, scan or type "case 12.50"')}
              spellCheck={false}
              autoComplete="off"
              aria-label={tr('Buscar producto', 'Search product')}
            />
            <kbd>F2</kbd>
          </div>
          {query.trim() && (
            <div className="dropdown" role="listbox">
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
            <div className="empty">
              <div className="empty-icon">
                <ScanBarcode size={30} />
              </div>
              <h3>{tr('Escanea un código o busca un producto', 'Scan a barcode or search for a product')}</h3>
              <ul className="hints">
                <li>
                  <kbd>Enter</kbd> {tr('añade el resultado marcado', 'adds the highlighted result')}
                </li>
                <li>
                  <code>funda 12,50</code> {tr('vende un artículo libre a ese precio', 'sells a custom item at that price')}
                </li>
                <li>
                  <code>3*cable</code> {tr('añade 3 unidades', 'adds 3 units')}
                </li>
                <li>
                  <kbd>F12</kbd> {tr('cobrar', 'charge')} · <kbd>Ctrl</kbd>+<kbd>Z</kbd> {tr('deshacer', 'undo')}
                </li>
              </ul>
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
                  <div className="cart-name">
                    <span>{l.name}</span>
                    {(!l.productId || l.vatBp !== settings.defaultVatBp) && (
                      <small>
                        {!l.productId && tr('Artículo libre', 'Custom item')}
                        {!l.productId && l.vatBp !== settings.defaultVatBp && ' · '}
                        {l.vatBp !== settings.defaultVatBp && `IVA ${rate(l.vatBp)}`}
                      </small>
                    )}
                  </div>
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
            </>
          )}
        </div>

        {favs.length > 0 && (
          <div className="favs-wrap">
            <div className="section-label">{tr('Rápidos', 'Quick picks')}</div>
            <div className="favs">
              {favs.map((p) => (
                <button key={p.id} className="fav" onMouseDown={keep} onClick={() => addProduct(p)}>
                  <span className="fav-name">{p.name}</span>
                  <span className="fav-price">{p.priceCents ? eur(p.priceCents) : tr('Precio libre', 'Open price')}</span>
                </button>
              ))}
            </div>
          </div>
        )}

        {paying && (
          <div className="pay-back" onMouseDown={() => setPaying(false)}>
            <div className="pay" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label={tr('Cobrar', 'Charge')}>
              <div className="pay-head">
                <h2>{tr('Cobrar', 'Charge')}</h2>
                <button className="icon-btn" onMouseDown={keep} onClick={() => setPaying(false)} aria-label={tr('Cerrar', 'Close')}>
                  <X size={18} />
                </button>
              </div>
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
                  <div className={`change${short ? ' short' : ''}`}>
                    <span>{short ? tr('Falta', 'Still due') : tr('Cambio', 'Change')}</span>
                    <strong>{eur(short ? total - (tendered ?? 0) : preview?.payment?.changeCents ?? 0)}</strong>
                  </div>
                </div>
              ) : (
                <p className="pay-note">
                  {method === 'card'
                    ? tr('Cobra en el datáfono y confirma aquí.', 'Take the payment on the card terminal, then confirm here.')
                    : tr('Comprueba que el Bizum ha llegado y confirma.', 'Check that the Bizum payment has arrived, then confirm.')}
                </p>
              )}
              <div className="pay-actions">
                <button className="btn btn-primary btn-xl" disabled={busy || short} onMouseDown={keep} onClick={() => finish(true)}>
                  <Printer size={20} />
                  {busy ? tr('Imprimiendo…', 'Printing…') : tr('Imprimir ticket', 'Print ticket')}
                  <kbd className="on-dark">Enter</kbd>
                </button>
                <button className="btn btn-ghost" disabled={busy || short} onMouseDown={keep} onClick={() => finish(false)}>
                  {tr('Guardar sin imprimir', 'Save without printing')}
                  <kbd>Shift+Enter</kbd>
                </button>
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
              <button className="link" onMouseDown={keep} onClick={() => reprint(last.id, true)}>
                <Printer size={14} />
                {tr('Copia', 'Copy')}
              </button>
            </div>
          )}
          <div className="checkout-total">
            <span>
              {tr('Total', 'Total')}
              {count > 0 && ` · ${count} ${count === 1 ? tr('unidad', 'unit') : tr('unidades', 'units')}`}
            </span>
            <strong>{eur(total)}</strong>
          </div>
          <button className="btn-charge" disabled={!lines.length || busy || paying} onMouseDown={keep} onClick={openPay}>
            <span>{tr('Cobrar', 'Charge')}</span>
            <kbd>F12</kbd>
          </button>
        </div>
      </aside>
    </div>
  );
}
