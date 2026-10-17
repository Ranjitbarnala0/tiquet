import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import { CircleCheck, Info, TriangleAlert, X } from 'lucide-react';

type Kind = 'ok' | 'error' | 'info';

interface Opts {
  action?: { label: string; run: () => void };
  sticky?: boolean;
}

interface Item extends Opts {
  id: number;
  kind: Kind;
  text: string;
}

export interface Toaster {
  ok(text: string, o?: Opts): void;
  error(text: string, o?: Opts): void;
  info(text: string, o?: Opts): void;
}

const Ctx = createContext<Toaster | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Item[]>([]);
  const next = useRef(1);

  const close = useCallback((id: number) => setItems((xs) => xs.filter((x) => x.id !== id)), []);

  const push = useCallback(
    (kind: Kind, text: string, o: Opts = {}) => {
      const id = next.current++;
      setItems((xs) => [...xs.slice(-3), { id, kind, text, ...o }]);
      if (!o.sticky) setTimeout(() => close(id), kind === 'error' ? 8000 : o.action ? 6000 : 3500);
    },
    [close],
  );

  const api = useMemo<Toaster>(
    () => ({
      ok: (t, o) => push('ok', t, o),
      error: (t, o) => push('error', t, o),
      info: (t, o) => push('info', t, o),
    }),
    [push],
  );

  return (
    <Ctx.Provider value={api}>
      {children}
      <div className="toasts" role="status" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className={`toast ${t.kind}`}>
            {t.kind === 'ok' ? <CircleCheck size={18} /> : t.kind === 'error' ? <TriangleAlert size={18} /> : <Info size={18} />}
            <span className="toast-text">{t.text}</span>
            {t.action && (
              <button
                className="toast-action"
                onClick={() => {
                  close(t.id);
                  t.action!.run();
                }}
              >
                {t.action.label}
              </button>
            )}
            <button className="toast-x" aria-label="Cerrar" onClick={() => close(t.id)}>
              <X size={15} />
            </button>
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export function useToast(): Toaster {
  const c = useContext(Ctx);
  if (!c) throw new Error('ToastProvider missing');
  return c;
}
