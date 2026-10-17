import { useCallback, useEffect, useMemo, useState } from 'react';
import { Ban, Banknote, CreditCard, Printer, ReceiptText, Repeat2, Search, Smartphone, Undo2 } from 'lucide-react';
import { call } from './api';
import { addDays, dateTime, eur, isoDate } from './format';
import { errorText, isPrinterSetup, isTransient, printText, tr, warningText } from './i18n';
import { Paper } from './Paper';
import { RefundModal } from './Refund';
import { useToast } from './toast';
import type { Logo, Method, PLine, Settings, TicketRecord, TicketSummary, User } from './types';
import { Empty, Field, Modal } from './ui';

type RangeKey = 'today' | 'yesterday' | 'week' | 'month' | 'all';

const RANGES: { id: RangeKey; label: () => string }[] = [
  { id: 'today', label: () => tr('Hoy', 'Today') },
  { id: 'yesterday', label: () => tr('Ayer', 'Yesterday') },
  { id: 'week', label: () => tr('7 días', '7 days') },
  { id: 'month', label: () => tr('30 días', '30 days') },
  { id: 'all', label: () => tr('Todos', 'All') },
];

function rangeOf(k: RangeKey): { from?: string; to?: string } {
  const now = new Date();
  switch (k) {
    case 'today':
      return { from: isoDate(now), to: isoDate(now) };
    case 'yesterday': {
      const y = isoDate(addDays(now, -1));
      return { from: y, to: y };
    }
    case 'week':
      return { from: isoDate(addDays(now, -6)), to: isoDate(now) };
    case 'month':
      return { from: isoDate(addDays(now, -29)), to: isoDate(now) };
    default:
      return {};
  }
}

export function methodName(m: Method): string {
  return m === 'cash' ? tr('Efectivo', 'Cash') : m === 'card' ? tr('Tarjeta', 'Card') : 'Bizum';
}

export function MethodIcon({ m }: { m: Method }) {
  const Icon = m === 'cash' ? Banknote : m === 'card' ? CreditCard : Smartphone;
  return <Icon size={16} className="method-icon" aria-label={methodName(m)} />;
}

export function Tickets({
  user,
  settings,
  logo,
  onSetupPrinter,
  onRepeat,
}: {
  user: User;
  settings: Settings;
  logo: Logo | null;
  onSetupPrinter?: () => void;
  onRepeat: (t: TicketRecord) => void;
}) {
  const toast = useToast();
  const [range, setRange] = useState<RangeKey>('today');
  const [q, setQ] = useState('');
  const [list, setList] = useState<TicketSummary[] | null>(null);
  const [selId, setSelId] = useState<number | null>(null);
  const [detail, setDetail] = useState<{ ticket: TicketRecord; lines: PLine[]; logo: Logo | null } | null>(null);
  const [unprinted, setUnprinted] = useState(false);
  const [voiding, setVoiding] = useState(false);
  const [refunding, setRefunding] = useState(false);
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const admin = user.role === 'admin';

  const load = useCallback(() => {
    call<TicketSummary[]>('tickets.list', { ...rangeOf(range), q: q.trim(), limit: 1000, unprinted, training: settings.training })
      .then((l) => {
        setList(l);
        setSelId((id) => (id !== null && l.some((t) => t.id === id) ? id : l[0]?.id ?? null));
      })
      .catch((e) => toast.error(errorText(e)));
  }, [range, q, unprinted, settings.training, toast]);

  useEffect(() => {
    const t = setTimeout(load, q ? 200 : 0);
    return () => clearTimeout(t);
  }, [load, q]);

  useEffect(() => {
    if (selId === null) {
      setDetail(null);
      return;
    }
    // a slow answer for a ticket the cashier already left must not replace
    // the one now highlighted
    let live = true;
    call<{ ticket: TicketRecord; lines: PLine[]; logo: Logo | null }>('tickets.get', { id: selId })
      .then((d) => live && setDetail(d))
      .catch((e) => live && toast.error(errorText(e)));
    return () => {
      live = false;
    };
  }, [selId, toast]);

  const stats = useMemo(() => {
    const live = (list ?? []).filter((t) => !t.voided);
    return { n: live.length, sum: live.reduce((a, t) => a + t.totalCents, 0) };
  }, [list]);

  const move = (d: number) => {
    if (!list?.length) return;
    const i = list.findIndex((t) => t.id === selId);
    const j = Math.min(list.length - 1, Math.max(0, i + d));
    setSelId(list[j].id);
  };

  const reprint = async () => {
    if (!detail || busy) return;
    const id = detail.ticket.id;
    setBusy(true);
    try {
      const r = await call<{ warning: string | null }>('tickets.print', { id });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Impreso', 'Printed'));
      const d = await call<{ ticket: TicketRecord; lines: PLine[]; logo: Logo | null }>('tickets.get', { id });
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

  const doVoid = async () => {
    if (!detail || busy || !reason.trim()) return;
    setBusy(true);
    try {
      const r = await call<{ ticket: TicketRecord; lines: PLine[]; logo: Logo | null }>('tickets.void', { id: detail.ticket.id, reason });
      setDetail(r);
      setVoiding(false);
      setReason('');
      toast.ok(`${r.ticket.number} ${tr('anulado', 'voided')}`);
      load();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const t = detail && detail.ticket.id === selId ? detail.ticket : undefined;
  const today = t ? t.issuedAt.slice(0, 10) === isoDate(new Date()) : false;
  const returnable = t ? t.kind === 'sale' && !t.voidedAt && t.lines.some((l) => l.qty > (l.refunded ?? 0)) : false;
  const liveRefunds = t ? t.refunds.some((r) => !r.voided) : false;
  const onPaper = t ? t.prints.some((p) => p.status !== 'failed') : false;
  const lastPrint = t?.prints[t.prints.length - 1];
  const printedTimes = t ? t.prints.filter((p) => p.status === 'printed' || p.status === 'sent').length : 0;

  const refunded = (r: TicketRecord) => {
    setRefunding(false);
    load();
    setSelId(r.id);
  };

  return (
    <div className="split">
      <section className="list-pane">
        <div className="pane-head">
          <div className="chips">
            {RANGES.map((r) => (
              <button key={r.id} className={`chip${range === r.id ? ' on' : ''}`} onClick={() => setRange(r.id)}>
                {r.label()}
              </button>
            ))}
            <button className={`chip${unprinted ? ' on' : ''}`} onClick={() => setUnprinted(!unprinted)} aria-pressed={unprinted}>
              {tr('Sin imprimir', 'Not printed')}
            </button>
          </div>
          <div className="search-sm">
            <Search size={16} />
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={tr('Nº de ticket o importe', 'Ticket number or amount')} />
          </div>
          <div className="pane-sub">
            <span>
              {stats.n} {stats.n === 1 ? 'ticket' : 'tickets'}
            </span>
            <b>{eur(stats.sum)}</b>
          </div>
          {list && list.length >= 1000 && (
            <p className="cap-note">{tr('Se muestran los 1.000 más recientes: acota las fechas o busca por número.', 'Showing the 1,000 most recent: narrow the dates or search by number.')}</p>
          )}
        </div>
        <div
          className="list"
          tabIndex={0}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
              e.preventDefault();
              move(e.key === 'ArrowDown' ? 1 : -1);
            }
          }}
        >
          {list?.map((s) => (
            <button key={s.id} className={`ticket-row${selId === s.id ? ' on' : ''}${s.voided ? ' voided' : ''}`} onClick={() => setSelId(s.id)}>
              <div className="ticket-row-main">
                <b className="mono">{s.number}</b>
                <span>
                  {dateTime(s.issuedAt)} · {s.userName}
                </span>
              </div>
              <div className="ticket-row-side">
                {s.invoice && <span className="badge blue">{tr('Factura', 'Invoice')}</span>}
                {s.kind === 'refund' && <span className="badge amber">{tr('Devolución', 'Return')}</span>}
                {s.voided && <span className="badge red">{tr('Anulado', 'Void')}</span>}
                {!s.voided && (s.printed === 'failed' || s.printed === 'none') && <span className="badge">{tr('Sin imprimir', 'Not printed')}</span>}
                {!s.voided && s.printed === 'unknown' && <span className="badge amber">{tr('¿Impreso?', 'Printed?')}</span>}
                <MethodIcon m={s.paymentMethod} />
                <b>{eur(s.totalCents)}</b>
              </div>
            </button>
          ))}
          {list && list.length === 0 && (
            <Empty
              icon={<ReceiptText size={28} />}
              title={tr('No hay tickets', 'No tickets')}
              text={q ? tr('Prueba con otro número o importe.', 'Try another number or amount.') : tr('Los tickets de este periodo aparecerán aquí.', 'Tickets for this period will show up here.')}
            />
          )}
        </div>
      </section>

      <section className="detail-pane">
        {t && detail ? (
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
                  {dateTime(t.issuedAt)} · {t.userName} · {methodName(t.payment.method)} · {eur(t.totalCents)}
                </p>
                {t.refundOf && (
                  <p className="ref-note">
                    {tr('Devuelve artículos de', 'Returns items from')}{' '}
                    <button className="link mono" onClick={() => setSelId(t.refundOf!.id)}>
                      {t.refundOf.number}
                    </button>
                    {t.refundReason ? ` · ${t.refundReason}` : ''}
                  </p>
                )}
                {t.refunds.length > 0 && (
                  <p className="ref-note">
                    {tr('Devoluciones:', 'Returns:')}{' '}
                    {t.refunds.map((r, i) => (
                      <span key={r.id}>
                        {i > 0 && ', '}
                        <button className={`link mono${r.voided ? ' struck' : ''}`} onClick={() => setSelId(r.id)}>
                          {r.number}
                        </button>{' '}
                        ({eur(r.totalCents)})
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
                  <p className="print-note">{tr('No se ha impreso todavía', 'Not printed yet')}</p>
                )}
                {t.voidedAt && (
                  <p className="void-note">
                    {tr('Anulado el', 'Voided on')} {dateTime(t.voidedAt)} {tr('por', 'by')} {t.voidedBy}
                    {t.voidReason ? `: ${t.voidReason}` : ''}
                  </p>
                )}
              </div>
              <div className="actions">
                <button className="btn" onClick={reprint} disabled={busy}>
                  <Printer size={16} />
                  {onPaper ? tr('Reimprimir copia', 'Reprint copy') : tr('Imprimir', 'Print')}
                </button>
                {t.kind === 'sale' && (
                  <button className="btn" onClick={() => onRepeat(t)} title={tr('Una venta nueva con los mismos artículos', 'A new sale with the same items')}>
                    <Repeat2 size={16} />
                    {tr('Repetir', 'Repeat')}
                  </button>
                )}
                {admin && returnable && (
                  <button className="btn" onClick={() => setRefunding(true)}>
                    <Undo2 size={16} />
                    {tr('Devolución', 'Return')}
                  </button>
                )}
                {admin && !t.voidedAt && today && !liveRefunds && (
                  <button className="btn btn-danger-ghost" onClick={() => setVoiding(true)}>
                    <Ban size={16} />
                    {tr('Anular', 'Void')}
                  </button>
                )}
              </div>
            </div>
            <div className="detail-paper">
              <Paper lines={detail.lines} logo={detail.logo} cols={settings.columns} stamp={t.voidedAt ? tr('ANULADO', 'VOID') : undefined} />
            </div>
          </>
        ) : (
          <Empty icon={<ReceiptText size={28} />} title={tr('Elige un ticket', 'Pick a ticket')} />
        )}
      </section>

      {voiding && t && (
        <Modal
          title={`${tr('Anular', 'Void')} ${t.number}`}
          onClose={() => setVoiding(false)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setVoiding(false)}>
                {tr('Cancelar', 'Cancel')}
              </button>
              <button className="btn btn-danger" onClick={doVoid} disabled={busy || !reason.trim()}>
                <Ban size={16} />
                {tr('Anular ticket', 'Void ticket')}
              </button>
            </>
          }
        >
          <p className="muted">
            {tr(
              'Para errores de cobro de hoy. El ticket no se borra: queda guardado como anulado y deja de contar en las ventas. Esto no se puede deshacer. Si el cliente devuelve algo, usa Devolución.',
              'For charging mistakes made today. The ticket is not deleted: it stays on record as void and no longer counts in sales. This cannot be undone. If the customer brings something back, use Return.',
            )}
          </p>
          <Field label={tr('Motivo', 'Reason')}>
            <input
              autoFocus
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              maxLength={200}
              placeholder={tr('Cobrado dos veces, importe equivocado…', 'Charged twice, wrong amount…')}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  doVoid();
                }
              }}
            />
          </Field>
        </Modal>
      )}

      {refunding && t && (
        <RefundModal
          ticket={t}
          settings={settings}
          logo={logo}
          onClose={() => setRefunding(false)}
          onDone={refunded}
          onSetupPrinter={onSetupPrinter}
        />
      )}
    </div>
  );
}
