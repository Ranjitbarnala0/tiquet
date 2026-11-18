import { useCallback, useEffect, useRef, useState } from 'react';
import { call } from './api';
import { tr } from './i18n';
import type { PrinterEntry } from './types';

// A printer chosen for one print only: a receipt printer gets the ticket as
// it is ("windows"); any other printer gets it drawn as a page ("document").
export interface OneOff {
  name: string;
  kind: 'windows' | 'document';
  // a receipt printer's characters per line, when Windows knows its paper
  columns?: number;
}

interface Choices {
  printers: PrinterEntry[];
  receipt: string;
  a4: string;
}

// This PC's printers and the ones set up in Ajustes, asked each time a pick
// shows (a printer just chosen in Ajustes is already the usual one) and again
// when its list opens, in case one was plugged in since. A failed answer
// keeps what was there.
export function usePrinterChoices(again?: unknown) {
  const [c, setC] = useState<Choices | null>(null);
  const live = useRef(true);
  const ask = useCallback(() => {
    call<Choices>('printers.choices')
      .then((v) => live.current && setC(v))
      .catch(() => {});
  }, []);
  useEffect(() => {
    live.current = true;
    ask();
    return () => {
      live.current = false;
    };
  }, [ask, again]);
  return { choices: c, refresh: ask };
}

// The printer for this print: the one set up in Ajustes unless another is
// picked. An A4 page goes to page printers only; a ticket to any printer.
// `again` changing (the settings, saved) asks this PC again.
export function PrinterPick({ format, value, onChange, again }: { format: 'a4' | 'ticket'; value: OneOff | null; onChange: (v: OneOff | null) => void; again?: unknown }) {
  const { choices: c, refresh } = usePrinterChoices(again);
  const real = (c?.printers ?? []).filter((p) => !p.virtual);
  const usual = format === 'a4' ? (c?.a4 ?? '') : (c?.receipt ?? '');
  const options = (format === 'a4' ? real.filter((p) => !p.receipt) : real).filter((p) => p.name !== usual);
  // a printer already picked always shows as itself, even one this format
  // would not offer
  const picked = value && value.name !== usual && !options.some((p) => p.name === value.name) ? value.name : '';
  // until this PC answers, the usual one, whatever it is
  const none = !c ? tr('La de siempre', 'The usual one') : format === 'a4' ? tr('Sin impresora A4: sale como ticket', 'No A4 printer: prints as a receipt') : tr('Sin impresora de tickets', 'No receipt printer');
  return (
    <select
      className="printer-pick"
      aria-label={tr('Impresora', 'Printer')}
      value={value?.name ?? ''}
      onFocus={() => refresh()}
      onChange={(e) => {
        const name = e.target.value;
        if (!name) return onChange(null);
        const p = real.find((x) => x.name === name);
        const receipt = format === 'ticket' && !!p?.receipt;
        onChange({ name, kind: receipt ? 'windows' : 'document', columns: (receipt && p?.columns) || undefined });
      }}
    >
      <option value="">{usual ? `${usual} · ${tr('la de siempre', 'the usual one')}` : none}</option>
      {options.map((p) => (
        <option key={p.name} value={p.name}>
          {p.name}
          {p.receipt ? ` · ${tr('tickets', 'receipts')}` : ''}
        </option>
      ))}
      {picked && <option value={picked}>{picked}</option>}
    </select>
  );
}
