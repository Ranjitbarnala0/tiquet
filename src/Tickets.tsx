import { useCallback, useEffect, useMemo, useState } from 'react';
import { Ban, Banknote, CreditCard, Printer, ReceiptText, Search, Smartphone } from 'lucide-react';
import { call } from './api';
import { addDays, dateTime, eur, isoDate } from './format';
import { errorText, tr } from './i18n';
import { Paper } from './Paper';
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

export function Tickets({ user, settings, logo }: { user: User; settings: Settings; logo: Logo | null }) {
  const toast = useToast();
  const [range, setRange] = useState<RangeKey>('today');
  const [q, setQ] = useState('');
  const [list, setList] = useState<TicketSummary[] | null>(null);
  const [selId, setSelId] = useState<number | null>(null);
  const [detail, setDetail] = useState<{ ticket: TicketRecord; lines: PLine[] } | null>(null);
  const [voiding, setVoiding] = useState(false);
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const admin = user.role === 'admin';

  const load = useCallback(() => {
    call<TicketSummary[]>('tickets.list', { ...rangeOf(range), q: q.trim(), limit: 1000 })
      .then((l) => {
        setList(l);
        setSelId((id) => (id !== null && l.some((t) => t.id === id) ? id : l[0]?.id ?? null));
      })
      .catch((e) => toast.error(errorText(e)));
  }, [range, q, toast]);

  useEffect(() => {
    const t = setTimeout(load, q ? 200 : 0);
    return () => clearTimeout(t);
  }, [load, q]);

  useEffect(() => {
    if (selId === null) {
      setDetail(null);
      return;
    }
    call<{ ticket: TicketRecord; lines: PLine[] }>('tickets.get', { id: selId })
      .then(setDetail)
      .catch((e) => toast.error(errorText(e)));
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
    if (!detail) return;
    try {
      await call('tickets.print', { id: detail.ticket.id, copy: true });
      toast.ok(tr('Copia enviada a la impresora', 'Copy sent to the printer'));
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const doVoid = async () => {
    if (!detail || busy) return;
    setBusy(true);
    try {
      const r = await call<{ ticket: TicketRecord; lines: PLine[] }>('tickets.void', { id: detail.ticket.id, reason });
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

  const t = detail?.ticket;

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
                {s.voided && <span className="badge red">{tr('Anulado', 'Void')}</span>}
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
                <h2 className="mono">{t.number}</h2>
                <p className="muted">
                  {dateTime(t.issuedAt)} · {t.userName} · {methodName(t.payment.method)} · {eur(t.totalCents)}
                </p>
                {t.voidedAt && (
                  <p className="void-note">
                    {tr('Anulado el', 'Voided on')} {dateTime(t.voidedAt)} {tr('por', 'by')} {t.voidedBy}
                    {t.voidReason ? `: ${t.voidReason}` : ''}
                  </p>
                )}
              </div>
              <div className="actions">
                <button className="btn" onClick={reprint}>
                  <Printer size={16} />
                  {tr('Reimprimir copia', 'Reprint copy')}
                </button>
                {admin && !t.voidedAt && (
                  <button className="btn btn-danger-ghost" onClick={() => setVoiding(true)}>
                    <Ban size={16} />
                    {tr('Anular', 'Void')}
                  </button>
                )}
              </div>
            </div>
            <div className="detail-paper">
              <Paper lines={detail.lines} logo={logo} cols={settings.columns} stamp={t.voidedAt ? tr('ANULADO', 'VOID') : undefined} />
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
              <button className="btn btn-danger" onClick={doVoid} disabled={busy}>
                <Ban size={16} />
                {tr('Anular ticket', 'Void ticket')}
              </button>
            </>
          }
        >
          <p className="muted">
            {tr(
              'El ticket no se borra: queda guardado como anulado y deja de contar en las ventas. Esto no se puede deshacer.',
              'The ticket is not deleted: it stays on record as void and no longer counts in sales. This cannot be undone.',
            )}
          </p>
          <Field label={tr('Motivo', 'Reason')}>
            <input
              autoFocus
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              maxLength={200}
              placeholder={tr('Devolución, error de cobro…', 'Refund, wrong charge…')}
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
    </div>
  );
}
