import { useCallback, useEffect, useRef, useState } from 'react';
import { call } from './api';
import type { PrinterStatus } from './types';

// The spooler is asked every few seconds while the till is on screen, so a
// printer that was unplugged or ran out of paper shows before the next sale.
export function usePrinterStatus(active: boolean, printerName: string) {
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
  }, [active, printerName, refresh]);

  return { status, refresh };
}
