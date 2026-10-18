import { useCallback, useEffect, useRef, useState } from 'react';
import { Ban, HandCoins, PackageCheck, Pencil, Plus, Printer, Search, Tag, Wrench } from 'lucide-react';
import { ApiError, call, toError } from './api';
import { ClockModal } from './Clock';
import { dateTime, eur, fmt, parseMoney } from './format';
import { errorText, isPrinterSetup, isTransient, tr, warningText } from './i18n';
import type { ItemDraft } from './ItemDialog';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { Logo, Method, PLine, Repair, RepairCreated, RepairData, RepairDetail, RepairEvent, RepairStatus, RepairSummary, Settings, User } from './types';
import { Drawer, Empty, Field, Modal, Segmented } from './ui';

type Filter = 'open' | 'all' | RepairStatus;

// still in the shop
const OPEN: RepairStatus[] = ['received', 'in_progress', 'ready', 'not_repaired'];

const STATUS: Record<RepairStatus, [string, string]> = {
  received: ['Recibido', 'Received'],
  in_progress: ['En curso', 'In progress'],
  ready: ['Listo', 'Ready'],
  not_repaired: ['No reparado', 'Not repaired'],
  delivered: ['Entregado', 'Handed back'],
  cancelled: ['Cancelado', 'Cancelled'],
};

export const statusText = (s: RepairStatus) => tr(...STATUS[s]);

const BADGE: Record<RepairStatus, string> = { received: '', in_progress: 'blue', ready: 'green', not_repaired: 'amber', delivered: '', cancelled: 'red' };

function StatusBadge({ s }: { s: RepairStatus }) {
  return <span className={`badge ${BADGE[s]}`}>{statusText(s)}</span>;
}

const FILTERS: { id: Filter; label: () => string }[] = [
  { id: 'open', label: () => tr('En tienda', 'In the shop') },
  ...OPEN.map((s) => ({ id: s, label: () => statusText(s) })),
  { id: 'delivered', label: () => statusText('delivered') },
  { id: 'cancelled', label: () => statusText('cancelled') },
  { id: 'all', label: () => tr('Todos', 'All') },
];

const FIELD: Record<string, [string, string]> = {
  customerName: ['nombre', 'name'],
  customerPhone: ['teléfono', 'phone'],
  brand: ['marca', 'make'],
  model: ['modelo', 'model'],
  colour: ['color', 'colour'],
  imei: ['IMEI', 'IMEI'],
  fault: ['avería', 'fault'],
  notes: ['notas', 'notes'],
};

const price = (c: number | null | undefined) => (c === null || c === undefined ? tr('sin precio', 'no price') : eur(c));

function eventText(e: RepairEvent): string {
  const d = e.detail ?? {};
  switch (e.kind) {
    case 'created':
      return tr('Recibido en la tienda', 'Taken in');
    case 'status':
      return statusText(e.status);
    case 'edited': {
      const parts: string[] = [];
      if (d.fields?.includes('estimate')) parts.push(`${tr('Presupuesto', 'Estimate')} ${price(d.estimateFrom)} → ${price(d.estimateTo)}`);
      const rest = (d.fields ?? []).filter((f) => f !== 'estimate').map((f) => (FIELD[f] ? tr(...FIELD[f]) : f));
      if (rest.length) parts.push(`${tr('Cambiado', 'Changed')}: ${rest.join(', ')}`);
      return parts.join(' · ');
    }
    case 'delivered':
      return d.ticket ? `${tr('Entregado con el ticket', 'Handed back with ticket')} ${d.ticket}` : tr('Entregado sin nada que cobrar', 'Handed back with nothing to charge');
    case 'reopened':
      return `${tr('Anulado el ticket', 'Ticket')} ${d.ticket ?? ''}${tr(': vuelve a', ' voided: back to')} ${statusText(e.status)}`;
    case 'cancelled':
      return `${tr('Cancelado', 'Cancelled')}: ${d.reason ?? ''}`;
  }
}

function device(r: { brand: string; model: string; colour: string }) {
  const d = [r.brand, r.model].map((x) => x.trim()).filter(Boolean).join(' ');
  return r.colour.trim() ? `${d} · ${r.colour.trim()}` : d;
}

const dataOf = (r: Repair): RepairData => ({
  customerName: r.customerName,
  customerPhone: r.customerPhone,
  brand: r.brand,
  model: r.model,
  colour: r.colour,
  imei: r.imei,
  fault: r.fault,
  notes: r.notes,
  estimateCents: r.estimateCents,
});

// a 15-digit IMEI ends in a check digit; anything else is a serial number
function imeiLooksWrong(s: string): boolean {
  const d = s.replace(/[\s-]/g, '');
  if (!/^\d{15}$/.test(d)) return false;
  let sum = 0;
  for (let i = 0; i < 15; i++) {
    let n = Number(d[i]);
    if (i % 2 === 1) {
      n *= 2;
      if (n > 9) n -= 9;
    }
    sum += n;
  }
  return sum % 10 !== 0;
}

const newId = () => crypto.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

interface Draft {
  customerName: string;
  customerPhone: string;
  brand: string;
  model: string;
  colour: string;
  imei: string;
  fault: string;
  notes: string;
  estimate: string;
  deposit: string;
}

const BLANK: Draft = { customerName: '', customerPhone: '', brand: '', model: '', colour: '', imei: '', fault: '', notes: '', estimate: '', deposit: '' };

const toDraft = (r: Repair): Draft => ({ ...dataOf(r), estimate: r.estimateCents === null ? '' : fmt(r.estimateCents), deposit: '' });

interface FormProps {
  repair: Repair | null;
  settings: Settings;
  logo: Logo | null;
  onClose: () => void;
  onSaved: (r: Repair) => void;
  onSetupPrinter?: () => void;
}

// Taking a phone in, or correcting one still in the shop. The resguardo on
// the right is the core's own, as it will print.
function RepairForm({ repair, settings, logo, onClose, onSaved, onSetupPrinter }: FormProps) {
  const toast = useToast();
  const [d, setD] = useState<Draft>(() => (repair ? toDraft(repair) : BLANK));
  const [opened] = useState(() => JSON.stringify(repair ? toDraft(repair) : BLANK));
  const [method, setMethod] = useState<Method>('cash');
  const [given, setGiven] = useState('');
  const [paper, setPaper] = useState<{ resguardo: PLine[]; slip: PLine[]; number: string } | null>(null);
  const [show, setShow] = useState<'resguardo' | 'slip'>('resguardo');
  const [busy, setBusy] = useState(false);
  const [clock, setClock] = useState<{ last: string; print: boolean } | null>(null);
  const requestId = useRef(newId());
  const noPrinter = !settings.printerName.trim();
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => setD((p) => ({ ...p, [k]: v }));

  const estimate = d.estimate.trim() ? parseMoney(d.estimate) : null;
  const badEstimate = d.estimate.trim() !== '' && (estimate === null || estimate < 0);
  const deposit = d.deposit.trim() ? parseMoney(d.deposit) : 0;
  const badDeposit = deposit === null || deposit < 0;
  const depositCents = badDeposit ? 0 : deposit ?? 0;
  const depositHigh = !badEstimate && estimate !== null && depositCents > estimate;
  // the deposit ticket is a simplified invoice for a service
  const depositOver = depositCents > 40_000;
  const tendered = method === 'cash' && given.trim() ? parseMoney(given, 999_999_999) : null;
  const badGiven = method === 'cash' && given.trim() !== '' && tendered === null;
  const short = tendered !== null && tendered < depositCents;
  const data: RepairData = {
    customerName: d.customerName,
    customerPhone: d.customerPhone,
    brand: d.brand,
    model: d.model,
    colour: d.colour,
    imei: d.imei,
    fault: d.fault,
    notes: d.notes,
    estimateCents: badEstimate ? null : estimate,
  };
  const complete = !!d.customerName.trim() && !!d.customerPhone.trim() && !!(d.brand.trim() || d.model.trim()) && !!d.fault.trim();
  const valid = complete && !badEstimate && !badDeposit && !depositHigh && !depositOver && !badGiven && !short;
  const dirty = JSON.stringify(d) !== opened;

  const args = JSON.stringify({ ...data, depositCents, id: repair?.id });
  useEffect(() => {
    let live = true;
    const t = setTimeout(() => {
      call<{ resguardo: PLine[]; slip: PLine[]; number: string }>('repairs.preview', JSON.parse(args))
        .then((p) => live && setPaper(p))
        .catch(() => {});
    }, 150);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [args]);

  // what was taken in is saved even when the printer fails; a retry prints
  // whatever did not come out
  const report = (r: RepairCreated) => {
    const dep = r.deposit;
    const pay = dep?.ticket.payment;
    const change = pay && pay.method === 'cash' && pay.changeCents > 0 ? ` · ${tr('Cambio', 'Change')} ${eur(pay.changeCents)}` : '';
    const head = `${tr('Reparación', 'Repair')} ${r.repair.number}${dep ? ` · ${tr('anticipo', 'deposit')} ${dep.ticket.number}` : ''}${change}`;
    const failed = r.printError ?? dep?.printError ?? null;
    if (r.duplicate) {
      toast.info(`${tr('Esta reparación ya estaba guardada', 'This repair had already been saved')}: ${r.repair.number}`);
    } else if (failed) {
      const err = toError(failed);
      const text = `${head}. ${tr('Guardada, pero no se imprimió', 'Saved, but not printed')}: ${errorText(err)}`;
      const retry = async () => {
        try {
          if (dep?.printError) await call('tickets.print', { id: dep.ticket.id });
          if (r.printError) await call('repairs.print', { id: r.repair.id, what: 'both' });
          toast.ok(tr('Impreso', 'Printed'));
        } catch (e) {
          toast.error(errorText(e));
        }
      };
      if (isTransient(err)) toast.error(text, { sticky: true, action: { label: tr('Reintentar', 'Retry'), run: retry } });
      else if (isPrinterSetup(err) && onSetupPrinter) toast.error(text, { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } });
      else toast.error(text);
    } else {
      toast.ok(head);
      const warning = r.printWarning ?? dep?.printWarning;
      if (warning) toast.info(warningText(warning));
    }
  };

  const save = async (print: boolean, clockOk = false) => {
    if (!valid || busy) return;
    setBusy(true);
    try {
      if (repair) {
        const r = await call<RepairDetail>('repairs.update', { id: repair.id, ...data });
        toast.ok(tr('Cambios guardados', 'Changes saved'));
        onSaved(r.repair);
        return;
      }
      const r = await call<RepairCreated>('repairs.create', {
        ...data,
        depositCents,
        payment: depositCents ? { method, tenderedCents: tendered ?? undefined } : undefined,
        requestId: requestId.current,
        clockOk,
        print: print && !noPrinter,
      });
      report(r);
      onSaved(r.repair);
    } catch (e) {
      if (e instanceof ApiError && e.code === 'E_CLOCK') setClock({ last: e.detail, print });
      else toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const invalid = tr('Importe no válido', 'Invalid amount');
  const footer = repair ? (
    <>
      <button className="btn btn-ghost" onClick={onClose}>
        {tr('Cancelar', 'Cancel')}
      </button>
      <button className="btn btn-primary" onClick={() => save(false)} disabled={!valid || busy || !dirty}>
        {tr('Guardar cambios', 'Save changes')}
      </button>
    </>
  ) : (
    <>
      <span className="muted small form-note">
        {noPrinter
          ? tr('Sin impresora: el resguardo se imprime después desde aquí.', 'No printer: the receipt can be printed later from here.')
          : depositCents
            ? tr('Imprime el ticket del anticipo, el resguardo y la etiqueta.', 'Prints the deposit ticket, the receipt and the tag.')
            : tr('Imprime el resguardo y la etiqueta.', 'Prints the receipt and the tag.')}
      </span>
      <div className="row-gap">
        {!noPrinter && (
          <button className="btn btn-ghost" onClick={() => save(false)} disabled={!valid || busy}>
            {tr('Guardar sin imprimir', 'Save without printing')}
          </button>
        )}
        <button className="btn btn-primary" onClick={() => save(true)} disabled={!valid || busy}>
          {noPrinter ? <Plus size={16} /> : <Printer size={16} />}
          {noPrinter ? tr('Guardar', 'Save') : tr('Guardar e imprimir', 'Save and print')}
        </button>
      </div>
    </>
  );

  return (
    <Drawer wide title={repair ? `${tr('Editar', 'Edit')} ${repair.number}` : tr('Nueva reparación', 'New repair')} onClose={onClose} dirty={dirty} footer={footer}>
      <div className="repair-form">
        <div className="stack">
          <div className="grid2">
            <Field label={tr('Cliente', 'Customer')}>
              <input autoFocus value={d.customerName} onChange={(e) => set('customerName', e.target.value)} maxLength={120} />
            </Field>
            <Field label={tr('Teléfono', 'Phone')}>
              <input value={d.customerPhone} onChange={(e) => set('customerPhone', e.target.value)} inputMode="tel" maxLength={30} />
            </Field>
          </div>
          <div className="grid3">
            <Field label={tr('Marca', 'Make')}>
              <input value={d.brand} onChange={(e) => set('brand', e.target.value)} maxLength={40} placeholder="Samsung" />
            </Field>
            <Field label={tr('Modelo', 'Model')}>
              <input value={d.model} onChange={(e) => set('model', e.target.value)} maxLength={60} placeholder="Galaxy A54" />
            </Field>
            <Field label={tr('Color', 'Colour')}>
              <input value={d.colour} onChange={(e) => set('colour', e.target.value)} maxLength={30} />
            </Field>
          </div>
          <Field
            label={tr('IMEI o n.º de serie', 'IMEI or serial number')}
            hint={tr('*#06# en el teclado lo muestra', '*#06# on the keypad shows it')}
            error={imeiLooksWrong(d.imei) ? tr('Revisa el IMEI: el último dígito no cuadra', 'Check the IMEI: the last digit does not match') : undefined}
          >
            <input className="mono" value={d.imei} onChange={(e) => set('imei', e.target.value)} maxLength={40} spellCheck={false} />
          </Field>
          <Field label={tr('Avería', 'Fault')} hint={tr('Como la cuenta el cliente. Sale en el resguardo.', 'As the customer tells it. Printed on the receipt.')}>
            <textarea rows={3} value={d.fault} onChange={(e) => set('fault', e.target.value)} maxLength={500} />
          </Field>
          <Field label={tr('Notas internas', 'Internal notes')} hint={tr('Solo para la tienda: no se imprimen.', 'For the shop only: never printed.')}>
            <textarea rows={2} value={d.notes} onChange={(e) => set('notes', e.target.value)} maxLength={1000} />
          </Field>
          <div className="grid2">
            <Field
              label={tr('Presupuesto', 'Estimate')}
              hint={tr('Vacío si aún no hay precio', 'Empty if there is no price yet')}
              error={badEstimate ? invalid : undefined}
            >
              <div className="money-input">
                <input value={d.estimate} onChange={(e) => set('estimate', e.target.value)} inputMode="decimal" placeholder="0,00" />
                <span>€</span>
              </div>
            </Field>
            {!repair && (
              <Field
                label={tr('Anticipo', 'Deposit')}
                hint={tr('Lo que paga ahora. Sale un ticket.', 'What is paid now. It prints a ticket.')}
                error={
                  badDeposit
                    ? invalid
                    : depositHigh
                      ? tr('Es más que el presupuesto', 'It is more than the estimate')
                      : depositOver
                        ? tr('Un anticipo no puede pasar de 400 €', 'A deposit cannot be over 400 EUR')
                        : undefined
                }
              >
                <div className="money-input">
                  <input value={d.deposit} onChange={(e) => set('deposit', e.target.value)} inputMode="decimal" placeholder="0,00" />
                  <span>€</span>
                </div>
              </Field>
            )}
          </div>
          {!repair && depositCents > 0 && (
            <div className="repair-pay">
              <Segmented
                value={method}
                onChange={setMethod}
                options={[
                  { value: 'cash', label: tr('Efectivo', 'Cash') },
                  { value: 'card', label: tr('Tarjeta', 'Card') },
                  { value: 'bizum', label: 'Bizum' },
                ]}
              />
              {method === 'cash' && (
                <Field
                  label={tr('Entregado por el cliente', 'Received from the customer')}
                  error={badGiven ? invalid : short ? tr('No cubre el anticipo', 'It does not cover the deposit') : undefined}
                  hint={tendered !== null && !short ? `${tr('Cambio', 'Change')} ${eur(tendered - depositCents)}` : tr('Vacío si es justo', 'Empty if exact')}
                >
                  <div className="money-input">
                    <input value={given} onChange={(e) => setGiven(e.target.value)} inputMode="decimal" placeholder={fmt(depositCents)} />
                    <span>€</span>
                  </div>
                </Field>
              )}
            </div>
          )}
          {repair?.deposit && (
            <p className="muted small">
              {tr('Anticipo cobrado', 'Deposit paid')}: <b>{eur(repair.depositCents)}</b> <span className="mono">({repair.deposit.number})</span>
            </p>
          )}
        </div>
        <div className="repair-paper">
          <Segmented
            value={show}
            onChange={setShow}
            options={[
              { value: 'resguardo', label: tr('Resguardo', 'Receipt') },
              { value: 'slip', label: tr('Etiqueta', 'Tag') },
            ]}
          />
          {paper && <Paper lines={show === 'resguardo' ? paper.resguardo : paper.slip} logo={show === 'resguardo' ? logo : null} cols={settings.columns} />}
        </div>
      </div>
      {clock && (
        <ClockModal
          last={clock.last}
          onCancel={() => setClock(null)}
          onConfirm={() => {
            const p = clock.print;
            setClock(null);
            save(p, true);
          }}
        />
      )}
    </Drawer>
  );
}

// The price is settled here; whatever the deposit does not cover is charged
// in Venta, so it is a normal ticket with the repair on its line.
function DeliverModal({ repair, settings, onClose, onCharge, onDone }: {
  repair: Repair;
  settings: Settings;
  onClose: () => void;
  onCharge: (line: ItemDraft, number: string) => void;
  onDone: (d: RepairDetail) => void;
}) {
  const toast = useToast();
  const [value, setValue] = useState(repair.estimateCents === null ? '' : fmt(repair.estimateCents));
  const [busy, setBusy] = useState(false);
  const cents = value.trim() ? parseMoney(value) : null;
  const bad = value.trim() !== '' && (cents === null || cents < 0);
  const due = cents === null || bad ? null : cents - repair.depositCents;

  const go = async () => {
    if (due === null || due < 0 || busy) return;
    setBusy(true);
    try {
      let r = repair;
      if (cents !== repair.estimateCents) r = (await call<RepairDetail>('repairs.update', { id: repair.id, ...dataOf(repair), estimateCents: cents })).repair;
      if (due === 0) {
        const d = await call<RepairDetail>('repairs.deliver', { id: repair.id });
        toast.ok(`${repair.number} ${tr('entregado', 'handed back')}`);
        onDone(d);
      } else {
        onCharge({ name: r.lineName, qty: 1, unitCents: due, vatBp: settings.defaultVatBp, repairId: r.id }, r.number);
      }
    } catch (e) {
      toast.error(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`${tr('Entregar', 'Hand back')} ${repair.number}`}
      onClose={onClose}
      actions={
        <>
          <button className="btn btn-ghost" onClick={onClose}>
            {tr('Cancelar', 'Cancel')}
          </button>
          <button className="btn btn-primary" onClick={go} disabled={due === null || due < 0 || busy}>
            {due === 0 ? <PackageCheck size={16} /> : <HandCoins size={16} />}
            {due === 0 ? tr('Entregar', 'Hand back') : tr('Cobrar en Venta', 'Charge in Sale')}
          </button>
        </>
      }
    >
      <p className="muted">
        {repair.customerName} · {device(repair)}
      </p>
      <Field label={tr('Precio final', 'Final price')} hint={tr('El presupuesto, o lo que se haya acordado. 0 si no se cobra nada.', 'The estimate, or whatever was agreed. 0 if nothing is charged.')} error={bad ? tr('Importe no válido', 'Invalid amount') : undefined}>
        <div className="money-input big">
          <input
            autoFocus
            value={value}
            onChange={(e) => setValue(e.target.value)}
            inputMode="decimal"
            placeholder="0,00"
            onFocus={(e) => e.target.select()}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                go();
              }
            }}
          />
          <span>€</span>
        </div>
      </Field>
      <div className="deliver-sum">
        {repair.depositCents > 0 && (
          <div>
            <span>
              {tr('Anticipo ya cobrado', 'Deposit already paid')} <span className="mono">({repair.deposit?.number})</span>
            </span>
            <b>-{eur(repair.depositCents)}</b>
          </div>
        )}
        <div className="due">
          <span>{tr('A cobrar', 'To charge')}</span>
          <strong>{due === null ? '…' : eur(Math.max(0, due))}</strong>
        </div>
      </div>
      {due !== null && due < 0 && <p className="form-error">{errorText(new ApiError('E_REPAIR_DEPOSIT_HIGH', ''))}</p>}
      {due !== null && due > 40_000 && (
        <p className="muted small">{tr('Más de 400 €: al cobrar se piden los datos del cliente para la factura.', 'Over 400 EUR: the customer details are asked for the invoice when charging.')}</p>
      )}
    </Modal>
  );
}

interface Props {
  user: User;
  settings: Settings;
  logo: Logo | null;
  onSetupPrinter?: () => void;
  // "Entregar y cobrar": the line goes to Venta
  onCharge: (line: ItemDraft, number: string) => void;
}

export function Repairs({ user, settings, logo, onSetupPrinter, onCharge }: Props) {
  const toast = useToast();
  const [filter, setFilter] = useState<Filter>('open');
  const [q, setQ] = useState('');
  const [list, setList] = useState<RepairSummary[] | null>(null);
  const [selId, setSelId] = useState<number | null>(null);
  const [detail, setDetail] = useState<RepairDetail | null>(null);
  const [form, setForm] = useState<'new' | Repair | null>(null);
  const [delivering, setDelivering] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [reason, setReason] = useState('');
  const [show, setShow] = useState<'resguardo' | 'slip'>('resguardo');
  const [busy, setBusy] = useState(false);
  const admin = user.role === 'admin';

  // `keep` stays on screen after it was handed back or cancelled, even when
  // the filter no longer lists it
  const load = useCallback(
    (keep?: number) => {
      call<RepairSummary[]>('repairs.list', { status: filter, q: q.trim() })
        .then((l) => {
          setList(l);
          setSelId((id) => keep ?? (id !== null && l.some((r) => r.id === id) ? id : l[0]?.id ?? null));
        })
        .catch((e) => toast.error(errorText(e)));
    },
    [filter, q, toast],
  );

  useEffect(() => {
    const t = setTimeout(() => load(), q ? 200 : 0);
    return () => clearTimeout(t);
  }, [load, q]);

  useEffect(() => {
    if (selId === null) {
      setDetail(null);
      return;
    }
    let live = true;
    call<RepairDetail>('repairs.get', { id: selId })
      .then((d) => live && setDetail(d))
      .catch((e) => live && toast.error(errorText(e)));
    return () => {
      live = false;
    };
  }, [selId, toast]);

  const move = (step: number) => {
    if (!list?.length) return;
    const i = list.findIndex((r) => r.id === selId);
    setSelId(list[Math.min(list.length - 1, Math.max(0, i + step))].id);
  };

  // a repair just saved is shown, even when the filter would hide it
  const saved = (r: Repair) => {
    setForm(null);
    if (filter !== 'all' && filter !== 'open' && filter !== r.status) setFilter('open');
    setSelId(r.id);
    call<RepairDetail>('repairs.get', { id: r.id })
      .then(setDetail)
      .catch(() => {});
    load(r.id);
  };

  const run = async (cmd: string, args: object, done?: string) => {
    if (busy) return;
    setBusy(true);
    try {
      const d = await call<RepairDetail>(cmd, args);
      setDetail(d);
      if (done) toast.ok(done);
      load(d.repair.id);
      return d;
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const print = async (what: 'resguardo' | 'slip') => {
    if (!detail || busy) return;
    const id = detail.repair.id;
    setBusy(true);
    try {
      const r = await call<{ warning: string | null }>('repairs.print', { id, what });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Impreso', 'Printed'));
    } catch (e) {
      if (isTransient(e)) toast.error(errorText(e), { action: { label: tr('Reintentar', 'Retry'), run: () => print(what) } });
      else if (isPrinterSetup(e) && onSetupPrinter) toast.error(errorText(e), { action: { label: tr('Configurar', 'Set up'), run: onSetupPrinter } });
      else toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const doCancel = async () => {
    if (!detail || !reason.trim()) return;
    const d = await run('repairs.cancel', { id: detail.repair.id, reason }, `${detail.repair.number} ${tr('cancelado', 'cancelled')}`);
    if (d) {
      setCancelling(false);
      setReason('');
    }
  };

  const r = detail && detail.repair.id === selId ? detail.repair : undefined;
  const open = !!r && OPEN.includes(r.status);
  const n = list?.length ?? 0;

  return (
    <div className="split">
      <section className="list-pane">
        <div className="pane-head">
          <button className="btn btn-primary block" onClick={() => setForm('new')}>
            <Plus size={16} />
            {tr('Nueva reparación', 'New repair')}
          </button>
          <div className="chips">
            {FILTERS.map((f) => (
              <button key={f.id} className={`chip${filter === f.id ? ' on' : ''}`} onClick={() => setFilter(f.id)}>
                {f.label()}
              </button>
            ))}
          </div>
          <div className="search-sm">
            <Search size={16} />
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={tr('Nº, nombre, teléfono o IMEI', 'Number, name, phone or IMEI')} />
          </div>
          <div className="pane-sub">
            <span>
              {n} {n === 1 ? tr('reparación', 'repair') : tr('reparaciones', 'repairs')}
            </span>
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
            <button key={s.id} className={`ticket-row${selId === s.id ? ' on' : ''}${s.status === 'cancelled' ? ' voided' : ''}`} onClick={() => setSelId(s.id)}>
              <div className="ticket-row-main">
                <b className="mono">{s.number}</b>
                <span>
                  {s.customerName} · {s.device}
                </span>
                <span>{dateTime(s.createdAt)}</span>
              </div>
              <div className="ticket-row-side">
                <StatusBadge s={s.status} />
                {s.estimateCents !== null && <b>{eur(s.estimateCents)}</b>}
              </div>
            </button>
          ))}
          {list && list.length === 0 && (
            <Empty
              icon={<Wrench size={28} />}
              title={tr('No hay reparaciones', 'No repairs')}
              text={q ? tr('Prueba con otro número, nombre o teléfono.', 'Try another number, name or phone.') : tr('Los equipos que dejen los clientes aparecerán aquí.', 'Devices customers leave will show up here.')}
            />
          )}
        </div>
      </section>

      <section className="detail-pane">
        {r && detail ? (
          <>
            <div className="detail-head">
              <div>
                <h2 className="mono">
                  {r.number}
                  <StatusBadge s={r.status} />
                </h2>
                <p className="ref-note">
                  <b>{r.customerName}</b> · {r.customerPhone}
                </p>
                <p className="muted">
                  {device(r)}
                  {r.imei && (
                    <>
                      {' · '}
                      <span className="mono">{r.imei}</span>
                    </>
                  )}
                </p>
                <p className="ref-note">
                  {tr('Presupuesto', 'Estimate')} <b>{r.estimateCents === null ? tr('pendiente', 'pending') : eur(r.estimateCents)}</b>
                  {r.deposit && (
                    <>
                      {' · '}
                      {tr('Anticipo', 'Deposit')} <b className={r.depositCents === 0 ? 'struck' : ''}>{eur(r.deposit.totalCents)}</b>{' '}
                      <span className="mono">({r.deposit.number})</span>
                    </>
                  )}
                  {open && r.deposit && r.dueCents !== null && (
                    <>
                      {' · '}
                      {tr('Pendiente', 'Left to pay')} <b>{eur(Math.max(0, r.dueCents))}</b>
                    </>
                  )}
                </p>
                <p className="print-note">
                  {tr('Recibido el', 'Taken in on')} {dateTime(r.createdAt)} {tr('por', 'by')} {r.createdBy}
                </p>
                {r.status === 'delivered' && r.closedAt && (
                  <p className="print-note">
                    {tr('Entregado el', 'Handed back on')} {dateTime(r.closedAt)} {tr('por', 'by')} {r.closedBy}
                    {r.ticket && (
                      <>
                        {' · '}
                        <span className="mono">{r.ticket.number}</span> ({eur(r.ticket.totalCents)})
                      </>
                    )}
                  </p>
                )}
                {r.status === 'cancelled' && (
                  <p className="void-note">
                    {tr('Cancelado el', 'Cancelled on')} {dateTime(r.closedAt ?? '')} {tr('por', 'by')} {r.closedBy}: {r.cancelReason}
                  </p>
                )}
              </div>
              <div className="actions wrap">
                {open && (
                  <button className="btn btn-primary" onClick={() => setDelivering(true)} disabled={busy}>
                    <HandCoins size={16} />
                    {tr('Entregar y cobrar', 'Hand back and charge')}
                  </button>
                )}
                <button className="btn" onClick={() => print('resguardo')} disabled={busy}>
                  <Printer size={16} />
                  {tr('Resguardo', 'Receipt')}
                </button>
                <button className="btn" onClick={() => print('slip')} disabled={busy} title={tr('La etiqueta que va con el equipo', 'The tag that goes with the device')}>
                  <Tag size={16} />
                  {tr('Etiqueta', 'Tag')}
                </button>
                {open && (
                  <button className="btn" onClick={() => setForm(r)} disabled={busy}>
                    <Pencil size={16} />
                    {tr('Editar', 'Edit')}
                  </button>
                )}
                {admin && open && (
                  <button className="btn btn-danger-ghost" onClick={() => setCancelling(true)} disabled={busy}>
                    <Ban size={16} />
                    {tr('Cancelar', 'Cancel')}
                  </button>
                )}
              </div>
            </div>
            <div className="repair-body">
              <div className="repair-info">
                {open && (
                  <Field label={tr('Estado', 'Status')}>
                    <Segmented
                      value={r.status}
                      onChange={(s) => run('repairs.status', { id: r.id, status: s })}
                      options={OPEN.map((s) => ({ value: s, label: statusText(s) }))}
                    />
                  </Field>
                )}
                <div className="info-block">
                  <span className="field-label">{tr('Avería', 'Fault')}</span>
                  <p className="pre">{r.fault}</p>
                </div>
                {r.notes && (
                  <div className="info-block">
                    <span className="field-label">{tr('Notas internas', 'Internal notes')}</span>
                    <p className="pre">{r.notes}</p>
                  </div>
                )}
                <div className="info-block">
                  <span className="field-label">{tr('Historial', 'History')}</span>
                  <ol className="history">
                    {[...r.events].reverse().map((e, i) => (
                      <li key={i}>
                        <span className="history-at">
                          {dateTime(e.at)} · {e.byName}
                        </span>
                        <span>{eventText(e)}</span>
                      </li>
                    ))}
                  </ol>
                </div>
              </div>
              <div className="repair-paper">
                <Segmented
                  value={show}
                  onChange={setShow}
                  options={[
                    { value: 'resguardo', label: tr('Resguardo', 'Receipt') },
                    { value: 'slip', label: tr('Etiqueta', 'Tag') },
                  ]}
                />
                <Paper
                  lines={show === 'resguardo' ? detail.resguardo : detail.slip}
                  logo={show === 'resguardo' ? detail.logo : null}
                  cols={settings.columns}
                  stamp={r.status === 'cancelled' ? tr('CANCELADO', 'CANCELLED') : undefined}
                />
              </div>
            </div>
          </>
        ) : (
          <Empty icon={<Wrench size={28} />} title={list?.length ? tr('Elige una reparación', 'Pick a repair') : tr('Ninguna reparación aquí', 'No repair here')} />
        )}
      </section>

      {form && (
        <RepairForm
          repair={form === 'new' ? null : form}
          settings={settings}
          logo={form === 'new' ? logo : detail?.logo ?? logo}
          onClose={() => setForm(null)}
          onSaved={saved}
          onSetupPrinter={onSetupPrinter}
        />
      )}

      {delivering && r && (
        <DeliverModal
          repair={r}
          settings={settings}
          onClose={() => setDelivering(false)}
          onCharge={(line, number) => {
            setDelivering(false);
            onCharge(line, number);
          }}
          onDone={(d) => {
            setDelivering(false);
            setDetail(d);
            load(d.repair.id);
          }}
        />
      )}

      {cancelling && r && (
        <Modal
          title={`${tr('Cancelar', 'Cancel')} ${r.number}`}
          onClose={() => setCancelling(false)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setCancelling(false)}>
                {tr('Volver', 'Back')}
              </button>
              <button className="btn btn-danger" onClick={doCancel} disabled={busy || !reason.trim() || r.depositCents > 0}>
                <Ban size={16} />
                {tr('Cancelar reparación', 'Cancel repair')}
              </button>
            </>
          }
        >
          {r.depositCents > 0 ? (
            <p className="form-error">{errorText(new ApiError('E_REPAIR_DEPOSIT', ''))}</p>
          ) : (
            <>
              <p className="muted">
                {tr(
                  'Para una reparación registrada por error. No se borra: queda guardada como cancelada, con el motivo. Esto no se puede deshacer.',
                  'For a repair taken in by mistake. It is not deleted: it stays on record as cancelled, with the reason. This cannot be undone.',
                )}
              </p>
              <Field label={tr('Motivo', 'Reason')}>
                <input
                  autoFocus
                  value={reason}
                  onChange={(e) => setReason(e.target.value)}
                  maxLength={200}
                  placeholder={tr('Duplicada, el cliente no lo dejó…', 'Duplicate, the customer did not leave it…')}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      doCancel();
                    }
                  }}
                />
              </Field>
            </>
          )}
        </Modal>
      )}
    </div>
  );
}
