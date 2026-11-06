import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { Banknote, CreditCard, Smartphone, SmartphoneCharging, type LucideIcon } from 'lucide-react';
import { call, toError } from './api';
import { eur, fmt, isoDate, parseMoney, rate, VAT_RATES } from './format';
import { errorText, tr } from './i18n';
import { machineName, rechargeMachines } from './machines';
import { useToast } from './toast';
import type { BookMethod, DayRow, Settings } from './types';
import { Field, Modal, Segmented } from './ui';

// an amount this big is checked twice: a typo of 1000 for 10,00 is easy
export const BIG = 50_000;

export interface EntryDraft {
  kind: 'expense' | 'recharge';
  concept?: string;
  cents: number;
  method: BookMethod;
  // the machine a recharge was made on
  machine?: number;
  supplier?: string;
  supplierNif?: string;
  invoiceRef?: string;
  vatBp?: number;
  date?: string;
}

interface Quick {
  concept: string;
  cents: number;
  uses: number;
}

const newId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

const list = (text: string) =>
  text
    .split(/[,;]/)
    .map((x) => x.trim())
    .filter(Boolean);

// A recharge or an expense saved with one action, and an undo right after
// instead of an "are you sure?". A double tap or a second Enter saves it once.
export function useEntrySave(onChanged: () => void) {
  const toast = useToast();
  const last = useRef({ key: '', at: 0 });
  const busy = useRef(false);
  return useCallback(
    async (entry: EntryDraft): Promise<boolean> => {
      const key = JSON.stringify(entry);
      if (busy.current || (last.current.key === key && Date.now() - last.current.at < 1500)) return false;
      busy.current = true;
      try {
        const r = await call<{ entry: { id: number; concept: string; cents: number }; printError: string | null }>('book.add', { ...entry, clientId: newId() });
        last.current = { key, at: Date.now() };
        const id = r.entry.id;
        if (r.printError) toast.error(`${tr('Guardado, pero el justificante no se imprimió', 'Saved, but the receipt did not print')}: ${errorText(toError(r.printError))}`);
        toast.ok(`${r.entry.concept} · ${eur(r.entry.cents)}`, {
          action: {
            label: tr('Deshacer', 'Undo'),
            run: () =>
              call('book.delete', { id, reason: tr('Deshecho al momento', 'Undone right away') })
                .then(onChanged)
                .catch((e) => toast.error(errorText(e))),
          },
        });
        onChanged();
        return true;
      } catch (e) {
        toast.error(errorText(e));
        return false;
      } finally {
        busy.current = false;
      }
    },
    [onChanged, toast],
  );
}

// cash and card, and what an older entry was paid with if it was neither
const pays = (now: BookMethod = 'cash'): { value: BookMethod; label: string }[] => [
  { value: 'cash', label: tr('Efectivo', 'Cash') },
  { value: 'card', label: tr('Tarjeta', 'Card') },
  ...(now === 'bizum' ? [{ value: now, label: 'Bizum' }] : now === 'transfer' ? [{ value: now, label: tr('Transferencia', 'Transfer') }] : []),
];

// the ways a recharge can be done: in cash, or on each machine that does them
interface Way {
  key: string;
  label: string;
  icon: LucideIcon;
  method: BookMethod;
  machine?: number;
}

const wayKey = (method: BookMethod, machine?: number | null) => (machine ? `m${machine}` : method);

function ways(s: Settings, entry?: DayRow): Way[] {
  const out: Way[] = [{ key: 'cash', label: tr('Efectivo', 'Cash'), icon: Banknote, method: 'cash' }];
  for (const m of rechargeMachines(s)) out.push({ key: `m${m.id}`, label: m.name, icon: m.card ? CreditCard : SmartphoneCharging, method: m.card && !m.rechargeCash ? 'card' : 'cash', machine: m.id });
  if (s.payBizum) out.push({ key: 'bizum', label: 'Bizum', icon: Smartphone, method: 'bizum' });
  // what the recharge being corrected was done with stays on offer
  if (entry && !out.some((w) => w.key === wayKey(entry.method, entry.machine))) {
    const label = entry.machine ? machineName(s, entry.machine) : entry.method === 'card' ? tr('Tarjeta', 'Card') : entry.method === 'bizum' ? 'Bizum' : tr('Transferencia', 'Transfer');
    out.push({ key: wayKey(entry.method, entry.machine), label, icon: CreditCard, method: entry.method, machine: entry.machine ?? undefined });
  }
  return out;
}

// what a recharge done this way counts as an expense
function wayCost(s: Settings, w: Way, cents: number): number {
  const spent = (bp: number) => cents - Math.round((cents * bp) / 10_000);
  if (w.machine) {
    const m = s.machines.find((x) => x.id === w.machine);
    return m?.rechargeExpense ? spent(m.commissionBp) : 0;
  }
  return s.rechargeCashExpense ? spent(s.rechargeCommissionBp) : 0;
}

// Tap how it is done, tap an amount and it is saved. It always counts in the
// recharges; on a machine it counts as an expense too, as the shop has set.
export function RechargeModal({
  settings,
  cents,
  entry,
  onSave,
  onClose,
}: {
  settings: Settings;
  cents: number;
  // the recharge being corrected
  entry?: DayRow;
  onSave: (e: EntryDraft) => Promise<boolean>;
  onClose: (saved: boolean) => void;
}) {
  const amounts = useMemo(() => list(settings.rechargeAmounts).map((a) => parseMoney(a)).filter((c): c is number => !!c && c > 0), [settings.rechargeAmounts]);
  const operators = useMemo(() => {
    const all = list(settings.rechargeOperators);
    return entry?.supplier && !all.includes(entry.supplier) ? [...all, entry.supplier] : all;
  }, [settings.rechargeOperators, entry]);
  const [text, setText] = useState(cents > 0 ? fmt(cents) : '');
  const [operator, setOperator] = useState(entry?.supplier ?? '');
  const options = useMemo(() => ways(settings, entry), [settings, entry]);
  const [way, setWay] = useState(() => (entry ? wayKey(entry.method, entry.machine) : wayKey('cash', settings.rechargeMachine)));
  const chosen = options.find((w) => w.key === way) ?? options[0];
  const typed = parseMoney(text);
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);

  // F9 is cash and F10 the machines, one after the other, as on the sale page
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key !== 'F9' && e.key !== 'F10') return;
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'F9') {
        setWay('cash');
        return;
      }
      const machines = options.filter((w) => w.machine);
      if (machines.length) setWay((cur) => machines[(machines.findIndex((w) => w.key === cur) + 1) % machines.length].key);
    };
    window.addEventListener('keydown', k, true);
    return () => window.removeEventListener('keydown', k, true);
  }, [options]);

  const save = async (amount: number) => {
    if (await onSave({ kind: 'recharge', cents: amount, method: chosen.method, machine: chosen.machine, supplier: operator })) onClose(true);
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (typed && typed > 0) save(typed);
  };

  return (
    <Modal title={entry ? tr('Corregir la recarga', 'Correct the recharge') : tr('Recarga', 'Phone recharge')} onClose={() => onClose(false)}>
      <form className="quick-form" onSubmit={submit}>
        {options.length > 1 && (
          <div className="ways" role="radiogroup" aria-label={tr('Cómo se hace', 'How it is done')}>
            {options.map((w) => (
              <button
                key={w.key}
                type="button"
                role="radio"
                aria-checked={chosen.key === w.key}
                className={`way${chosen.key === w.key ? ' on' : ''}`}
                onClick={() => {
                  setWay(w.key);
                  ref.current?.focus();
                }}
              >
                <w.icon size={18} />
                <span>{w.label}</span>
              </button>
            ))}
          </div>
        )}
        {operators.length > 0 && (
          <div className="picks">
            {operators.map((o) => (
              <button
                key={o}
                type="button"
                className={`pick${operator === o ? ' on' : ''}`}
                onClick={() => {
                  setOperator(operator === o ? '' : o);
                  ref.current?.focus();
                }}
              >
                {o}
              </button>
            ))}
          </div>
        )}
        <div className="amounts">
          {amounts.map((a) => (
            <button key={a} type="button" className="amount" onClick={() => save(a)}>
              {a % 100 === 0 ? `${a / 100} €` : eur(a)}
            </button>
          ))}
        </div>
        <div className="money-input big">
          <input ref={ref} value={text} onChange={(e) => setText(e.target.value)} inputMode="decimal" placeholder={tr('Otro importe', 'Other amount')} aria-label={tr('Importe', 'Amount')} />
          <span>€</span>
        </div>
        <button className="btn btn-recharge btn-xl" disabled={!typed || typed <= 0}>
          {typed && typed > 0 ? `${entry ? tr('Guardar', 'Save') : tr('Apuntar recarga', 'Save recharge')} · ${eur(typed)}` : tr('Toca un importe', 'Tap an amount')}
          <kbd className="on-dark">Enter</kbd>
        </button>
        <p className="field-hint">
          {wayCost(settings, chosen, 100)
            ? tr('Cuenta en las recargas del día y, a la vez, en los gastos.', 'It counts in the day’s recharges and, at the same time, in the expenses.')
            : tr('Cuenta en las recargas del día. No es un gasto.', 'It counts in the day’s recharges. It is not an expense.')}
        </p>
      </form>
    </Modal>
  );
}

// "pantalla iphone 45": the amount is the last word that reads as money
function split(text: string): { concept: string; cents: number } | null {
  const words = text.trim().split(/\s+/).filter(Boolean);
  for (let i = words.length - 1; i >= 0; i--) {
    const cents = parseMoney(words[i]);
    if (cents !== null && cents > 0) return { concept: [...words.slice(0, i), ...words.slice(i + 1)].join(' '), cents };
  }
  return null;
}

// What was bought or paid for and how much. Two boxes, Enter, done.
export function ExpenseModal({
  initial,
  entry,
  settings,
  onSave,
  onClose,
}: {
  initial: string;
  // the expense being corrected
  entry?: DayRow;
  settings: Settings;
  onSave: (e: EntryDraft) => Promise<boolean>;
  onClose: (saved: boolean) => void;
}) {
  const [concept, setConcept] = useState(entry?.concept ?? initial);
  const [text, setText] = useState(entry ? fmt(Math.abs(entry.cents)) : '');
  const [method, setMethod] = useState<BookMethod>(entry?.method ?? settings.expenseMethod);
  const [more, setMore] = useState(!!entry && !!(entry.supplier || entry.invoiceRef || entry.supplierNif));
  const [supplier, setSupplier] = useState(entry?.supplier ?? '');
  const [supplierNif, setSupplierNif] = useState(entry?.supplierNif ?? '');
  const [invoiceRef, setInvoiceRef] = useState(entry?.invoiceRef ?? '');
  const [vatBp, setVatBp] = useState(entry?.vatBp ?? settings.defaultVatBp);
  const [date, setDate] = useState(entry ? entry.at.slice(0, 10) : '');
  const [sure, setSure] = useState(false);
  const [recent, setRecent] = useState<Quick[]>([]);
  const amountRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    call<Quick[]>('book.quick')
      .then(setRecent)
      .catch(() => {});
  }, []);

  const typed = parseMoney(text);
  const parsed = typed && typed > 0 ? { concept: concept.trim(), cents: typed } : split(concept);
  const usual = useMemo(() => list(settings.expenseConcepts), [settings.expenseConcepts]);

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!parsed) {
      amountRef.current?.focus();
      return;
    }
    if (parsed.cents >= BIG && !sure) {
      setSure(true);
      return;
    }
    const extra = more || entry ? { supplier, supplierNif, invoiceRef, vatBp, date } : {};
    if (await onSave({ kind: 'expense', method, ...parsed, ...extra })) onClose(true);
  };

  return (
    <Modal title={entry ? tr('Corregir el gasto', 'Correct the expense') : tr('Gasto', 'Expense')} onClose={() => onClose(false)}>
      <form className="quick-form" onSubmit={submit}>
        <Field label={tr('¿Qué has comprado o pagado?', 'What did you buy or pay for?')}>
          <input
            autoFocus
            className="quick-input"
            value={concept}
            onChange={(e) => {
              setConcept(e.target.value);
              setSure(false);
            }}
            placeholder={tr('Pantalla iPhone 11, 10 fundas, cable…', 'iPhone 11 screen, 10 cases, cable…')}
            maxLength={120}
          />
        </Field>
        {(recent.length > 0 || usual.length > 0) && !concept && (
          <div className="picks">
            {usual.map((c) => (
              <button
                key={c}
                type="button"
                className="pick"
                onClick={() => {
                  setConcept(c);
                  amountRef.current?.focus();
                }}
              >
                {c}
              </button>
            ))}
            {recent.filter((q) => !usual.some((c) => c.toLowerCase() === q.concept.toLowerCase())).map((q) => (
              <button
                key={q.concept}
                type="button"
                className="pick"
                onClick={() => {
                  setConcept(q.concept);
                  setText(fmt(q.cents));
                  amountRef.current?.focus();
                }}
              >
                {q.concept} · {fmt(q.cents)}
              </button>
            ))}
          </div>
        )}
        <div className="quick-line">
          <div className="money-input big">
            <input
              ref={amountRef}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                setSure(false);
              }}
              inputMode="decimal"
              placeholder={tr('¿Cuánto?', 'How much?')}
              aria-label={tr('Importe', 'Amount')}
            />
            <span>€</span>
          </div>
          <Segmented value={method} onChange={setMethod} options={pays(entry?.method)} />
        </div>
        {sure && parsed && (
          <p className="form-error">
            {tr('Importe alto', 'Large amount')}: {eur(parsed.cents)}. {tr('Pulsa Enter otra vez para apuntarlo.', 'Press Enter again to save it.')}
          </p>
        )}
        <button className="btn btn-expense btn-xl" disabled={!parsed}>
          {parsed ? `${entry ? tr('Guardar', 'Save') : tr('Apuntar gasto', 'Save expense')} · ${eur(parsed.cents)}` : tr('Escribe qué y cuánto', 'Type what and how much')}
          <kbd className="on-dark">Enter</kbd>
        </button>
        <button type="button" className="link more-link" onClick={() => setMore(!more)}>
          {more ? tr('Menos detalles', 'Fewer details') : tr('Para el gestor: proveedor, factura, IVA, fecha', 'For the accountant: supplier, invoice, VAT, date')}
        </button>
        {more && (
          <div className="grid2">
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
            <Field label={tr('Fecha', 'Date')} hint={tr('Vacío: hoy', 'Empty: today')}>
              <input type="date" value={date} max={isoDate(new Date())} onChange={(e) => setDate(e.target.value)} />
            </Field>
          </div>
        )}
      </form>
    </Modal>
  );
}
