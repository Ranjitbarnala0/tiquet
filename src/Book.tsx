import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { BookOpen, Download, RotateCcw, Trash2 } from 'lucide-react';
import { call } from './api';
import { addDays, dateTime, eur, fmt, isoDate, parseMoney, rate, VAT_RATES } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { Settings, User } from './types';
import { Empty, Field, Modal, Segmented } from './ui';

export type BookMethod = 'cash' | 'card' | 'bizum' | 'transfer';

export interface BookRow {
  source: 'sale' | 'expense' | 'ticket';
  id: number;
  at: string;
  concept: string;
  cents: number;
  method: BookMethod;
  byName: string;
  category: string;
  number: string | null;
  gone: boolean;
  deletedBy: string | null;
  deletedAt: string | null;
  deleteReason: string | null;
}

interface BookSummary {
  ticketCents: number;
  ticketCount: number;
  saleCents: number;
  saleCount: number;
  expenseCents: number;
  expenseCount: number;
  resultCents: number;
  cashCents: number;
  byCategory: [string, number][];
}

interface BookList {
  rows: BookRow[];
  summary: BookSummary;
  from: string;
  to: string;
  staffDelete: boolean;
}

interface Quick {
  concept: string;
  cents: number;
  uses: number;
}

const CATEGORIES = ['Compras de género', 'Alquiler', 'Luz y agua', 'Teléfono e internet', 'Sueldos', 'Impuestos y tasas', 'Transporte', 'Reparaciones y servicios', 'Material', 'Otros'];

// an amount this big is checked twice: a typo of 1000 for 10,00 is easy
const BIG = 50_000;

export function methodLabel(m: BookMethod): string {
  return m === 'cash' ? tr('Efectivo', 'Cash') : m === 'card' ? tr('Tarjeta', 'Card') : m === 'bizum' ? 'Bizum' : tr('Transferencia', 'Transfer');
}

// "funda 12,50" is a sale, "-luz 45" an expense, "10" a sale of 10 €. The
// amount is the last word that reads as money; the rest is what it was.
export function parseLine(text: string): { kind: 'sale' | 'expense' | null; concept: string; cents: number } | null {
  let t = text.trim();
  let kind: 'sale' | 'expense' | null = null;
  if (t.startsWith('-')) {
    kind = 'expense';
    t = t.slice(1).trim();
  } else if (t.startsWith('+')) {
    kind = 'sale';
    t = t.slice(1).trim();
  }
  const words = t.split(/\s+/).filter(Boolean);
  for (let i = words.length - 1; i >= 0; i--) {
    const cents = parseMoney(words[i]);
    if (cents !== null && cents > 0) {
      return { kind, concept: [...words.slice(0, i), ...words.slice(i + 1)].join(' '), cents };
    }
  }
  return null;
}

const newId = () => (typeof crypto !== 'undefined' && 'randomUUID' in crypto ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`);

// One line to type a sale without a ticket or an expense. Enter saves it; an
// undo comes with every entry instead of an "are you sure?".
export function QuickEntry({ settings, onAdded, compact, autoFocus }: { settings: Settings; onAdded?: () => void; compact?: boolean; autoFocus?: boolean }) {
  const toast = useToast();
  const [kind, setKind] = useState<'sale' | 'expense'>('sale');
  const [text, setText] = useState('');
  const [method, setMethod] = useState<BookMethod>('cash');
  const [more, setMore] = useState(false);
  const [category, setCategory] = useState('Otros');
  const [supplier, setSupplier] = useState('');
  const [supplierNif, setSupplierNif] = useState('');
  const [invoiceRef, setInvoiceRef] = useState('');
  const [vatBp, setVatBp] = useState(settings.defaultVatBp);
  const [date, setDate] = useState('');
  const [confirmBig, setConfirmBig] = useState(false);
  const [quick, setQuick] = useState<Quick[]>([]);
  const last = useRef({ key: '', at: 0 });
  const busy = useRef(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const loadQuick = useCallback(() => {
    call<Quick[]>('book.quick')
      .then(setQuick)
      .catch(() => {});
  }, []);
  useEffect(loadQuick, [loadQuick]);

  const parsed = parseLine(text);
  const effKind = parsed?.kind ?? kind;

  const save = async (entry: { kind: 'sale' | 'expense'; concept: string; cents: number }) => {
    // a second Enter in a hurry, or a double tap, saves it once
    const key = `${entry.kind}|${entry.concept}|${entry.cents}|${method}`;
    if (busy.current || (last.current.key === key && Date.now() - last.current.at < 2000)) return;
    busy.current = true;
    const expense = entry.kind === 'expense';
    try {
      const r = await call<{ entry: { id: number; concept: string; cents: number } }>('book.add', {
        ...entry,
        method,
        clientId: newId(),
        vatBp: expense && more ? vatBp : undefined,
        category: expense ? category : '',
        supplier: expense && more ? supplier : '',
        supplierNif: expense && more ? supplierNif : '',
        invoiceRef: expense && more ? invoiceRef : '',
        date: more ? date : '',
      });
      last.current = { key, at: Date.now() };
      setText('');
      setConfirmBig(false);
      setSupplier('');
      setSupplierNif('');
      setInvoiceRef('');
      const id = r.entry.id;
      toast.ok(`${expense ? tr('Gasto apuntado', 'Expense saved') : tr('Venta apuntada', 'Sale saved')}: ${r.entry.concept} · ${eur(r.entry.cents)}`, {
        action: {
          label: tr('Deshacer', 'Undo'),
          run: () =>
            call('book.delete', { id, reason: tr('Deshecho al momento', 'Undone right away') })
              .then(() => onAdded?.())
              .catch((e) => toast.error(errorText(e))),
        },
      });
      onAdded?.();
      if (!expense) loadQuick();
      inputRef.current?.focus();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      busy.current = false;
    }
  };

  const submit = (e?: FormEvent) => {
    e?.preventDefault();
    if (!parsed) {
      if (text.trim()) toast.info(tr('Falta el importe: escribe por ejemplo «funda 10» o «-luz 45».', 'The amount is missing: type e.g. "case 10" or "-power 45".'));
      return;
    }
    if (parsed.cents >= BIG && !confirmBig) {
      setConfirmBig(true);
      return;
    }
    save({ kind: effKind, concept: parsed.concept, cents: parsed.cents });
  };

  const tap = (q: Quick) => save({ kind: 'sale', concept: q.concept, cents: q.cents });

  return (
    <div className={`quick-entry${compact ? ' compact' : ''}`}>
      <form onSubmit={submit} className="quick-row">
        <Segmented
          value={effKind}
          onChange={(v) => {
            setKind(v);
            if (parsed?.kind) setText(text.trim().slice(1).trim());
          }}
          options={[
            { value: 'sale', label: tr('Venta sin ticket', 'Sale, no ticket') },
            { value: 'expense', label: tr('Gasto', 'Expense') },
          ]}
        />
        <input
          ref={inputRef}
          className={`grow quick-input${effKind === 'expense' ? ' expense' : ''}`}
          value={text}
          autoFocus={autoFocus}
          onChange={(e) => {
            setText(e.target.value);
            setConfirmBig(false);
          }}
          placeholder={effKind === 'expense' ? tr('luz 45  ·  género 120,50', 'power 45  ·  stock 120.50') : tr('funda 10  ·  recarga 15  ·  -luz 45 es un gasto', 'case 10  ·  top-up 15  ·  -power 45 is an expense')}
          aria-label={tr('Concepto e importe', 'What and how much')}
        />
        <Segmented
          value={method}
          onChange={setMethod}
          options={[
            { value: 'cash', label: tr('Efectivo', 'Cash') },
            { value: 'card', label: tr('Tarjeta', 'Card') },
            { value: 'bizum', label: 'Bizum' },
          ]}
        />
        <button className={`btn ${effKind === 'expense' ? 'btn-danger' : 'btn-primary'}`} disabled={!parsed}>
          {parsed ? `${effKind === 'expense' ? '−' : '+'} ${eur(parsed.cents)}` : tr('Apuntar', 'Save')}
        </button>
      </form>
      {confirmBig && parsed && (
        <p className="form-error">
          {tr('Importe alto', 'Large amount')}: {eur(parsed.cents)}. {tr('Pulsa Enter otra vez para apuntarlo.', 'Press Enter again to save it.')}
        </p>
      )}
      {effKind === 'sale' && quick.length > 0 && (
        <div className="picks">
          {quick.map((q) => (
            <button key={q.concept} type="button" className="pick" onClick={() => tap(q)} title={tr('Apuntar con un toque', 'Save with one tap')}>
              {q.concept} · {fmt(q.cents)}
            </button>
          ))}
        </div>
      )}
      {!compact && (
        <button type="button" className="link more-link" onClick={() => setMore(!more)}>
          {more ? tr('Menos detalles', 'Fewer details') : effKind === 'expense' ? tr('Más detalles: categoría, proveedor, factura, IVA, fecha', 'More: category, supplier, invoice, VAT, date') : tr('Más detalles: fecha', 'More: date')}
        </button>
      )}
      {effKind === 'expense' && !more && (
        <div className="picks">
          {CATEGORIES.map((c) => (
            <button key={c} type="button" className={`pick${category === c ? ' on' : ''}`} onClick={() => setCategory(c)}>
              {c}
            </button>
          ))}
        </div>
      )}
      {more && (
        <div className="grid3 quick-more">
          {effKind === 'expense' && (
            <>
              <Field label={tr('Categoría', 'Category')}>
                <select value={category} onChange={(e) => setCategory(e.target.value)}>
                  {CATEGORIES.map((c) => (
                    <option key={c}>{c}</option>
                  ))}
                </select>
              </Field>
              <Field label={tr('Proveedor', 'Supplier')}>
                <input value={supplier} onChange={(e) => setSupplier(e.target.value)} maxLength={120} />
              </Field>
              <Field label={tr('NIF del proveedor', 'Supplier NIF')}>
                <input value={supplierNif} onChange={(e) => setSupplierNif(e.target.value.toUpperCase())} maxLength={20} />
              </Field>
              <Field label={tr('Nº de factura', 'Invoice no.')}>
                <input value={invoiceRef} onChange={(e) => setInvoiceRef(e.target.value)} maxLength={40} />
              </Field>
              <Field label={tr('IVA incluido', 'VAT included')}>
                <select value={vatBp} onChange={(e) => setVatBp(Number(e.target.value))}>
                  {VAT_RATES.map((v) => (
                    <option key={v} value={v}>
                      {rate(v)}
                    </option>
                  ))}
                </select>
              </Field>
            </>
          )}
          <Field label={tr('Fecha', 'Date')} hint={tr('Vacío: ahora', 'Empty: now')}>
            <input type="date" value={date} max={isoDate(new Date())} onChange={(e) => setDate(e.target.value)} />
          </Field>
        </div>
      )}
    </div>
  );
}

// F8 in Venta: a sale without a ticket, without leaving the till
export function QuickSaleModal({ settings, onClose }: { settings: Settings; onClose: () => void }) {
  return (
    <Modal title={tr('Venta sin ticket o gasto', 'Sale without a ticket, or expense')} onClose={onClose} wide>
      <QuickEntry settings={settings} compact autoFocus onAdded={() => {}} />
      <p className="field-hint">{tr('Se apunta en el Libro. Esc para volver a la venta.', 'It goes into the Book. Esc to go back to the sale.')}</p>
    </Modal>
  );
}

type RangeKey = 'today' | 'yesterday' | 'week' | 'month' | 'quarter' | 'year';

function rangeOf(k: RangeKey): { from: string; to: string } {
  const now = new Date();
  const today = isoDate(now);
  switch (k) {
    case 'yesterday': {
      const y = isoDate(addDays(now, -1));
      return { from: y, to: y };
    }
    case 'week':
      return { from: isoDate(addDays(now, -6)), to: today };
    case 'month':
      return { from: isoDate(new Date(now.getFullYear(), now.getMonth(), 1)), to: today };
    case 'quarter':
      return { from: isoDate(new Date(now.getFullYear(), Math.floor(now.getMonth() / 3) * 3, 1)), to: today };
    case 'year':
      return { from: `${now.getFullYear()}-01-01`, to: today };
    default:
      return { from: today, to: today };
  }
}

export function Book({ user, settings }: { user: User; settings: Settings }) {
  const toast = useToast();
  const admin = user.role === 'admin';
  const [range, setRange] = useState<RangeKey>('today');
  const [filter, setFilter] = useState<'all' | 'sale' | 'expense' | 'ticket'>('all');
  const [showDeleted, setShowDeleted] = useState(false);
  const [data, setData] = useState<BookList | null>(null);

  const load = useCallback(() => {
    call<BookList>('book.list', { ...rangeOf(range), deleted: showDeleted })
      .then(setData)
      .catch((e) => toast.error(errorText(e)));
  }, [range, showDeleted, toast]);

  useEffect(() => {
    load();
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
  }, [load]);

  const today = isoDate(new Date());
  const mayDelete = (r: BookRow) => r.source !== 'ticket' && (admin || (!!data?.staffDelete && r.byName === user.name && r.at.slice(0, 10) === today));

  const remove = (r: BookRow) =>
    call('book.delete', { id: r.id })
      .then(() => {
        load();
        toast.ok(`${tr('Borrado', 'Deleted')}: ${r.concept} · ${eur(Math.abs(r.cents))}`, {
          action: { label: tr('Deshacer', 'Undo'), run: () => call('book.restore', { id: r.id }).then(load).catch((e) => toast.error(errorText(e))) },
        });
      })
      .catch((e) => toast.error(errorText(e)));

  const restore = (r: BookRow) =>
    call('book.restore', { id: r.id })
      .then(load)
      .catch((e) => toast.error(errorText(e)));

  const exportCsv = async () => {
    try {
      const r = await call<{ path: string }>('book.export', rangeOf(range));
      toast.ok(`${tr('Libro exportado a', 'Book exported to')} ${r.path}`, {
        sticky: true,
        action: { label: tr('Abrir carpeta', 'Open folder'), run: () => call('app.reveal', { what: 'exports', name: r.path.split(/[\\/]/).pop() }).catch(() => {}) },
      });
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const s = data?.summary;
  const rows = (data?.rows ?? []).filter((r) => filter === 'all' || r.source === filter);
  const oneDay = data ? data.from === data.to : true;

  return (
    <div className="page narrow book">
      <div className="page-head">
        <div>
          <h1>{tr('Libro', 'Book')}</h1>
          <p className="muted">{tr('Todo lo que entra y sale: ventas con ticket, ventas sin ticket y gastos.', 'Everything in and out: ticket sales, sales without a ticket and expenses.')}</p>
        </div>
        <div className="page-actions">
          {admin && (
            <Segmented
              value={range}
              onChange={setRange}
              options={[
                { value: 'today', label: tr('Hoy', 'Today') },
                { value: 'yesterday', label: tr('Ayer', 'Yesterday') },
                { value: 'week', label: tr('7 días', '7 days') },
                { value: 'month', label: tr('Mes', 'Month') },
                { value: 'quarter', label: tr('Trimestre', 'Quarter') },
                { value: 'year', label: tr('Año', 'Year') },
              ]}
            />
          )}
          {admin && (
            <button className="btn" onClick={exportCsv} title={tr('Para el gestor: Excel con IVA separado', 'For the accountant: Excel with VAT split out')}>
              <Download size={16} />
              {tr('Exportar', 'Export')}
            </button>
          )}
        </div>
      </div>

      {s && (
        <div className="stats">
          <div className="stat hero">
            <span>{tr('Resultado', 'Result')}</span>
            <b className={s.resultCents < 0 ? 'neg' : ''}>{eur(s.resultCents)}</b>
            <small>{tr('ventas menos gastos', 'sales less expenses')}</small>
          </div>
          <div className="stat">
            <span>{tr('Ventas con ticket', 'Ticket sales')}</span>
            <b>{eur(s.ticketCents)}</b>
            <small>{s.ticketCount}</small>
          </div>
          <div className="stat">
            <span>{tr('Ventas sin ticket', 'Sales, no ticket')}</span>
            <b>{eur(s.saleCents)}</b>
            <small>{s.saleCount}</small>
          </div>
          <div className="stat">
            <span>{tr('Gastos', 'Expenses')}</span>
            <b className="neg">{eur(-s.expenseCents)}</b>
            <small>{s.expenseCount}</small>
          </div>
          <div className="stat">
            <span>{tr('Efectivo (entra − sale)', 'Cash (in − out)')}</span>
            <b>{eur(s.cashCents)}</b>
          </div>
        </div>
      )}

      <div className="card">
        <QuickEntry settings={settings} onAdded={load} autoFocus />
      </div>

      {admin && s && s.byCategory.length > 0 && (
        <div className="cat-strip">
          {s.byCategory.map(([c, cents]) => (
            <span key={c} className="badge">
              {c}: {eur(cents)}
            </span>
          ))}
        </div>
      )}

      <div className="card">
        <div className="card-head book-head">
          <Segmented
            value={filter}
            onChange={setFilter}
            options={[
              { value: 'all', label: tr('Todo', 'All') },
              { value: 'ticket', label: tr('Con ticket', 'Ticket') },
              { value: 'sale', label: tr('Sin ticket', 'No ticket') },
              { value: 'expense', label: tr('Gastos', 'Expenses') },
            ]}
          />
          {admin && (
            <label className="check">
              <input type="checkbox" checked={showDeleted} onChange={(e) => setShowDeleted(e.target.checked)} />
              {tr('Ver borrados', 'Show deleted')}
            </label>
          )}
        </div>
        {rows.length === 0 ? (
          <Empty icon={<BookOpen size={26} />} title={tr('Nada apuntado', 'Nothing yet')} text={tr('Escribe arriba «funda 10» y pulsa Enter.', 'Type "case 10" above and press Enter.')} />
        ) : (
          <table className="table compact book-table">
            <tbody>
              {rows.map((r) => (
                <tr key={`${r.source}-${r.id}`} className={r.gone ? 'gone' : ''}>
                  <td className="muted nowrap">{oneDay ? r.at.slice(11, 16) : dateTime(r.at)}</td>
                  <td className="grow">
                    <span className="concept">{r.concept}</span>{' '}
                    {r.source === 'ticket' ? (
                      <span className="badge blue">{r.number}</span>
                    ) : r.source === 'sale' ? (
                      <span className="badge">{tr('Sin ticket', 'No ticket')}</span>
                    ) : (
                      <span className="badge red">{r.category || tr('Gasto', 'Expense')}</span>
                    )}
                    {r.gone && (
                      <small className="muted">
                        {' '}
                        {r.source === 'ticket'
                          ? tr('anulado', 'voided')
                          : `${tr('borrado por', 'deleted by')} ${r.deletedBy ?? ''}${r.deletedAt ? ` · ${dateTime(r.deletedAt)}` : ''}${r.deleteReason ? ` · ${r.deleteReason}` : ''}`}
                      </small>
                    )}
                  </td>
                  <td className="muted nowrap">{r.byName}</td>
                  <td className="muted nowrap">{methodLabel(r.method)}</td>
                  <td className={`num strong nowrap${r.cents < 0 ? ' neg' : ''}`}>{eur(r.cents)}</td>
                  <td className="nowrap">
                    {r.gone && r.source !== 'ticket' && mayDelete(r) ? (
                      <button className="icon-btn" onClick={() => restore(r)} title={tr('Recuperar', 'Restore')} aria-label={tr('Recuperar', 'Restore')}>
                        <RotateCcw size={15} />
                      </button>
                    ) : !r.gone && mayDelete(r) ? (
                      <button className="icon-btn" onClick={() => remove(r)} title={tr('Borrar', 'Delete')} aria-label={tr('Borrar', 'Delete')}>
                        <Trash2 size={15} />
                      </button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
