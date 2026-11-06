import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Banknote,
  ChevronLeft,
  ChevronRight,
  CreditCard,
  Download,
  Pencil,
  Printer,
  ReceiptText,
  Repeat2,
  RotateCcw,
  Search,
  Send,
  ShoppingCart,
  Smartphone,
  SmartphoneCharging,
  Trash2,
  Undo2,
} from 'lucide-react';
import { call } from './api';
import { Cash } from './Cash';
import { addDays, dateTime, eur, isoDate, shortDate } from './format';
import { errorText, getLang, isPrinterSetup, isTransient, printText, tr, warningText } from './i18n';
import { Paper } from './Paper';
import { machineName, payOptions, payValue, type PayOption } from './machines';
import { ExpenseModal, RechargeModal, type EntryDraft } from './Quick';
import { RefundModal } from './Refund';
import { Reports } from './Reports';
import { ShareDialog } from './Share';
import { useToast } from './toast';
import type { DayGone, DayList, DayRow, Logo, Method, PLine, Settings, TicketRecord, User } from './types';
import { Empty, Field, Modal, Segmented } from './ui';
import { aeatText } from './Verifactu';

export type DayView = 'moves' | 'cash' | 'reports';

type Preset = 'day' | 'week' | 'month' | 'year';
type Filter = 'all' | 'sales' | 'recharges' | 'expenses' | 'unprinted' | 'gone';
type DayPart = 'all' | 'sales' | 'recharges' | 'expenses';
type Detail = { ticket: TicketRecord; lines: PLine[]; logo: Logo | null };

export function methodName(m: string): string {
  return m === 'cash' ? tr('Efectivo', 'Cash') : m === 'card' ? tr('Tarjeta', 'Card') : m === 'bizum' ? 'Bizum' : tr('Transferencia', 'Transfer');
}

export function MethodIcon({ m }: { m: Method }) {
  const Icon = m === 'cash' ? Banknote : m === 'card' ? CreditCard : Smartphone;
  return <Icon size={16} className="method-icon" aria-label={methodName(m)} />;
}

// how a line was paid, with its machine where that tells something
function payLabel(s: Settings, r: DayRow): string {
  const show = r.machine && (r.source === 'recharge' || r.method !== 'card' || s.machines.length > 1);
  if (!show) return methodName(r.method);
  return r.method === 'card' && r.source !== 'recharge' ? machineName(s, r.machine) : `${methodName(r.method)} · ${machineName(s, r.machine)}`;
}

const rowKey = (r: DayRow) => `${r.source}-${r.id}`;
const isTicket = (r: DayRow) => r.source === 'ticket';
const unprinted = (r: DayRow) => r.printed === 'none' || r.printed === 'failed';

// "1 venta", "3 ventas"
const many = (n: number, one: string, more: string) => `${n} ${n === 1 ? one : more}`;

function longDate(iso: string): string {
  const d = new Date(`${iso}T12:00:00`);
  return d.toLocaleDateString(getLang() === 'es' ? 'es-ES' : 'en-GB', { weekday: 'long', day: 'numeric', month: 'long' });
}

function presetRange(p: Preset): { from: string; to: string } {
  const now = new Date();
  const today = isoDate(now);
  switch (p) {
    case 'week':
      return { from: isoDate(addDays(now, -6)), to: today };
    case 'month':
      return { from: isoDate(new Date(now.getFullYear(), now.getMonth(), 1)), to: today };
    case 'year':
      return { from: `${now.getFullYear()}-01-01`, to: today };
    default:
      return { from: today, to: today };
  }
}

export function Day(props: {
  user: User;
  settings: Settings;
  logo: Logo | null;
  request: { view: DayView; n: number };
  onSetupPrinter?: () => void;
  // the same items in a new sale; with `correct`, one that replaces this ticket
  onRepeat: (t: TicketRecord, correct: boolean) => void;
}) {
  const [view, setView] = useState<DayView>(props.request.view);
  useEffect(() => setView(props.request.view), [props.request]);
  const admin = props.user.role === 'admin';
  const views: { id: DayView; label: string }[] = [
    { id: 'moves', label: tr('Ventas y gastos', 'Sales and expenses') },
    { id: 'cash', label: tr('Caja', 'Till') },
  ];
  if (admin) views.push({ id: 'reports', label: tr('Informes', 'Reports') });

  return (
    <div className="day">
      <div className="day-views" role="tablist">
        {views.map((v) => (
          <button key={v.id} role="tab" aria-selected={view === v.id} className={view === v.id ? 'on' : ''} onClick={() => setView(v.id)}>
            {v.label}
          </button>
        ))}
      </div>
      <div className="day-body">
        {view === 'moves' && <Moves {...props} />}
        {view === 'cash' && <Cash user={props.user} settings={props.settings} />}
        {view === 'reports' && admin && <Reports settings={props.settings} />}
      </div>
    </div>
  );
}

function Moves({ user, settings, logo, onSetupPrinter, onRepeat }: { user: User; settings: Settings; logo: Logo | null; onSetupPrinter?: () => void; onRepeat: (t: TicketRecord, correct: boolean) => void }) {
  const toast = useToast();
  const admin = user.role === 'admin';
  const today = isoDate(new Date());
  const [preset, setPreset] = useState<Preset>('day');
  const [range, setRange] = useState(() => presetRange('day'));
  const [filter, setFilter] = useState<Filter>('all');
  // only what went through this machine; 0 is cards from before there were any
  const [onMachine, setOnMachine] = useState<number | null>(null);
  const [q, setQ] = useState('');
  const [data, setData] = useState<DayList | null>(null);
  const [sel, setSel] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);
  const [removing, setRemoving] = useState<DayRow | null>(null);
  const [editing, setEditing] = useState<DayRow | null>(null);
  const [reason, setReason] = useState('');
  const [wholeDay, setWholeDay] = useState(false);
  const [word, setWord] = useState('');
  const [what, setWhat] = useState<DayPart>('all');
  const [refunding, setRefunding] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [busy, setBusy] = useState(false);
  const gone = filter === 'gone';

  const load = useCallback(() => {
    call<DayList>('day.list', { ...range, deleted: gone })
      .then(setData)
      .catch((e) => toast.error(errorText(e)));
  }, [range, gone, toast]);

  useEffect(() => {
    load();
    const t = setInterval(load, 30_000);
    return () => clearInterval(t);
  }, [load]);

  const rows = data?.rows ?? [];
  const alive = useMemo(() => rows.filter((r) => !r.gone), [rows]);
  const counts = useMemo(
    () => ({
      all: alive.length,
      sales: alive.filter((r) => isTicket(r) || r.source === 'sale').length,
      recharges: alive.filter((r) => r.source === 'recharge').length,
      expenses: alive.filter((r) => r.source === 'expense' || r.costCents !== 0).length,
      unprinted: alive.filter((r) => isTicket(r) && unprinted(r)).length,
    }),
    [alive],
  );

  const shown = useMemo(() => {
    const needle = q.trim().toLowerCase();
    return rows.filter((r) => {
      const fits = {
        all: !r.gone,
        sales: !r.gone && (isTicket(r) || r.source === 'sale'),
        recharges: !r.gone && r.source === 'recharge',
        expenses: !r.gone && (r.source === 'expense' || r.costCents !== 0),
        unprinted: !r.gone && isTicket(r) && unprinted(r),
        gone: r.gone,
      }[filter];
      if (!fits) return false;
      if (onMachine !== null && (r.source === 'expense' || (r.machine ?? (r.method === 'card' ? 0 : null)) !== onMachine)) return false;
      if (!needle) return true;
      return `${r.concept} ${r.number ?? ''} ${r.supplier} ${r.byName} ${eur(Math.abs(r.cents))}`.toLowerCase().includes(needle);
    });
  }, [rows, filter, q, onMachine]);

  const row = shown.find((r) => rowKey(r) === sel) ?? null;

  useEffect(() => {
    setSel((cur) => (cur !== null && shown.some((r) => rowKey(r) === cur) ? cur : shown[0] ? rowKey(shown[0]) : null));
  }, [shown]);

  const ticketId = row && isTicket(row) ? row.id : null;
  useEffect(() => {
    if (ticketId === null) {
      setDetail(null);
      return;
    }
    // a slow answer for a ticket the cashier already left must not replace
    // the one now highlighted
    let on = true;
    call<Detail>('tickets.get', { id: ticketId })
      .then((d) => on && setDetail(d))
      .catch((e) => on && toast.error(errorText(e)));
    return () => {
      on = false;
    };
  }, [ticketId, toast]);

  const oneDay = range.from === range.to;
  const step = (n: number) => {
    const next = isoDate(addDays(new Date(`${range.from}T12:00:00`), n));
    if (next > today) return;
    setPreset('day');
    setRange({ from: next, to: next });
  };
  const pickPreset = (p: Preset) => {
    setPreset(p);
    setRange(presetRange(p));
  };

  const own = (r: DayRow) => !!data?.staffDelete && r.byName === user.name && r.at.slice(0, 10) === today;
  const mayFix = (r: DayRow) => admin || own(r);
  const mayDelete = (r: DayRow) => (admin ? !isTicket(r) || r.at.slice(0, 10) === today || !!data?.anyDay : own(r));

  // charged the wrong way: one tap on the same ticket, and one tap back
  const setPayment = (id: number, method: Method, machine: number | null) =>
    call<Detail>('tickets.payment', { id, method, machine: machine ?? undefined }).then((d) => {
      setDetail((cur) => (cur?.ticket.id === id ? d : cur));
      load();
    });

  const changePay = async (t: TicketRecord, o: PayOption) => {
    const was = t.payment;
    if (busy || payValue(was) === o.value) return;
    setBusy(true);
    try {
      await setPayment(t.id, o.method, o.machine);
      toast.ok(`${t.number}: ${o.label}`, {
        action: { label: tr('Deshacer', 'Undo'), run: () => setPayment(t.id, was.method, was.machine).catch((e) => toast.error(errorText(e))) },
      });
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const saveEdit = async (e: EntryDraft) => {
    if (!editing) return false;
    try {
      await call('book.edit', { id: editing.id, ...e });
      toast.ok(tr('Corregido', 'Corrected'));
      load();
      return true;
    } catch (err) {
      toast.error(errorText(err));
      return false;
    }
  };

  const reprint = async () => {
    if (!detail || busy) return;
    const id = detail.ticket.id;
    setBusy(true);
    try {
      const r = await call<{ warning: string | null }>('tickets.print', { id });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Impreso', 'Printed'));
      const d = await call<Detail>('tickets.get', { id });
      setDetail((cur) => (cur?.ticket.id === id ? d : cur));
      load();
    } catch (e) {
      if (isTransient(e)) toast.error(errorText(e), { action: { label: tr('Reintentar', 'Retry'), run: reprint } });
      else if (isPrinterSetup(e) && onSetupPrinter) toast.error(errorText(e), { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } });
      else toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const printReceipt = (r: DayRow) =>
    call<{ warning: string | null }>('book.print', { id: r.id })
      .then((p) => (p.warning ? toast.info(warningText(p.warning)) : toast.ok(tr('Impreso', 'Printed'))))
      .catch((e) => toast.error(errorText(e)));

  const removeEntry = (r: DayRow) =>
    call('book.delete', { id: r.id })
      .then(() => {
        load();
        toast.ok(`${tr('Borrado', 'Deleted')}: ${r.concept} · ${eur(Math.abs(r.cents))}`, {
          action: { label: tr('Deshacer', 'Undo'), run: () => restore(r) },
        });
      })
      .catch((e) => toast.error(errorText(e)));

  const restore = (r: DayRow) =>
    call('book.restore', { id: r.id })
      .then(load)
      .catch((e) => toast.error(errorText(e)));

  // a typed entry goes at once with an undo; a sale leaves a voided ticket
  // that cannot come back, so it is asked first
  const remove = (r: DayRow) => {
    if (!isTicket(r)) {
      removeEntry(r);
      return;
    }
    setReason('');
    setRemoving(r);
  };

  const removeTicket = async () => {
    if (!removing || busy) return;
    setBusy(true);
    try {
      await call('tickets.void', { id: removing.id, reason });
      toast.ok(`${removing.number} ${tr('borrado', 'deleted')}`);
      setRemoving(null);
      load();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const removeDay = async () => {
    if (busy || word.trim().toUpperCase() !== 'BORRAR') return;
    setBusy(true);
    try {
      const r = await call<DayGone>('day.delete', { date: range.from, reason, what });
      const kept = r.kept > 0 ? ` · ${r.kept} ${tr('con devolución de otro día siguen', 'with a return on another day stay')}` : '';
      toast.ok(`${tr('Día borrado', 'Day deleted')}: ${many(r.tickets, tr('venta', 'sale'), tr('ventas', 'sales'))}, ${many(r.entries, tr('apunte', 'entry'), tr('apuntes', 'entries'))}${kept}`, { sticky: r.kept > 0 });
      setWholeDay(false);
      setWord('');
      setReason('');
      load();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const exportCsv = async () => {
    try {
      const r = await call<{ path: string }>('book.export', range);
      toast.ok(`${tr('Exportado a', 'Exported to')} ${r.path}`, {
        sticky: true,
        action: { label: tr('Abrir carpeta', 'Open folder'), run: () => call('app.reveal', { what: 'exports', name: r.path.split(/[\\/]/).pop() }).catch(() => {}) },
      });
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const printSummary = async () => {
    try {
      const r = await call<{ warning: string | null }>('reports.print', range);
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Resumen enviado a la impresora', 'Summary sent to the printer'));
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const s = data?.summary;
  // once there have been several, what was done on each still tells something
  const several = settings.machines.length > 1;
  const title = oneDay ? (range.from === today ? tr('Hoy', 'Today') : range.from === isoDate(addDays(new Date(), -1)) ? tr('Ayer', 'Yesterday') : shortDate(range.from)) : `${shortDate(range.from)} – ${shortDate(range.to)}`;

  const t = detail && ticketId === detail.ticket.id ? detail.ticket : undefined;
  const fixes = t ? payOptions({ ...settings, payBizum: true }, t.payment) : [];
  const returnable = t ? t.kind === 'sale' && !t.voidedAt && t.lines.some((l) => l.qty > (l.refunded ?? 0)) : false;
  // a sale of today is corrected in a new one that replaces it; a repair or a
  // return already made is tied to this ticket
  const correctable = t ? t.kind === 'sale' && !t.voidedAt && t.issuedAt.slice(0, 10) === today && t.lines.every((l) => !l.repairId) && !t.refunds.some((r) => !r.voided) : false;
  const onPaper = t ? t.prints.some((p) => p.status !== 'failed') : false;
  const lastPrint = t?.prints[t.prints.length - 1];
  const printedTimes = t ? t.prints.filter((p) => p.status === 'printed' || p.status === 'sent').length : 0;

  const refunded = (r: TicketRecord) => {
    setRefunding(false);
    load();
    setSel(`ticket-${r.id}`);
  };

  const chips: { id: Filter; label: string; n?: number }[] = [
    { id: 'all', label: tr('Todo', 'All'), n: counts.all },
    { id: 'sales', label: tr('Ventas', 'Sales'), n: counts.sales },
    ...(settings.showRecharge || counts.recharges > 0 ? [{ id: 'recharges' as const, label: tr('Recargas', 'Recharges'), n: counts.recharges }] : []),
    { id: 'expenses', label: tr('Gastos', 'Expenses'), n: counts.expenses },
    { id: 'unprinted', label: tr('Sin imprimir', 'Not printed'), n: counts.unprinted },
  ];
  if (admin) chips.push({ id: 'gone', label: tr('Borrados', 'Deleted') });

  return (
    <div className="moves">
      <div className="moves-top">
        <div className="date-nav">
          {admin && (
            <button className="icon-btn" onClick={() => step(-1)} aria-label={tr('Día anterior', 'Previous day')}>
              <ChevronLeft size={20} />
            </button>
          )}
          <div className="date-label">
            <h1>{title}</h1>
            {oneDay && <small>{longDate(range.from)}</small>}
          </div>
          {admin && (
            <button className="icon-btn" onClick={() => step(1)} disabled={range.to >= today} aria-label={tr('Día siguiente', 'Next day')}>
              <ChevronRight size={20} />
            </button>
          )}
        </div>
        {admin && (
          <div className="chips">
            {(
              [
                ['day', tr('Día', 'Day')],
                ['week', tr('7 días', '7 days')],
                ['month', tr('Este mes', 'This month')],
                ['year', tr('Este año', 'This year')],
              ] as [Preset, string][]
            ).map(([id, label]) => (
              <button key={id} className={`chip${preset === id ? ' on' : ''}`} onClick={() => pickPreset(id)}>
                {label}
              </button>
            ))}
            <input
              type="date"
              className="date-pick"
              value={oneDay ? range.from : ''}
              max={today}
              aria-label={tr('Elegir día', 'Pick a day')}
              onChange={(e) => {
                if (!e.target.value) return;
                setPreset('day');
                setRange({ from: e.target.value, to: e.target.value });
              }}
            />
          </div>
        )}
        {admin && (
          <div className="page-actions">
            <button className="btn" onClick={printSummary} title={tr('Resumen en la impresora de tickets', 'Summary on the receipt printer')}>
              <Printer size={16} />
              {tr('Resumen', 'Summary')}
            </button>
            <button className="btn" onClick={exportCsv} title={tr('Para el gestor: Excel con IVA separado', 'For the accountant: Excel with VAT split out')}>
              <Download size={16} />
              {tr('Exportar', 'Export')}
            </button>
            {oneDay && (
              <button
                className="btn btn-danger-ghost"
                onClick={() => {
                  setReason('');
                  setWord('');
                  setWhat('all');
                  setWholeDay(true);
                }}
                disabled={counts.all === 0}
              >
                <Trash2 size={16} />
                {tr('Borrar el día', 'Delete the day')}
              </button>
            )}
          </div>
        )}
      </div>

      {s && (
        <div className="day-stats">
          <div className="day-stat">
            <span>
              <i className="dot dot-cash" />
              {tr('Efectivo', 'Cash')}
            </span>
            <b>{eur(s.cashCents)}</b>
          </div>
          <div className="day-stat">
            <span>
              <i className="dot dot-card" />
              {tr('Tarjeta', 'Card')}
            </span>
            <b>{eur(s.cardCents)}</b>
          </div>
          {s.bizumCents !== 0 && (
            <div className="day-stat">
              <span>
                <i className="dot dot-bizum" />
                Bizum
              </span>
              <b>{eur(s.bizumCents)}</b>
            </div>
          )}
          {s.otherCents !== 0 && (
            <div className="day-stat">
              <span>{tr('Otros', 'Other')}</span>
              <b>{eur(s.otherCents)}</b>
            </div>
          )}
          {(settings.showRecharge || s.rechargeCount > 0) && (
            <div className="day-stat">
              <span>
                <i className="dot dot-recharge" />
                {tr('Recargas', 'Recharges')}
              </span>
              <b>{eur(s.rechargeCents)}</b>
              {s.rechargeCount > 0 && <small>{s.rechargeCount}</small>}
            </div>
          )}
          <div className="day-stat hero">
            <span>{tr('Total', 'Total')}</span>
            <b>{eur(s.totalCents)}</b>
            <small>
              {many(s.saleCount, tr('venta', 'sale'), tr('ventas', 'sales'))}
            </small>
          </div>
          <div className="day-stat">
            <span>
              <i className="dot dot-expense" />
              {tr('Gastos', 'Expenses')}
            </span>
            <b className="neg">{eur(-s.expenseCents)}</b>
            {s.rechargeCostCents + s.machineCostCents !== 0 && (
              <small>
                {[
                  s.purchaseCents !== 0 && `${eur(s.purchaseCents)} ${tr('compras', 'purchases')}`,
                  s.rechargeCostCents !== 0 && `${eur(s.rechargeCostCents)} ${tr('recargas', 'recharges')}`,
                  s.machineCostCents !== 0 && `${eur(s.machineCostCents)} ${tr('máquinas', 'machines')}`,
                ]
                  .filter(Boolean)
                  .join(' + ')}
              </small>
            )}
          </div>
          <div className="day-stat hero">
            <span>{tr('Queda', 'Left')}</span>
            <b className={s.netCents < 0 ? 'neg' : ''}>{eur(s.netCents)}</b>
          </div>
        </div>
      )}

      {s && several && s.machines.length > 0 && (
        <div className="day-machines">
          <span>{tr('Por máquina', 'By machine')}</span>
          {s.machines.map((m) => (
            <button key={m.id} className={`chip${onMachine === m.id ? ' on' : ''}`} onClick={() => setOnMachine(onMachine === m.id ? null : m.id)}>
              {machineName(settings, m.id)}
              <b>{eur(m.salesCents + m.rechargeCents)}</b>
              <small>{m.count}</small>
            </button>
          ))}
        </div>
      )}

      <div className="moves-split">
        <section className="moves-list">
          <div className="moves-filters">
            <div className="chips">
              {chips.map((c) => (
                <button key={c.id} className={`chip${filter === c.id ? ' on' : ''}`} onClick={() => setFilter(c.id)}>
                  {c.label}
                  {c.n !== undefined && c.n > 0 && <small>{c.n}</small>}
                </button>
              ))}
            </div>
            <div className="search-sm">
              <Search size={16} />
              <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={tr('Buscar', 'Search')} aria-label={tr('Buscar', 'Search')} />
            </div>
          </div>
          {data?.capped && <p className="cap-note">{tr('Se muestran los 1.500 más recientes; los totales cuentan todos.', 'Showing the 1,500 most recent; the totals count them all.')}</p>}
          <div className="list">
            {shown.map((r) => {
              const asCost = filter === 'expenses' && r.source !== 'expense';
              const Icon = r.source === 'expense' ? ShoppingCart : r.source === 'recharge' ? SmartphoneCharging : ReceiptText;
              return (
                <button key={rowKey(r)} className={`move${sel === rowKey(r) ? ' on' : ''}${r.gone ? ' gone' : ''}`} onClick={() => setSel(rowKey(r))}>
                  <span className="move-time">{oneDay ? r.at.slice(11, 16) : `${r.at.slice(8, 10)}/${r.at.slice(5, 7)}`}</span>
                  <span className={`move-icon ${r.source}`}>
                    <Icon size={16} />
                  </span>
                  <span className="move-main">
                    <b>{r.concept}</b>
                    <small>
                      {payLabel(settings, r)}
                      {r.number ? ` · ${r.number}` : ''} · {r.byName}
                    </small>
                  </span>
                  {isTicket(r) && !r.gone && (r.printed === 'printed' ? <span className="badge blue">{tr('Impreso', 'Printed')}</span> : r.printed === 'unknown' ? <span className="badge amber">{tr('¿Impreso?', 'Printed?')}</span> : <span className="badge">{tr('Sin imprimir', 'Not printed')}</span>)}
                  {r.invoice && <span className="badge blue">{tr('Factura', 'Invoice')}</span>}
                  {r.refund && <span className="badge amber">{tr('Devolución', 'Return')}</span>}
                  {r.source === 'sale' && <span className="badge">{tr('Sin ticket', 'No ticket')}</span>}
                  <b className={`move-cents${r.cents < 0 || asCost ? ' neg' : ''}`}>{eur(asCost ? -r.costCents : r.cents)}</b>
                </button>
              );
            })}
            {data && shown.length === 0 && (
              <Empty
                icon={<ReceiptText size={28} />}
                title={gone ? tr('Nada borrado', 'Nothing deleted') : tr('Nada por aquí', 'Nothing here')}
                text={q ? tr('Prueba con otra búsqueda.', 'Try another search.') : gone ? undefined : tr('Las ventas, recargas y gastos aparecen aquí.', 'Sales, recharges and expenses show up here.')}
              />
            )}
          </div>
        </section>

        <section className="detail-pane">
          {row && !isTicket(row) && (
            <div className="entry">
              <h2>{row.concept}</h2>
              <p className={`entry-cents${row.cents < 0 ? ' neg' : ''}`}>{eur(row.cents)}</p>
              <dl>
                <dt>{tr('Fecha', 'Date')}</dt>
                <dd>{dateTime(row.at)}</dd>
                <dt>{tr('Pago', 'Paid with')}</dt>
                <dd>{payLabel(settings, row)}</dd>
                <dt>{tr('Por', 'By')}</dt>
                <dd>{row.byName}</dd>
                {row.source === 'recharge' && (
                  <>
                    <dt>{tr('Cuenta como gasto', 'Counts as expense')}</dt>
                    <dd>{row.costCents ? eur(row.costCents) : tr('No', 'No')}</dd>
                  </>
                )}
                {row.supplier && (
                  <>
                    <dt>{row.source === 'recharge' ? tr('Operador', 'Operator') : tr('Proveedor', 'Supplier')}</dt>
                    <dd>{row.supplier}</dd>
                  </>
                )}
                {row.invoiceRef && (
                  <>
                    <dt>{tr('Factura', 'Invoice')}</dt>
                    <dd>{row.invoiceRef}</dd>
                  </>
                )}
                {row.gone && (
                  <>
                    <dt>{tr('Borrado', 'Deleted')}</dt>
                    <dd>
                      {row.deletedBy}
                      {row.deletedAt ? ` · ${dateTime(row.deletedAt)}` : ''}
                      {row.deleteReason ? ` · ${row.deleteReason}` : ''}
                    </dd>
                  </>
                )}
              </dl>
              <div className="row-gap">
                {row.source === 'recharge' && !row.gone && (
                  <button className="btn" onClick={() => printReceipt(row)}>
                    <Printer size={16} />
                    {tr('Imprimir justificante', 'Print a receipt')}
                  </button>
                )}
                {!row.gone && row.source !== 'sale' && mayFix(row) && (
                  <button className="btn" onClick={() => setEditing(row)}>
                    <Pencil size={16} />
                    {tr('Editar', 'Edit')}
                  </button>
                )}
                {row.gone
                  ? mayDelete(row) &&
                    row.deleteReason !== 'Corregido' && (
                      <button className="btn" onClick={() => restore(row)}>
                        <RotateCcw size={16} />
                        {tr('Recuperar', 'Restore')}
                      </button>
                    )
                  : mayDelete(row) && (
                      <button className="btn btn-danger-ghost" onClick={() => remove(row)}>
                        <Trash2 size={16} />
                        {tr('Borrar', 'Delete')}
                      </button>
                    )}
              </div>
            </div>
          )}

          {row && isTicket(row) && t && detail && (
            <>
              <div className="detail-head">
                <div>
                  <h2 className="mono">
                    {t.number}
                    {t.customer && <span className="badge blue">{tr('Factura', 'Invoice')}</span>}
                    {t.kind === 'refund' && <span className="badge amber">{tr('Devolución', 'Return')}</span>}
                    {t.training && <span className="badge">{tr('Prueba', 'Practice')}</span>}
                  </h2>
                  {t.customer && (
                    <p className="ref-note">
                      {t.customer.name} · {t.customer.nif}
                    </p>
                  )}
                  <p className="muted">
                    {dateTime(t.issuedAt)} · {t.userName} · {t.payment.machine && several ? machineName(settings, t.payment.machine) : methodName(t.payment.method)} · {eur(t.totalCents)}
                  </p>
                  {t.refundOf && (
                    <p className="ref-note">
                      {tr('Devuelve artículos de', 'Returns items from')} <span className="mono">{t.refundOf.number}</span>
                      {t.refundReason ? ` · ${t.refundReason}` : ''}
                    </p>
                  )}
                  {t.refunds.length > 0 && (
                    <p className="ref-note">
                      {tr('Devoluciones:', 'Returns:')}{' '}
                      {t.refunds.map((r, i) => (
                        <span key={r.id} className={r.voided ? 'struck' : ''}>
                          {i > 0 && ', '}
                          <span className="mono">{r.number}</span> ({eur(r.totalCents)})
                        </span>
                      ))}
                    </p>
                  )}
                  {lastPrint ? (
                    <p className={`print-note${lastPrint.status === 'failed' ? ' bad' : ''}`}>
                      {lastPrint.status === 'printed' || lastPrint.status === 'sent'
                        ? `${lastPrint.copy ? tr('Copia impresa el', 'Copy printed on') : tr('Impreso el', 'Printed on')} ${dateTime(lastPrint.at)}`
                        : `${dateTime(lastPrint.at)} · ${printText(lastPrint.detail?.replace(/^E_PRINT:/, '') ?? '')}`}
                      {printedTimes > 1 && ` · ${tr('impreso', 'printed')} ${printedTimes} ${tr('veces', 'times')}`}
                    </p>
                  ) : (
                    <p className="print-note">{tr('No se ha impreso', 'Not printed')}</p>
                  )}
                  {t.paymentChanges.map((c) => (
                    <p key={c.at + c.to} className="print-note">
                      {tr('Cambiado de', 'Changed from')} {c.fromMachine && c.from === 'card' && several ? machineName(settings, c.fromMachine) : methodName(c.from)} {tr('a', 'to')}{' '}
                      {c.toMachine && c.to === 'card' && several ? machineName(settings, c.toMachine) : methodName(c.to)} · {c.byName} · {dateTime(c.at)}
                    </p>
                  ))}
                  {t.aeat && <p className={`print-note${t.aeat.state === 'Incorrecto' ? ' bad' : ''}`}>{aeatText(t.aeat)}</p>}
                  {t.voidedAt && (
                    <p className="void-note">
                      {tr('Borrado el', 'Deleted on')} {dateTime(t.voidedAt)} {tr('por', 'by')} {t.voidedBy}
                      {t.voidReason ? `: ${t.voidReason}` : ''}
                    </p>
                  )}
                </div>
                {!t.voidedAt && mayFix(row) && (
                  <div className="pay-fix">
                    <span>{tr('Pagado con', 'Paid with')}</span>
                    <Segmented
                      value={payValue(t.payment)}
                      onChange={(v) => {
                        const o = fixes.find((x) => x.value === v);
                        if (o) changePay(t, o);
                      }}
                      options={fixes}
                    />
                  </div>
                )}
                <div className="actions">
                  {correctable && mayFix(row) && (
                    <button className="btn" onClick={() => onRepeat(t, true)} title={tr('Cambiar artículos, cantidades o precios', 'Change items, quantities or prices')}>
                      <Pencil size={16} />
                      {tr('Editar', 'Edit')}
                    </button>
                  )}
                  <button className="btn" onClick={reprint} disabled={busy}>
                    <Printer size={16} />
                    {onPaper ? tr('Copia', 'Copy') : tr('Imprimir', 'Print')}
                  </button>
                  <button className="btn" onClick={() => setSharing(true)} title={tr('WhatsApp, email o PDF', 'WhatsApp, email or PDF')}>
                    <Send size={16} />
                    {tr('Enviar', 'Send')}
                  </button>
                  {t.kind === 'sale' && t.lines.some((l) => !l.repairId) && (
                    <button className="btn" onClick={() => onRepeat(t, false)} title={tr('Una venta nueva con los mismos artículos', 'A new sale with the same items')}>
                      <Repeat2 size={16} />
                      {tr('Repetir', 'Repeat')}
                    </button>
                  )}
                  {(admin || settings.staffRefund) && returnable && (
                    <button className="btn" onClick={() => setRefunding(true)}>
                      <Undo2 size={16} />
                      {tr('Devolución', 'Return')}
                    </button>
                  )}
                  {!row.gone && mayDelete(row) && (
                    <button className="btn btn-danger-ghost" onClick={() => remove(row)}>
                      <Trash2 size={16} />
                      {tr('Borrar', 'Delete')}
                    </button>
                  )}
                </div>
              </div>
              <div className="detail-paper">
                <Paper lines={detail.lines} logo={detail.logo} cols={settings.columns} stamp={t.voidedAt ? tr('BORRADO', 'DELETED') : undefined} />
              </div>
            </>
          )}

          {!row && <Empty icon={<ReceiptText size={28} />} title={tr('Elige una línea de la lista', 'Pick a line from the list')} />}
        </section>
      </div>

      {sharing && t && <ShareDialog ticketId={t.id} number={t.number} onClose={() => setSharing(false)} />}

      {editing?.source === 'recharge' && <RechargeModal settings={settings} cents={editing.cents} entry={editing} onSave={saveEdit} onClose={() => setEditing(null)} />}
      {editing?.source === 'expense' && <ExpenseModal initial="" entry={editing} settings={settings} onSave={saveEdit} onClose={() => setEditing(null)} />}

      {removing && (
        <Modal
          title={`${tr('Borrar la venta', 'Delete the sale')} ${removing.number}`}
          onClose={() => setRemoving(null)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setRemoving(null)}>
                {tr('Cancelar', 'Cancel')}
              </button>
              <button className="btn btn-danger" autoFocus onClick={removeTicket} disabled={busy}>
                <Trash2 size={16} />
                {tr('Borrar', 'Delete')} · {eur(removing.cents)}
              </button>
            </>
          }
        >
          <p className="muted">
            {tr(
              'Sale de todas las listas y totales. El dueño la sigue viendo en Borrados, con quién la borró. Si el cliente devuelve algo, usa Devolución.',
              'It leaves every list and total. The owner can still see it in Deleted, with who deleted it. If the customer brings something back, use Return.',
            )}
          </p>
          <Field label={tr('Motivo (opcional)', 'Reason (optional)')}>
            <input value={reason} onChange={(e) => setReason(e.target.value)} maxLength={200} placeholder={tr('Cobrado dos veces, importe equivocado…', 'Charged twice, wrong amount…')} />
          </Field>
        </Modal>
      )}

      {wholeDay && s && (
        <Modal
          title={`${tr('Borrar el día', 'Delete the day')}: ${title}`}
          onClose={() => setWholeDay(false)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setWholeDay(false)}>
                {tr('Cancelar', 'Cancel')}
              </button>
              <button className="btn btn-danger" onClick={removeDay} disabled={busy || word.trim().toUpperCase() !== 'BORRAR'}>
                <Trash2 size={16} />
                {what === 'all' ? tr('Borrar el día', 'Delete the day') : tr('Borrar', 'Delete')}
              </button>
            </>
          }
        >
          <Segmented
            value={what}
            onChange={setWhat}
            options={[
              { value: 'all', label: tr('Todo', 'All') },
              { value: 'sales', label: tr('Ventas', 'Sales') },
              { value: 'recharges', label: tr('Recargas', 'Recharges') },
              { value: 'expenses', label: tr('Gastos', 'Expenses') },
            ]}
          />
          <div className="whole-day">
            <b>{eur(what === 'all' ? s.totalCents : what === 'sales' ? s.totalCents - s.rechargeCents : what === 'recharges' ? s.rechargeCents : s.purchaseCents)}</b>
            <span>
              {what !== 'all'
                ? what === 'sales'
                  ? many(s.saleCount + s.refundCount, tr('venta', 'sale'), tr('ventas', 'sales'))
                  : what === 'recharges'
                    ? many(s.rechargeCount, tr('recarga', 'recharge'), tr('recargas', 'recharges'))
                    : many(s.purchaseCount, tr('gasto', 'expense'), tr('gastos', 'expenses'))
                : `${many(s.saleCount + s.refundCount, tr('venta', 'sale'), tr('ventas', 'sales'))} · ${many(s.rechargeCount, tr('recarga', 'recharge'), tr('recargas', 'recharges'))} · ${many(s.purchaseCount, tr('gasto', 'expense'), tr('gastos', 'expenses'))}`}
            </span>
          </div>
          <p className="muted">
            {tr(
              'Sale de las listas, los totales y la caja. Las ventas borradas no se pueden recuperar. El dueño lo sigue viendo todo en Borrados, con quién lo borró y cuándo.',
              'It leaves the lists, the totals and the till. Deleted sales cannot be brought back. The owner can still see it all in Deleted, with who deleted it and when.',
            )}
          </p>
          <Field label={tr('Motivo (opcional)', 'Reason (optional)')}>
            <input value={reason} onChange={(e) => setReason(e.target.value)} maxLength={200} />
          </Field>
          <Field label={tr('Escribe BORRAR para confirmar', 'Type BORRAR to confirm')}>
            <input
              autoFocus
              value={word}
              onChange={(e) => setWord(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  removeDay();
                }
              }}
            />
          </Field>
        </Modal>
      )}

      {refunding && t && <RefundModal ticket={t} settings={settings} logo={logo} onClose={() => setRefunding(false)} onDone={refunded} onSetupPrinter={onSetupPrinter} />}
    </div>
  );
}
