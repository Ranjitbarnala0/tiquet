import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Download } from 'lucide-react';
import { call, native } from './api';
import { errorText, tr } from './i18n';
import { useToast } from './toast';

const DAY = 24 * 3600_000;

export type Updates = ReturnType<typeof useUpdate>;

// Asks GitHub for a newer Tiquet a few seconds after start and then once a
// day, never while a sale is in progress: a check that falls due then waits
// until the ticket is cleared. Installing closes Tiquet; the installer runs
// silently and opens the new version.
export function useUpdate(saleBusy: boolean) {
  const toast = useToast();
  const [found, setFound] = useState<string | null>(null);
  const [later, setLater] = useState(false);
  const [checking, setChecking] = useState(false);
  const [upToDate, setUpToDate] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [due, setDue] = useState(false);
  const last = useRef(0);

  const check = useCallback(
    async (asked = false) => {
      setChecking(true);
      try {
        const r = await call<{ version: string } | null>('app.update.check');
        last.current = Date.now();
        setFound(r?.version ?? null);
        setUpToDate(!r);
        setLater(false);
      } catch (e) {
        // a shop without internet is not told about it every day
        if (asked) toast.error(errorText(e));
      } finally {
        setChecking(false);
      }
    },
    [toast],
  );

  useEffect(() => {
    if (!native) return;
    const first = setTimeout(() => setDue(true), 5000);
    const daily = setInterval(() => {
      if (Date.now() - last.current >= DAY) setDue(true);
    }, 3600_000);
    return () => {
      clearTimeout(first);
      clearInterval(daily);
    };
  }, []);

  useEffect(() => {
    if (!due || saleBusy) return;
    setDue(false);
    check();
  }, [due, saleBusy, check]);

  const install = useCallback(async () => {
    if (installing) return;
    setInstalling(true);
    setProgress(null);
    let off: (() => void) | undefined;
    try {
      off = await listen<{ got: number; total: number | null }>('tiquet://update', (e) => {
        const { got, total } = e.payload;
        setProgress(total ? got / total : null);
      });
      // an update that installs never answers: Tiquet closes here
      await call('app.update.install');
      setFound(null);
      setUpToDate(true);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      off?.();
      setInstalling(false);
      setProgress(null);
    }
  }, [installing, toast]);

  return { found, later, checking, upToDate, installing, progress, check, install, hide: () => setLater(true) };
}

export function UpdateBar({ u, saleBusy }: { u: Updates; saleBusy: boolean }) {
  if (!u.found || (!u.installing && (u.later || saleBusy))) return null;
  return (
    <div className="update-bar" role="status">
      <Download size={16} />
      {u.installing ? (
        <span>
          {tr('Descargando la versión', 'Downloading version')} {u.found}
          {u.progress !== null && ` · ${Math.round(u.progress * 100)} %`}.{' '}
          {tr('Tiquet se cerrará y volverá a abrirse solo.', 'Tiquet will close and open again by itself.')}
        </span>
      ) : (
        <>
          <span>{tr(`Hay una versión nueva (${u.found})`, `There is a new version (${u.found})`)}</span>
          <button className="btn btn-primary sm" onClick={u.install}>
            {tr('Actualizar ahora', 'Update now')}
          </button>
          <button className="btn btn-ghost sm" onClick={u.hide}>
            {tr('Más tarde', 'Later')}
          </button>
        </>
      )}
    </div>
  );
}
