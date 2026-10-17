import { useEffect, useMemo, useState } from 'react';
import { ChartColumn, FileSpreadsheet, Printer } from 'lucide-react';
import { call } from './api';
import { addDays, eur, fmt, isoDate, rate, shortDate } from './format';
import { errorText, getLang, tr } from './i18n';
import { MethodIcon, methodName } from './Tickets';
import { useToast } from './toast';
import type { Summary } from './types';
import { Empty } from './ui';

type Preset = 'today' | 'yesterday' | 'week' | 'month' | 'lastMonth' | 'custom';

const PRESETS: { id: Preset; label: () => string }[] = [
  { id: 'today', label: () => tr('Hoy', 'Today') },
  { id: 'yesterday', label: () => tr('Ayer', 'Yesterday') },
  { id: 'week', label: () => tr('7 días', '7 days') },
  { id: 'month', label: () => tr('Este mes', 'This month') },
  { id: 'lastMonth', label: () => tr('Mes pasado', 'Last month') },
  { id: 'custom', label: () => tr('Fechas', 'Dates') },
];

function presetRange(p: Preset): { from: string; to: string } | null {
  const now = new Date();
  const today = isoDate(now);
  switch (p) {
    case 'today':
      return { from: today, to: today };
    case 'yesterday': {
      const y = isoDate(addDays(now, -1));
      return { from: y, to: y };
    }
    case 'week':
      return { from: isoDate(addDays(now, -6)), to: today };
    case 'month':
      return { from: isoDate(new Date(now.getFullYear(), now.getMonth(), 1)), to: today };
    case 'lastMonth':
      return {
        from: isoDate(new Date(now.getFullYear(), now.getMonth() - 1, 1)),
        to: isoDate(new Date(now.getFullYear(), now.getMonth(), 0)),
      };
    default:
      return null;
  }
}

// axis top that lands on a round number with 4 gridlines
function niceMax(v: number): number {
  if (v <= 0) return 1000;
  const raw = v / 4;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw)!;
  return step * 4;
}

function compact(cents: number): string {
  const e = cents / 100;
  if (e >= 10000) return `${Math.round(e / 1000)}k`;
  if (e >= 1000) return `${(e / 1000).toLocaleString('es-ES', { maximumFractionDigits: 1 })}k`;
  return Math.round(e).toLocaleString('es-ES');
}

function weekday(iso: string): string {
  const d = new Date(`${iso}T12:00:00`);
  return d.toLocaleDateString(getLang() === 'es' ? 'es-ES' : 'en-GB', { weekday: 'short' });
}

function DayChart({ days }: { days: Summary['byDay'] }) {
  const [hover, setHover] = useState<number | null>(null);
  const top = niceMax(Math.max(...days.map((d) => d.totalCents)));
  const best = days.reduce((b, d, i) => (d.totalCents > days[b].totalCents ? i : b), 0);
  const ticks = [0, 1, 2, 3, 4].map((i) => (top / 4) * i);
  const showEvery = Math.ceil(days.length / 12);

  return (
    <div className="chart">
      <div className="chart-plot">
        <div className="chart-grid" aria-hidden="true">
          {ticks.map((t) => (
            <div key={t} className="chart-gridline" style={{ bottom: `${(t / top) * 100}%` }}>
              <span>{compact(t)}</span>
            </div>
          ))}
        </div>
        <div className="chart-cols" onMouseLeave={() => setHover(null)}>
          {days.map((d, i) => (
            <div key={d.date} className={`chart-col${hover === i ? ' hover' : ''}`} onMouseEnter={() => setHover(i)}>
              <div className="chart-bar" style={{ height: `${(d.totalCents / top) * 100}%` }}>
                {i === best && d.totalCents > 0 && hover === null && (
                  <span className="chart-cap">{d.totalCents < 100000 ? eur(d.totalCents) : `${compact(d.totalCents)} €`}</span>
                )}
              </div>
              {hover === i && (
                <div className={`chart-tip${i > days.length / 2 ? ' left' : ''}`} role="tooltip">
                  <b>
                    {weekday(d.date)} {shortDate(d.date)}
                  </b>
                  <span>{eur(d.totalCents)}</span>
                  <span className="muted">
                    {d.count} {d.count === 1 ? 'ticket' : 'tickets'}
                  </span>
                </div>
              )}
            </div>
          ))}
        </div>
      </div>
      <div className="chart-x" aria-hidden="true">
        {days.map((d, i) => (
          <span key={d.date}>{i % showEvery === 0 ? d.date.slice(8, 10) : ''}</span>
        ))}
      </div>
    </div>
  );
}

export function Reports() {
  const toast = useToast();
  const [preset, setPreset] = useState<Preset>('today');
  const [range, setRange] = useState(() => presetRange('today')!);
  const [sum, setSum] = useState<Summary | null>(null);
  const [table, setTable] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const r = presetRange(preset);
    if (r) setRange(r);
  }, [preset]);

  useEffect(() => {
    if (range.from > range.to) return;
    call<Summary>('reports.summary', range)
      .then(setSum)
      .catch((e) => toast.error(errorText(e)));
  }, [range, toast]);

  const methodMax = useMemo(() => Math.max(1, ...(sum?.byMethod ?? []).map((m) => m.totalCents)), [sum]);

  const run = async (cmd: 'reports.print' | 'reports.export') => {
    if (busy) return;
    setBusy(true);
    try {
      const r = await call<{ path?: string } | null>(cmd, range);
      if (cmd === 'reports.print') toast.ok(tr('Resumen enviado a la impresora', 'Summary sent to the printer'));
      else toast.ok(`${tr('Exportado a', 'Exported to')} ${r?.path ?? ''}`, { sticky: true });
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const avg = sum && sum.tickets ? Math.round(sum.totalCents / sum.tickets) : 0;

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>{tr('Informes', 'Reports')}</h1>
          <p className="muted">
            {range.from === range.to ? shortDate(range.from) : `${shortDate(range.from)} – ${shortDate(range.to)}`}
            {sum?.firstNumber && ` · ${sum.firstNumber} → ${sum.lastNumber}`}
          </p>
        </div>
        <div className="page-actions">
          <button className="btn" onClick={() => run('reports.print')} disabled={busy || !sum}>
            <Printer size={16} />
            {tr('Imprimir resumen', 'Print summary')}
          </button>
          <button className="btn" onClick={() => run('reports.export')} disabled={busy || !sum}>
            <FileSpreadsheet size={16} />
            {tr('Exportar CSV', 'Export CSV')}
          </button>
        </div>
      </div>

      <div className="filters">
        <div className="chips">
          {PRESETS.map((p) => (
            <button key={p.id} className={`chip${preset === p.id ? ' on' : ''}`} onClick={() => setPreset(p.id)}>
              {p.label()}
            </button>
          ))}
        </div>
        {preset === 'custom' && (
          <div className="dates">
            <input type="date" value={range.from} max={range.to} onChange={(e) => e.target.value && setRange({ ...range, from: e.target.value })} />
            <span>–</span>
            <input type="date" value={range.to} min={range.from} onChange={(e) => e.target.value && setRange({ ...range, to: e.target.value })} />
          </div>
        )}
      </div>

      {sum && (
        <>
          <div className="stats">
            <div className="stat hero">
              <span>{tr('Ventas', 'Sales')}</span>
              <b>{eur(sum.totalCents)}</b>
              {sum.voided > 0 && (
                <small>
                  {sum.voided} {sum.voided === 1 ? tr('ticket anulado no incluido', 'voided ticket excluded') : tr('tickets anulados no incluidos', 'voided tickets excluded')}
                </small>
              )}
            </div>
            <div className="stat">
              <span>Tickets</span>
              <b>{sum.tickets}</b>
            </div>
            <div className="stat">
              <span>{tr('Ticket medio', 'Average ticket')}</span>
              <b>{eur(avg)}</b>
            </div>
            <div className="stat">
              <span>{tr('Unidades vendidas', 'Units sold')}</span>
              <b>{sum.items}</b>
            </div>
          </div>

          {sum.tickets === 0 ? (
            <div className="card">
              <Empty icon={<ChartColumn size={28} />} title={tr('Sin ventas en este periodo', 'No sales in this period')} />
            </div>
          ) : (
            <div className="report-grid">
              {sum.byDay.length > 1 && (
                <div className="card span2">
                  <div className="card-head">
                    <h2>{tr('Ventas por día', 'Sales per day')}</h2>
                    <button className="link" onClick={() => setTable(!table)}>
                      {table ? tr('Ver gráfico', 'Show chart') : tr('Ver tabla', 'Show table')}
                    </button>
                  </div>
                  {table ? (
                    <div className="scroll-y">
                      <table className="table compact">
                        <thead>
                          <tr>
                            <th>{tr('Día', 'Day')}</th>
                            <th className="num">Tickets</th>
                            <th className="num">{tr('Ventas', 'Sales')}</th>
                          </tr>
                        </thead>
                        <tbody>
                          {sum.byDay.map((d) => (
                            <tr key={d.date}>
                              <td>
                                {weekday(d.date)} {shortDate(d.date)}
                              </td>
                              <td className="num">{d.count}</td>
                              <td className="num">{eur(d.totalCents)}</td>
                            </tr>
                          ))}
                        </tbody>
                      </table>
                    </div>
                  ) : (
                    <DayChart days={sum.byDay} />
                  )}
                </div>
              )}

              <div className="card">
                <div className="card-head">
                  <h2>{tr('Formas de pago', 'Payment methods')}</h2>
                </div>
                <div className="hbars">
                  {sum.byMethod.map((m) => (
                    <div key={m.method} className="hbar">
                      <div className="hbar-label">
                        <MethodIcon m={m.method} />
                        <span>{methodName(m.method)}</span>
                        <small className="muted">
                          {m.count} {m.count === 1 ? 'ticket' : 'tickets'}
                        </small>
                      </div>
                      <div className="hbar-track">
                        <div className="hbar-fill" style={{ width: `${(m.totalCents / methodMax) * 100}%` }} />
                      </div>
                      <b className="hbar-value">{eur(m.totalCents)}</b>
                    </div>
                  ))}
                </div>
              </div>

              <div className="card">
                <div className="card-head">
                  <h2>{tr('Desglose de IVA', 'VAT breakdown')}</h2>
                </div>
                <table className="table compact">
                  <thead>
                    <tr>
                      <th>IVA</th>
                      <th className="num">{tr('Base', 'Net')}</th>
                      <th className="num">{tr('Cuota', 'VAT')}</th>
                      <th className="num">Total</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sum.vat.map((v) => (
                      <tr key={v.vatBp}>
                        <td>{rate(v.vatBp)}</td>
                        <td className="num">{fmt(v.baseCents)}</td>
                        <td className="num">{fmt(v.vatCents)}</td>
                        <td className="num strong">{fmt(v.totalCents)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <div className={`card${sum.byDay.length > 1 ? ' span2' : ''}`}>
                <div className="card-head">
                  <h2>{tr('Lo más vendido', 'Best sellers')}</h2>
                </div>
                <table className="table compact">
                  <thead>
                    <tr>
                      <th>{tr('Producto', 'Product')}</th>
                      <th className="num">{tr('Unidades', 'Units')}</th>
                      <th className="num">{tr('Importe', 'Amount')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sum.top.map((p) => (
                      <tr key={p.name}>
                        <td>{p.name}</td>
                        <td className="num">{p.qty}</td>
                        <td className="num strong">{eur(p.totalCents)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}
