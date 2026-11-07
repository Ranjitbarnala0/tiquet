import { useMemo } from 'react';
import { fmt } from './format';
import { tr } from './i18n';
import { machineName } from './machines';
import type { DayRow, Settings } from './types';

const sold = (r: DayRow) => r.source === 'ticket' || r.source === 'sale';
const as = (r: DayRow) => r.cents;
// among the expenses, what a recharge or a sale counts as one
const spent = (r: DayRow) => (r.source === 'expense' ? r.cents : -r.costCents);

// a column does not need more than the eye can take in; Hoy has them all
const MOST = 60;

interface Column {
  id: string;
  label: string;
  rows: DayRow[];
  cents: (r: DayRow) => number;
}

function columns(rows: DayRow[], s: Settings): Column[] {
  const by = (method: string) => rows.filter((r) => sold(r) && r.method === method);
  const out: Column[] = [
    { id: 'cash', label: tr('Efectivo', 'Cash'), rows: by('cash'), cents: as },
    { id: 'card', label: tr('Tarjeta', 'Card'), rows: by('card'), cents: as },
  ];
  const bizum = by('bizum');
  if (s.payBizum || bizum.length) out.push({ id: 'bizum', label: 'Bizum', rows: bizum, cents: as });
  const recharges = rows.filter((r) => r.source === 'recharge');
  if (s.showRecharge || recharges.length) out.push({ id: 'recharge', label: tr('Recargas', 'Recharges'), rows: recharges, cents: as });
  const expenses = rows.filter((r) => r.source === 'expense' || r.costCents !== 0);
  if (s.showExpense || expenses.length) out.push({ id: 'expense', label: tr('Gastos', 'Expenses'), rows: expenses, cents: spent });
  return out;
}

// The day as it happens, each way of paying in its own stack with the newest on
// top. Nothing here can be tapped: it only shows, so it can never add anything
// to a sale.
export function Board({ rows, settings }: { rows: DayRow[]; settings: Settings }) {
  const stacks = useMemo(() => columns(rows, settings), [rows, settings]);
  const several = settings.machines.length > 1;
  if (!rows.length) {
    return (
      <div className="board empty">
        <p>{tr('Aquí se irá apilando lo que hagas hoy, cada cosa en su sitio.', 'What you do today will stack up here, each thing in its place.')}</p>
      </div>
    );
  }
  return (
    <div className="board">
      {stacks.map((c) => (
        <section key={c.id} className="board-col">
          <h3>
            <i className={`dot dot-${c.id}`} aria-hidden="true" />
            <span>{c.label}</span>
            {c.rows.length > 0 && <small>{c.rows.length}</small>}
          </h3>
          <ul>
            {c.rows.slice(0, MOST).map((r) => (
              <li key={`${r.source}-${r.id}`}>
                <span>{r.concept}</span>
                <b className={c.cents(r) < 0 ? 'neg' : ''}>{fmt(c.cents(r))}</b>
                <small>
                  {r.at.slice(11, 16)}
                  {several && r.machine ? ` · ${machineName(settings, r.machine)}` : ''}
                </small>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
