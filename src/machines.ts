import { tr } from './i18n';
import type { Method, Payment, Settings } from './types';

// those still in the shop; one taken away only keeps its name on what it did
export const live = (s: Settings) => s.machines.filter((m) => !m.removed);
export const cardMachines = (s: Settings) => live(s).filter((m) => m.card);
export const rechargeMachines = (s: Settings) => live(s).filter((m) => m.recharge);

export function machineName(s: Settings, id: number | null | undefined): string {
  return s.machines.find((m) => m.id === id)?.name ?? tr('Sin máquina', 'No machine');
}

export interface PayOption {
  value: string;
  label: string;
  method: Method;
  machine: number | null;
}

export const payValue = (p: { method: Method; machine?: number | null }) => (p.method === 'card' && p.machine ? `card:${p.machine}` : p.method);

const cashOption = (): PayOption => ({ value: 'cash', label: tr('Efectivo', 'Cash'), method: 'cash', machine: null });
const bizumOption = (): PayOption => ({ value: 'bizum', label: 'Bizum', method: 'bizum', machine: null });

// How a sale can be said to have been paid. One machine stays "Tarjeta", also
// for a card from before there were machines; with several each is a choice of
// its own. What the ticket has now is always there.
export function payOptions(s: Settings, now?: Payment): PayOption[] {
  const cards = cardMachines(s);
  const out = [cashOption()];
  if (cards.length > 1) {
    for (const m of cards) out.push({ value: `card:${m.id}`, label: m.name, method: 'card', machine: m.id });
  } else {
    const one = cards[0]?.id ?? null;
    const old = now?.method === 'card' && !now.machine;
    out.push({ value: one && !old ? `card:${one}` : 'card', label: tr('Tarjeta', 'Card'), method: 'card', machine: one });
  }
  if (now?.method === 'card' && !out.some((o) => o.value === payValue(now))) {
    out.push({ value: payValue(now), label: now.machine ? machineName(s, now.machine) : tr('Tarjeta', 'Card'), method: 'card', machine: now.machine ?? null });
  }
  if (s.payBizum || now?.method === 'bizum') out.push(bizumOption());
  return out;
}
