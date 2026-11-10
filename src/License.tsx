import { useCallback, useEffect, useState } from 'react';
import { RefreshCw, ShieldAlert } from 'lucide-react';
import { call } from './api';
import { dateTime } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';

// What the control server says about this copy of Tiquet: locked, or not
// heard from for too long, it can look at everything but charges nothing;
// close to that it warns; it may also carry a message for the shop and ask
// for a newer version.
export interface LicenseState {
  server: boolean;
  status: 'ok' | 'locked' | 'lapsed' | 'new';
  message: string | null;
  blocked: boolean;
  warn: boolean;
  daysLeft: number | null;
  minVersion: string | null;
  updateNow: boolean;
  forceUpdate: boolean;
  device: string;
  checked: string | null;
  error?: string;
}

const EVERY = 5 * 60_000;

export function useLicense(on: boolean) {
  const [state, setState] = useState<LicenseState | null>(null);
  const [checking, setChecking] = useState(false);
  const toast = useToast();

  const load = useCallback(() => {
    call<LicenseState>('license.state')
      .then(setState)
      .catch(() => {});
  }, []);

  useEffect(() => {
    if (!on) return;
    load();
    const t = window.setInterval(load, EVERY);
    return () => window.clearInterval(t);
  }, [on, load]);

  const check = useCallback(async () => {
    setChecking(true);
    try {
      const s = await call<LicenseState>('license.check');
      setState(s);
      if (s.error) toast.error(tr('No se ha podido llegar al servidor. Revisa que este PC tenga internet.', 'The server could not be reached. Check this PC has internet.'));
      else if (!s.blocked) toast.ok(tr('Comprobado: todo en orden', 'Checked: all in order'));
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setChecking(false);
    }
  }, [toast]);

  return { state, checking, check, reload: load };
}

export function LicenseBanner({ l }: { l: ReturnType<typeof useLicense> }) {
  const s = l.state;
  if (!s || !s.server || (!s.blocked && !s.warn && !s.message)) return null;
  const text = s.blocked
    ? s.status === 'locked'
      ? tr('Tiquet está bloqueado en este PC: se puede ver todo, pero no cobrar.', 'Tiquet is locked on this PC: everything can be seen, but nothing charged.')
      : s.status === 'lapsed'
        ? tr('Este PC lleva demasiado tiempo sin conectarse. Conéctalo a internet y pulsa Comprobar para seguir cobrando.', 'This PC has been offline for too long. Connect it to the internet and press Check to keep charging.')
        : tr('Este PC aún no ha podido comprobar su licencia. Conéctalo a internet y pulsa Comprobar.', 'This PC has not been able to check its licence yet. Connect it to the internet and press Check.')
    : s.warn
      ? tr(
          `Conecta este PC a internet un momento: le quedan ${s.daysLeft ?? 0} días para comprobar su licencia.`,
          `Connect this PC to the internet for a moment: ${s.daysLeft ?? 0} days left to check its licence.`,
        )
      : '';
  return (
    <div className={`license-bar${s.blocked ? ' blocked' : s.warn ? ' warn' : ''}`} role={s.blocked ? 'alert' : 'status'}>
      <ShieldAlert size={18} />
      <span>
        {text}
        {s.message && <b> {s.message}</b>}
        {s.checked && (s.blocked || s.warn) && (
          <small>
            {' '}
            · {tr('última comprobación', 'last checked')} {dateTime(s.checked)}
          </small>
        )}
      </span>
      {(s.blocked || s.warn) && (
        <button className="btn sm" onClick={l.check} disabled={l.checking}>
          <RefreshCw size={14} className={l.checking ? 'lan-spin' : undefined} />
          {tr('Comprobar', 'Check')}
        </button>
      )}
    </div>
  );
}
