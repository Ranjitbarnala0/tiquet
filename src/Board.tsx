import { useMemo } from 'react';
import { fmt } from './format';
import { tr } from './i18n';
import { cardMachines, machineName } from './machines';
import type { DayRow, Settings } from './types';

const sold = (r: DayRow) => r.source === 'ticket' || r.source === 'sale';
const as = (r: DayRow) => r.cents;
// among the expenses, what a recharge or a sale counts as one
const spent = (r: DayRow) => (r.source === 'expense' ? r.cents : -r.costCents);

// a column does not need more than the eye can take in; Hoy has them all
const MOST = 60;

interface Column {
  id: string;
  dot: string;
  label: string;
  rows: DayRow[];
  cents: (r: DayRow) => number;
}

function columns(rows: DayRow[], s: Settings): Column[] {
  const by = (method: string) => rows.filter((r) => sold(r) && r.method === method);
  const out: Column[] = [{ id: 'cash', dot: 'cash', label: tr('Efectivo', 'Cash'), rows: by('cash'), cents: as }];

  const cardRows = by('card');
  const cards = cardMachines(s);
  const distinctCardMachines = Array.from(new Set(cardRows.map((r) => r.machine || 0)));
  if (s.splitCardMachines !== false && (cards.length > 1 || distinctCardMachines.length > 1)) {
    const shown = new Set<number>();
    cards.forEach((m) => {
      shown.add(m.id);
      const mRows = cardRows.filter((r) => (r.machine || 0) === m.id);
      out.push({ id: `card-${m.id}`, dot: 'card', label: m.name, rows: mRows, cents: as });
    });
    distinctCardMachines.forEach((id) => {
      if (shown.has(id)) return;
      const mRows = cardRows.filter((r) => (r.machine || 0) === id);
      if (mRows.length) {
        out.push({ id: `card-${id}`, dot: 'card', label: id ? machineName(s, id) : tr('Tarjeta', 'Card'), rows: mRows, cents: as });
      }
    });
  } else {
    out.push({ id: 'card', dot: 'card', label: tr('Tarjeta', 'Card'), rows: cardRows, cents: as });
  }

  const bizum = by('bizum');
  if (s.payBizum || bizum.length) out.push({ id: 'bizum', dot: 'bizum', label: 'Bizum', rows: bizum, cents: as });
  const recharges = rows.filter((r) => r.source === 'recharge');
  if (s.showRecharge || recharges.length) out.push({ id: 'recharge', dot: 'recharge', label: tr('Recargas', 'Recharges'), rows: recharges, cents: as });
  const expenses = rows.filter((r) => r.source === 'expense' || r.costCents !== 0);
  if (s.showExpense || expenses.length) out.push({ id: 'expense', dot: 'expense', label: tr('Gastos', 'Expenses'), rows: expenses, cents: spent });
  return out;
}

// The day as it happens, each way of paying in its own stack with the newest on
// top. Nothing here can be tapped: it only shows, so it can never add anything
// to a sale.
export function Board({ rows, settings }: { rows: DayRow[]; settings: Settings }) {
  const stacks = useMemo(() => columns(rows, settings), [rows, settings]);
  const showMachineTag = settings.machines.length > 1 && settings.splitCardMachines === false;
  if (!rows.length) {
    return (
      <div className="board empty">
        <p>{tr('Aquí se irá apilando lo que hagas hoy, cada cosa en su sitio.', 'What you do today will stack up here, each thing in its place.')}</p>
      </div>
    );
  }
  return (
    <div className="board">
      {stacks.map((c) => {
        const total = c.rows.reduce((sum, r) => sum + c.cents(r), 0);
        return (
          <section key={c.id} className="board-col" data-kind={c.dot}>
            <h3>
              <i className={`dot dot-${c.dot}`} aria-hidden="true" />
              <span>{c.label}</span>
              {c.rows.length > 0 && (
                <small>
                  {fmt(total)} € · {c.rows.length}
                </small>
              )}
            </h3>
            <ul>
              {c.rows.slice(0, MOST).map((r) => (
                <li key={`${r.source}-${r.id}`}>
                  <span>{r.concept}</span>
                  <b className={c.cents(r) < 0 ? 'neg' : ''}>{fmt(c.cents(r))}</b>
                  <small>
                    {r.at.slice(11, 16)}
                    {showMachineTag && r.machine ? ` · ${machineName(settings, r.machine)}` : ''}
                  </small>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}
