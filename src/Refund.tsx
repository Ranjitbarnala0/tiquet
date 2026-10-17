import { useEffect, useMemo, useRef, useState } from 'react';
import { Minus, Plus, Printer, Undo2 } from 'lucide-react';
import { ApiError, call, toError } from './api';
import { ClockModal } from './Clock';
import { eur, fmt } from './format';
import { errorText, isPrinterSetup, isTransient, tr, warningText } from './i18n';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { Finalized, Logo, Method, RefundPreview, Settings, TicketRecord } from './types';
import { Field, Modal, Segmented } from './ui';

interface Props {
  ticket: TicketRecord;
  settings: Settings;
  logo: Logo | null;
  onClose: () => void;
  onDone: (refund: TicketRecord) => void;
  onSetupPrinter?: () => void;
}

// A return is its own ticket (a rectifying simplified invoice) that points at
// the sale; the original is never edited. The core refuses anything beyond the
// units still on the ticket, so this screen only has to make that easy to see.
export function RefundModal({ ticket, settings, logo, onClose, onDone, onSetupPrinter }: Props) {
  const toast = useToast();
  const [qty, setQty] = useState<Record<number, number>>({});
  const [method, setMethod] = useState<Method>(ticket.payment.method);
  const [reason, setReason] = useState('');
  const [preview, setPreview] = useState<RefundPreview | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [clock, setClock] = useState<{ last: string; print: boolean } | null>(null);
  const seq = useRef(0);
  const noPrinter = !settings.printerName.trim();

  const left = (id: number) => {
    const l = ticket.lines.find((x) => x.id === id);
    return l ? l.qty - (l.refunded ?? 0) : 0;
  };
  const picked = useMemo(
    () => ticket.lines.flatMap((l) => (l.id !== undefined && qty[l.id] ? [{ lineId: l.id, qty: qty[l.id] }] : [])),
    [ticket.lines, qty],
  );
  const amount = ticket.lines.reduce((a, l) => a + (l.id !== undefined ? (qty[l.id] ?? 0) * l.unitCents : 0), 0);
  const anyLeft = ticket.lines.some((l) => l.id !== undefined && left(l.id) > 0);
  const args = useMemo(() => ({ id: ticket.id, lines: picked, method, reason: reason.trim() }), [ticket.id, picked, method, reason]);

  useEffect(() => {
    const n = ++seq.current;
    if (!args.lines.length) {
      setPreview(null);
      setProblem(null);
      return;
    }
    const t = setTimeout(() => {
      call<RefundPreview>('tickets.refund', { ...args, preview: true })
        .then((p) => {
          if (n !== seq.current) return;
          setPreview(p);
          setProblem(null);
        })
        .catch((e) => {
          if (n !== seq.current) return;
          setPreview(null);
          setProblem(errorText(e));
        });
    }, 120);
    return () => clearTimeout(t);
  }, [args]);

  const set = (id: number, n: number) => setQty((q) => ({ ...q, [id]: Math.max(0, Math.min(left(id), n)) }));
  const all = () => setQty(Object.fromEntries(ticket.lines.flatMap((l) => (l.id !== undefined ? [[l.id, left(l.id)]] : []))));

  const confirm = async (print: boolean, clockOk = false) => {
    if (busy || !picked.length || amount <= 0) return;
    setBusy(true);
    try {
      const r = await call<Finalized>('tickets.refund', { ...args, print: print && !noPrinter, clockOk });
      const done = `${tr('Devolución', 'Return')} ${r.ticket.number} · ${eur(Math.abs(r.ticket.totalCents))}`;
      if (r.printError) {
        const err = toError(r.printError);
        const text = `${done} ${tr('guardada, pero no se imprimió', 'saved, but not printed')}: ${errorText(err)}`;
        if (isTransient(err)) {
          const retry = () =>
            call<{ warning: string | null }>('tickets.print', { id: r.ticket.id, copy: false })
              .then((w) => (w.warning ? toast.info(warningText(w.warning)) : toast.ok(tr('Enviado a la impresora', 'Sent to the printer'))))
              .catch((e) => toast.error(errorText(e)));
          toast.error(text, { sticky: true, action: { label: tr('Reintentar', 'Retry'), run: retry } });
        } else if (isPrinterSetup(err) && onSetupPrinter) {
          toast.error(text, { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } });
        } else {
          toast.error(text);
        }
      } else {
        toast.ok(done);
        if (r.printWarning) toast.info(warningText(r.printWarning));
      }
      onDone(r.ticket);
    } catch (e) {
      if (e instanceof ApiError && e.code === 'E_CLOCK') setClock({ last: e.detail, print });
      else toast.error(errorText(e));
      setBusy(false);
    }
  };

  const ready = picked.length > 0 && amount > 0 && !problem;

  return (
    <Modal
      wide
      title={`${tr('Devolución de', 'Return from')} ${ticket.number}`}
      onClose={onClose}
      actions={
        <>
          <span className="refund-total">
            {tr('A devolver', 'To return')} <b>{eur(Math.max(0, amount))}</b>
          </span>
          <button className="btn btn-ghost" onClick={onClose}>
            {tr('Cancelar', 'Cancel')}
          </button>
          {!noPrinter && (
            <button className="btn" onClick={() => confirm(false)} disabled={!ready || busy}>
              {tr('Sin imprimir', 'Without printing')}
            </button>
          )}
          <button className="btn btn-primary" onClick={() => confirm(true)} disabled={!ready || busy}>
            {noPrinter ? <Undo2 size={16} /> : <Printer size={16} />}
            {noPrinter ? tr('Guardar devolución', 'Save return') : tr('Devolver e imprimir', 'Return and print')}
          </button>
        </>
      }
    >
      <div className="refund">
        <div className="refund-form">
          <div className="refund-head">
            <span className="field-label">{tr('Artículos que se devuelven', 'Items being returned')}</span>
            {anyLeft && (
              <button className="link" onClick={all}>
                {tr('Devolver todo', 'Return everything')}
              </button>
            )}
          </div>
          <div className="refund-lines">
            {ticket.lines.map((l) => {
              if (l.id === undefined) return null;
              const id = l.id;
              const max = left(id);
              const n = qty[id] ?? 0;
              return (
                <div key={id} className={`refund-line${max === 0 ? ' done' : ''}${n > 0 ? ' on' : ''}`}>
                  <div className="refund-name">
                    <b>{l.name}</b>
                    <small>
                      {l.qty} × {fmt(l.unitCents)}
                      {(l.refunded ?? 0) > 0 && ` · ${tr('ya devueltas', 'already returned')}: ${l.refunded}`}
                    </small>
                  </div>
                  {max === 0 ? (
                    <span className="badge">{tr('Devuelto', 'Returned')}</span>
                  ) : (
                    <div className="stepper">
                      <button onClick={() => set(id, n - 1)} disabled={n === 0} aria-label={tr('Una menos', 'One less')}>
                        <Minus size={15} />
                      </button>
                      <input
                        className="qty-input"
                        inputMode="numeric"
                        value={n}
                        aria-label={`${tr('Unidades a devolver de', 'Units to return of')} ${l.name}`}
                        onFocus={(e) => e.target.select()}
                        onChange={(e) => set(id, Number(e.target.value.replace(/\D/g, '') || 0))}
                      />
                      <button onClick={() => set(id, n + 1)} disabled={n >= max} aria-label={tr('Una más', 'One more')}>
                        <Plus size={15} />
                      </button>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
          <Field label={tr('Cómo se devuelve el dinero', 'How the money goes back')}>
            <Segmented
              value={method}
              onChange={setMethod}
              options={[
                { value: 'cash', label: tr('Efectivo', 'Cash') },
                { value: 'card', label: tr('Tarjeta', 'Card') },
                { value: 'bizum', label: 'Bizum' },
              ]}
            />
          </Field>
          <Field label={tr('Motivo (sale en el ticket)', 'Reason (printed on the ticket)')}>
            <input
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              maxLength={200}
              placeholder={tr('Defectuoso, no le sirve…', 'Faulty, does not fit…')}
            />
          </Field>
          {problem && <p className="field-error">{problem}</p>}
        </div>
        <div className="refund-paper">
          {preview ? (
            <Paper lines={preview.lines} logo={logo} cols={settings.columns} />
          ) : (
            <p className="muted refund-hint">{tr('Elige qué se devuelve para ver el ticket.', 'Pick what is returned to see the ticket.')}</p>
          )}
        </div>
      </div>
      {clock && (
        <ClockModal
          last={clock.last}
          onCancel={() => setClock(null)}
          onConfirm={() => {
            const p = clock.print;
            setClock(null);
            confirm(p, true);
          }}
        />
      )}
    </Modal>
  );
}
