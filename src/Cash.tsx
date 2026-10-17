import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { ArrowDownLeft, ArrowUpRight, Lock, Printer, Unlock, Wallet } from 'lucide-react';
import { call, toError } from './api';
import { dateTime, eur, fmt, parseMoney } from './format';
import { errorText, tr, warningText } from './i18n';
import { useToast } from './toast';
import type { CashDrawer, CashSession, User } from './types';
import { Empty, Field, Segmented } from './ui';

function Money({ value, onChange, autoFocus, placeholder }: { value: string; onChange: (v: string) => void; autoFocus?: boolean; placeholder?: string }) {
  return (
    <div className="money-input">
      <input value={value} onChange={(e) => onChange(e.target.value)} inputMode="decimal" autoFocus={autoFocus} placeholder={placeholder ?? '0,00'} />
      <span>€</span>
    </div>
  );
}

const MAX = 999_999_999;

export function Cash({ user }: { user: User }) {
  const toast = useToast();
  const [d, setD] = useState<CashDrawer | null>(null);
  const [history, setHistory] = useState<CashSession[] | null>(null);
  const [float, setFloat] = useState('');
  const [dir, setDir] = useState<'out' | 'in'>('out');
  const [amount, setAmount] = useState('');
  const [reason, setReason] = useState('');
  const [counted, setCounted] = useState('');
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const admin = user.role === 'admin';

  const load = useCallback(() => {
    call<CashDrawer>('cash.state')
      .then(setD)
      .catch((e) => toast.error(errorText(e)));
    if (admin) {
      call<CashSession[]>('cash.history')
        .then(setHistory)
        .catch((e) => toast.error(errorText(e)));
    }
  }, [admin, toast]);

  useEffect(() => {
    load();
    const t = setInterval(load, 20_000);
    return () => clearInterval(t);
  }, [load]);

  const run = async (cmd: string, args: object, done: string) => {
    if (busy) return false;
    setBusy(true);
    try {
      const r = await call<CashDrawer>(cmd, args);
      setD(r);
      toast.ok(done);
      return true;
    } catch (e) {
      toast.error(errorText(e));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const floatCents = parseMoney(float || '0', MAX);
  const open = async (e: FormEvent) => {
    e.preventDefault();
    if (floatCents === null || floatCents < 0) return;
    if (await run('cash.open', { floatCents }, tr('Caja abierta', 'Till opened'))) setFloat('');
  };

  const moveCents = parseMoney(amount, MAX);
  const move = async (e: FormEvent) => {
    e.preventDefault();
    if (!moveCents || moveCents <= 0 || !reason.trim()) return;
    const cents = dir === 'out' ? -moveCents : moveCents;
    if (await run('cash.move', { cents, reason }, dir === 'out' ? tr('Salida anotada', 'Cash out recorded') : tr('Entrada anotada', 'Cash in recorded'))) {
      setAmount('');
      setReason('');
    }
  };

  const countedCents = parseMoney(counted, MAX);
  const diff = d?.session && countedCents !== null ? countedCents - d.expectedCents : null;
  const close = async (print: boolean) => {
    if (busy || countedCents === null || countedCents < 0) return;
    setBusy(true);
    try {
      const r = await call<{ session: CashSession; printError: string | null }>('cash.close', { countedCents, note, print });
      if (r.printError) toast.error(`${tr('Caja cerrada, pero el cierre no se imprimió', 'Till closed, but the report did not print')}: ${errorText(toError(r.printError))}`, { sticky: true });
      else toast.ok(tr('Caja cerrada', 'Till closed'));
      setCounted('');
      setNote('');
      load();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const reprint = async (id: number) => {
    try {
      const r = await call<{ warning: string | null }>('cash.print', { id });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Cierre impreso', 'Report printed'));
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const s = d?.session;
  return (
    <div className="page narrow">
      <div className="page-head">
        <div>
          <h1>{tr('Caja', 'Cash drawer')}</h1>
          <p className="muted">
            {s
              ? `${tr('Abierta el', 'Opened on')} ${dateTime(s.openedAt)} ${tr('por', 'by')} ${s.openedBy}`
              : tr('Abre la caja con el fondo inicial al empezar el día y ciérrala contando el efectivo.', 'Open the till with the starting float and close it by counting the cash.')}
          </p>
        </div>
      </div>

      {d && !s && (
        <form className="card cash-open" onSubmit={open}>
          <Unlock size={22} />
          <div className="grow">
            <Field label={tr('Fondo inicial (cambio en el cajón)', 'Starting float (change in the drawer)')}>
              <Money value={float} onChange={setFloat} autoFocus />
            </Field>
          </div>
          <button className="btn btn-primary" disabled={busy || floatCents === null || floatCents < 0}>
            {tr('Abrir caja', 'Open till')}
          </button>
        </form>
      )}

      {s && d && (
        <>
          <div className="stats">
            <div className="stat hero">
              <span>{tr('Debería haber en el cajón', 'Should be in the drawer')}</span>
              <b>{eur(d.expectedCents)}</b>
            </div>
            <div className="stat">
              <span>{tr('Fondo inicial', 'Starting float')}</span>
              <b>{eur(s.floatCents)}</b>
            </div>
            <div className="stat">
              <span>{tr('Ventas en efectivo', 'Cash sales')}</span>
              <b>{eur(d.cashCents)}</b>
            </div>
            <div className="stat">
              <span>{tr('Entradas y salidas', 'Cash in and out')}</span>
              <b>{eur(d.movesCents)}</b>
            </div>
          </div>

          <div className="card">
            <div className="card-head">
              <h2>{tr('Entradas y salidas de efectivo', 'Cash in and out')}</h2>
            </div>
            <form className="cash-move" onSubmit={move}>
              <Segmented
                value={dir}
                onChange={setDir}
                options={[
                  { value: 'out', label: tr('Salida', 'Out') },
                  { value: 'in', label: tr('Entrada', 'In') },
                ]}
              />
              <Money value={amount} onChange={setAmount} />
              <input value={reason} onChange={(e) => setReason(e.target.value)} maxLength={120} placeholder={dir === 'out' ? tr('Pago a proveedor, compra…', 'Supplier payment, purchase…') : tr('Cambio traído, …', 'Change brought in, …')} />
              <button className="btn" disabled={busy || !moveCents || moveCents <= 0 || !reason.trim()}>
                {tr('Anotar', 'Record')}
              </button>
            </form>
            {d.moves.length > 0 && (
              <table className="table compact">
                <tbody>
                  {d.moves.map((m, i) => (
                    <tr key={i}>
                      <td className="muted">{dateTime(m.at)}</td>
                      <td>
                        {m.cents < 0 ? <ArrowUpRight size={14} className="move-out" /> : <ArrowDownLeft size={14} className="move-in" />} {m.reason}
                      </td>
                      <td className="muted">{m.byName}</td>
                      <td className="num strong">{fmt(m.cents)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="card">
            <div className="card-head">
              <h2>{tr('Cerrar caja', 'Close the till')}</h2>
            </div>
            <div className="cash-close">
              <Field label={tr('Efectivo contado en el cajón', 'Cash counted in the drawer')}>
                <Money value={counted} onChange={setCounted} />
              </Field>
              <div className={`cash-diff${diff === null ? '' : diff === 0 ? ' ok' : ' off'}`}>
                {diff === null
                  ? tr('Cuenta el efectivo y escríbelo', 'Count the cash and type it in')
                  : diff === 0
                    ? tr('Cuadra', 'It balances')
                    : `${diff > 0 ? tr('Sobran', 'Over by') : tr('Faltan', 'Short by')} ${eur(Math.abs(diff))}`}
              </div>
              <Field label={tr('Nota (opcional)', 'Note (optional)')}>
                <input value={note} onChange={(e) => setNote(e.target.value)} maxLength={200} />
              </Field>
              <div className="row-gap">
                <button className="btn btn-primary" onClick={() => close(true)} disabled={busy || countedCents === null}>
                  <Lock size={16} />
                  {tr('Cerrar e imprimir el cierre', 'Close and print the report')}
                </button>
                <button className="btn btn-ghost" onClick={() => close(false)} disabled={busy || countedCents === null}>
                  {tr('Cerrar sin imprimir', 'Close without printing')}
                </button>
              </div>
            </div>
          </div>
        </>
      )}

      {admin && history && (
        <div className="card">
          <div className="card-head">
            <h2>{tr('Cierres anteriores', 'Previous closes')}</h2>
          </div>
          {history.filter((h) => h.closedAt).length === 0 ? (
            <Empty icon={<Wallet size={26} />} title={tr('Todavía no hay cierres', 'No closes yet')} />
          ) : (
            <table className="table compact">
              <thead>
                <tr>
                  <th>{tr('Cerrada', 'Closed')}</th>
                  <th>{tr('Por', 'By')}</th>
                  <th className="num">{tr('Debería', 'Expected')}</th>
                  <th className="num">{tr('Contado', 'Counted')}</th>
                  <th className="num">{tr('Diferencia', 'Difference')}</th>
                  <th />
                </tr>
              </thead>
              <tbody>
                {history
                  .filter((h) => h.closedAt)
                  .map((h) => {
                    const off = (h.countedCents ?? 0) - (h.expectedCents ?? 0);
                    return (
                      <tr key={h.id}>
                        <td>{dateTime(h.closedAt!)}</td>
                        <td>{h.closedBy}</td>
                        <td className="num">{fmt(h.expectedCents ?? 0)}</td>
                        <td className="num">{fmt(h.countedCents ?? 0)}</td>
                        <td className={`num strong${off === 0 ? '' : ' warn-text'}`}>{fmt(off)}</td>
                        <td className="num">
                          <button className="icon-btn" onClick={() => reprint(h.id)} title={tr('Imprimir el cierre', 'Print the report')} aria-label={tr('Imprimir el cierre', 'Print the report')}>
                            <Printer size={16} />
                          </button>
                        </td>
                      </tr>
                    );
                  })}
              </tbody>
            </table>
          )}
        </div>
      )}
    </div>
  );
}
