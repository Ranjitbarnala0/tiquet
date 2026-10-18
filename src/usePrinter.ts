import { useCallback, useEffect, useRef, useState } from 'react';
import { call } from './api';
import type { PrinterStatus, Settings } from './types';

// what the core prints to, named as it names it; empty when nothing is set up
export function printerLabel(s: Settings): string {
  if (s.printerKind === 'network') return s.printerHost.trim() ? `${s.printerHost.trim()}:${s.printerPort}` : '';
  return s.printerName.trim();
}

// The spooler is asked every few seconds while the till is on screen, so a
// printer that was unplugged or ran out of paper shows before the next sale.
export function usePrinterStatus(active: boolean, printer: string) {
  const [status, setStatus] = useState<PrinterStatus | null>(null);
  const seq = useRef(0);

  const refresh = useCallback(() => {
    const n = ++seq.current;
    call<PrinterStatus>('printers.status')
      .then((s) => n === seq.current && setStatus(s))
      .catch(() => n === seq.current && setStatus(null));
  }, []);

  useEffect(() => {
    if (!active) return;
    refresh();
    const t = setInterval(refresh, 10_000);
    return () => clearInterval(t);
  }, [active, printer, refresh]);

  return { status, refresh };
}
