import { useEffect, useRef, useState } from 'react';
import { CreditCard, Plus, RotateCcw, SmartphoneCharging, Trash2 } from 'lucide-react';
import { tr } from './i18n';
import { cardMachines, live, rechargeMachines } from './machines';
import type { Machine, Settings } from './types';
import { Field, Segmented, Toggle } from './ui';

// the most a shop is offered; the core keeps to the same number
const MOST = 12;

// a percentage as people type it: "3", "1,5"
function Percent({ bp, onChange, label }: { bp: number; onChange: (bp: number) => void; label: string }) {
  const shown = (b: number) => (b / 100).toLocaleString('es-ES', { maximumFractionDigits: 2 });
  const [text, setText] = useState(shown(bp));
  useEffect(() => {
    if (read(text) !== bp) setText(shown(bp));
  }, [bp]);
  return (
    <div className="money-input percent">
      <input
        value={text}
        inputMode="decimal"
        aria-label={label}
        onChange={(e) => {
          setText(e.target.value);
          const v = read(e.target.value);
          if (v !== null) onChange(v);
        }}
        onBlur={() => setText(shown(bp))}
      />
      <span>%</span>
    </div>
  );
}

function read(text: string): number | null {
  const t = text.trim().replace(',', '.');
  if (!t) return 0;
  const n = Number(t);
  return Number.isFinite(n) && n >= 0 && n <= 100 ? Math.round(n * 100) : null;
}

function MachineCard({ m, focus, single, onChange, onRemove }: { m: Machine; focus: boolean; single: boolean; onChange: (c: Partial<Machine>) => void; onRemove: () => void }) {
  const name = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!focus) return;
    name.current?.focus();
    name.current?.select();
  }, [focus]);
  return (
    <div className="machine-card">
      <div className="machine-head">
        <span className={`machine-mark${m.card ? '' : ' top-up'}`}>{m.card ? <CreditCard size={18} /> : <SmartphoneCharging size={18} />}</span>
        <input ref={name} className="machine-name" value={m.name} onChange={(e) => onChange({ name: e.target.value })} maxLength={30} placeholder={tr('Nombre de la máquina', 'Machine name')} aria-label={tr('Nombre de la máquina', 'Machine name')} />
        {!single && (
          <button className="btn btn-ghost sm" onClick={onRemove}>
            <Trash2 size={15} />
            {tr('Quitar', 'Remove')}
          </button>
        )}
      </div>
      <div className="grid2">
        <Toggle checked={m.card} onChange={(v) => onChange({ card: v })} label={tr('Cobra tarjetas', 'Takes cards')} />
        <Toggle checked={m.recharge} onChange={(v) => onChange({ recharge: v })} label={tr('Hace recargas', 'Does recharges')} />
      </div>
      {m.recharge && (
        <div className="machine-more">
          <div className="machine-row">
            <Toggle checked={m.rechargeExpense} onChange={(v) => onChange({ rechargeExpense: v })} label={tr('Sus recargas cuentan como gasto', 'Its recharges count as an expense')} hint={tr('Menos la comisión, que es lo que gana la tienda', 'Less the commission, which is what the shop earns')} />
            {m.rechargeExpense && (
              <label className="machine-percent">
                <span>{tr('Comisión', 'Commission')}</span>
                <Percent bp={m.commissionBp} onChange={(v) => onChange({ commissionBp: v })} label={tr('Comisión', 'Commission')} />
              </label>
            )}
          </div>
          {m.card && (
            <div className="machine-row">
              <span className="machine-label">{tr('El cliente de una recarga paga', 'A recharge customer pays')}</span>
              <Segmented
                value={m.rechargeCash ? 'cash' : 'card'}
                onChange={(v) => onChange({ rechargeCash: v === 'cash' })}
                options={[
                  { value: 'card', label: tr('Con tarjeta', 'By card') },
                  { value: 'cash', label: tr('En efectivo', 'In cash') },
                ]}
              />
            </div>
          )}
        </div>
      )}
      {m.card && (
        <div className="machine-more">
          <div className="machine-row">
            <Toggle
              checked={m.takingsBp > 0}
              onChange={(v) => onChange({ takingsBp: v ? 10_000 : 0 })}
              label={tr('Lo que cobra en ventas cuenta como gasto', 'What it takes in sales counts as an expense')}
              hint={tr('Todo, si ese dinero no es de la tienda. O solo un tanto por ciento, como la comisión del banco.', 'All of it, if that money is not the shop’s. Or just a percentage, like the bank’s fee.')}
            />
            {m.takingsBp > 0 && (
              <label className="machine-percent">
                <span>{tr('Parte', 'Share')}</span>
                <Percent bp={m.takingsBp} onChange={(v) => onChange({ takingsBp: v })} label={tr('Parte que cuenta como gasto', 'Share that counts as an expense')} />
              </label>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

// The shop's machines, in a shop's words: with one nothing is ever asked; with
// several the card button and the recharge window ask which. Each machine says
// on its own what it does and what of it counts as an expense.
export function MachinesTab({ d, set }: { d: Settings; set: <K extends keyof Settings>(k: K, v: Settings[K]) => void }) {
  const [fresh, setFresh] = useState<number | null>(null);
  const shop = live(d);
  const gone = d.machines.filter((m) => m.removed);
  const cards = cardMachines(d);
  const tops = rechargeMachines(d);

  const put = (id: number, c: Partial<Machine>) => set('machines', d.machines.map((m) => (m.id === id ? { ...m, ...c } : m)));

  const add = (card: boolean) => {
    const id = Math.max(0, ...d.machines.map((m) => m.id)) + 1;
    const n = shop.filter((m) => m.card === card).length + 1;
    const name = card ? (n > 1 ? `${tr('Datáfono', 'Card machine')} ${n}` : tr('Datáfono', 'Card machine')) : n > 1 ? `${tr('Recargas', 'Recharges')} ${n}` : tr('Recargas', 'Recharges');
    set('machines', [...d.machines, { id, name, card, recharge: !card, rechargeCash: false, rechargeExpense: true, commissionBp: d.rechargeCommissionBp, takingsBp: 0, removed: false }]);
    setFresh(id);
  };

  return (
    <div className="stack">
      <p className="muted">
        {tr(
          'Ponles el nombre que quieras. Con una sola, Tiquet no pregunta nada. Con varias, pregunta cuál al cobrar con tarjeta y al hacer una recarga.',
          'Give them any name you like. With one, Tiquet asks nothing. With several, it asks which one when charging a card and when doing a recharge.',
        )}
      </p>
      {shop.map((m) => (
        <MachineCard key={m.id} m={m} focus={fresh === m.id} single={shop.length === 1} onChange={(c) => put(m.id, c)} onRemove={() => put(m.id, { removed: true })} />
      ))}
      <div className="row-gap">
        <button className="btn" onClick={() => add(true)} disabled={shop.length >= MOST}>
          <Plus size={16} />
          {tr('Máquina de tarjetas', 'Card machine')}
        </button>
        <button className="btn" onClick={() => add(false)} disabled={shop.length >= MOST}>
          <Plus size={16} />
          {tr('Máquina de recargas', 'Recharge machine')}
        </button>
      </div>
      {gone.length > 0 && (
        <p className="field-hint machines-gone">
          {tr('Quitadas', 'Removed')}:{' '}
          {gone.map((m) => (
            <button key={m.id} className="link" onClick={() => put(m.id, { removed: false })} title={tr('Volver a usarla', 'Use it again')}>
              <RotateCcw size={12} /> {m.name}
            </button>
          ))}
        </p>
      )}

      {cards.length > 1 && (
        <>
          <Field label={tr('Máquina de tarjetas predeterminada', 'Default card machine')} hint={tr('La primera al elegir, y la que usa Enter', 'First when choosing, and the one Enter takes')}>
            <select value={d.cardMachine} onChange={(e) => set('cardMachine', Number(e.target.value))}>
              {cards.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </Field>
          <Toggle
            checked={d.machineAsk}
            onChange={(v) => set('machineAsk', v)}
            label={tr('Preguntar la máquina en cada cobro con tarjeta', 'Ask which machine on every card sale')}
            hint={tr('Apagado, se usa la predeterminada, y se puede cambiar después en el ticket.', 'Off, the default one is used, and it can be changed afterwards on the ticket.')}
          />
        </>
      )}

      <h3 className="settings-title">{tr('Recargas', 'Recharges')}</h3>
      <Toggle
        checked={d.showRecharge}
        onChange={(v) => set('showRecharge', v)}
        label={tr('Esta tienda hace recargas', 'This shop does recharges')}
        hint={tr('Apagado, desaparecen el botón y su total de la pantalla de venta.', 'Off, the button and its total leave the sale page.')}
      />
      {d.showRecharge && (
        <>
          <Field label={tr('Cómo se hacen normalmente', 'How they are usually done')} hint={tr('Con lo que empieza la ventana de recarga', 'What the recharge window starts on')}>
            <select value={d.rechargeMachine} onChange={(e) => set('rechargeMachine', Number(e.target.value))}>
              <option value={0}>{tr('Efectivo', 'Cash')}</option>
              {tops.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))}
            </select>
          </Field>
          <div className="machine-row">
            <Toggle
              checked={d.rechargeCashExpense}
              onChange={(v) => set('rechargeCashExpense', v)}
              label={tr('Las recargas en efectivo cuentan como gasto', 'Cash recharges count as an expense')}
              hint={tr('Las que no pasan por una máquina. Lo normal es dejarlo apagado.', 'Those that go through no machine. It is normally left off.')}
            />
            {d.rechargeCashExpense && (
              <label className="machine-percent">
                <span>{tr('Comisión', 'Commission')}</span>
                <Percent bp={d.rechargeCommissionBp} onChange={(v) => set('rechargeCommissionBp', v)} label={tr('Comisión', 'Commission')} />
              </label>
            )}
          </div>
          <Toggle
            checked={d.rechargePrint}
            onChange={(v) => set('rechargePrint', v)}
            label={tr('Imprimir un justificante con cada recarga', 'Print a receipt with every recharge')}
            hint={tr('Apagado, se imprime solo cuando el cliente lo pide, desde Hoy.', 'Off, it only prints when the customer asks, from Today.')}
          />
          <Field label={tr('Importes', 'Amounts')} hint={tr('Un botón por importe, separados por comas', 'One button per amount, separated by commas')}>
            <input value={d.rechargeAmounts} onChange={(e) => set('rechargeAmounts', e.target.value)} placeholder="5, 10, 15, 20, 30, 50" maxLength={120} />
          </Field>
          <Field label={tr('Operadores (opcional)', 'Operators (optional)')} hint={tr('Para saber cuánto se recargó de cada uno, separados por comas', 'To know how much was recharged with each, separated by commas')}>
            <input value={d.rechargeOperators} onChange={(e) => set('rechargeOperators', e.target.value)} placeholder="Vodafone, Orange, Movistar, Lycamobile, Lebara, Digi" maxLength={200} />
          </Field>
        </>
      )}
    </div>
  );
}

// the totals the main page can show, in the order they appear
const CELLS: { id: string; label: () => string }[] = [
  { id: 'cash', label: () => tr('Efectivo', 'Cash') },
  { id: 'card', label: () => tr('Tarjeta', 'Card') },
  { id: 'recharge', label: () => tr('Recargas', 'Recharges') },
  { id: 'total', label: () => tr('Total', 'Total') },
  { id: 'expense', label: () => tr('Gastos', 'Expenses') },
  { id: 'net', label: () => tr('Queda', 'Left') },
];

export function DayCells({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const on = value.split(',').filter(Boolean);
  const flip = (id: string) => onChange(CELLS.filter((c) => (c.id === id ? !on.includes(id) : on.includes(c.id))).map((c) => c.id).join(','));
  return (
    <div className="picks">
      {CELLS.map((c) => (
        <button key={c.id} type="button" className={`pick${on.includes(c.id) ? ' on' : ''}`} aria-pressed={on.includes(c.id)} onClick={() => flip(c.id)}>
          {c.label()}
        </button>
      ))}
    </div>
  );
}
