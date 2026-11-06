import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Download } from 'lucide-react';
import { call, native } from './api';
import { errorText, tr } from './i18n';
import { useToast } from './toast';

// how often GitHub is asked, and how long nobody has to touch the till before
// a new version installs itself
const EVERY = 15 * 60_000;
const QUIET = 2 * 60_000;
// With a ticket on the screen it waits longer, for someone who only stepped
// away. A ticket forgotten there must not stop updates: it is saved, and it
// comes back when Tiquet opens again.
const LEFT = 10 * 60_000;
// Installing closes Tiquet, and only the installer opens it again. If an
// antivirus stopped the installer Tiquet would just vanish, so a version gets
// two tries alone; after that it waits for someone to press the button.
const TRIES = 2;
const TRIES_KEY = 'tiquet.updateTries';

function tries(version: string | null): number {
  try {
    const t = JSON.parse(localStorage.getItem(TRIES_KEY) ?? 'null');
    return t?.version === version ? t.n : 0;
  } catch {
    return 0;
  }
}

function tried(version: string) {
  try {
    localStorage.setItem(TRIES_KEY, JSON.stringify({ version, n: tries(version) + 1 }));
  } catch {
    // with no memory of it the count starts again
  }
}

export type Updates = ReturnType<typeof useUpdate>;

interface Opts {
  // off while another screen looks after it
  on?: boolean;
  // new versions come down and install themselves
  auto?: boolean;
  quiet?: number;
  // something that closing Tiquet would lose: it waits for as long as it lasts
  hold?: boolean;
  // the last thing to do before Tiquet closes to install
  before?: () => Promise<unknown>;
}

// Asks GitHub for a newer Tiquet a few seconds after start and every quarter
// of an hour, whatever is on the screen: asking and downloading get in nobody's
// way. With `auto` the new version runs once nobody has touched the till for a
// while and no window is open; a ticket on the screen only makes that wait
// longer. Installing closes Tiquet; the installer runs silently and opens the
// new version.
export function useUpdate(ticket: boolean, { on = true, auto = false, quiet = QUIET, hold = false, before }: Opts = {}) {
  const toast = useToast();
  const [found, setFound] = useState<string | null>(null);
  const [later, setLater] = useState(false);
  const [checking, setChecking] = useState(false);
  const [upToDate, setUpToDate] = useState(false);
  const [failed, setFailed] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [due, setDue] = useState(false);
  // the version downloaded and waiting, and how many checks have been made
  const [ready, setReady] = useState<string | null>(null);
  const [round, setRound] = useState(0);
  const running = useRef(false);
  const fetching = useRef(false);
  const seen = useRef(Date.now());
  const ticketNow = useRef(ticket);
  ticketNow.current = ticket;
  const holdNow = useRef(hold);
  holdNow.current = hold;
  const beforeNow = useRef(before);
  beforeNow.current = before;

  const check = useCallback(
    async (asked = false) => {
      setChecking(true);
      try {
        const r = await call<{ version: string } | null>('app.update.check');
        setFound(r?.version ?? null);
        setUpToDate(!r);
        setLater(false);
        setFailed(false);
        setRound((n) => n + 1);
      } catch (e) {
        // a shop without internet is not told about it every time
        setFailed(true);
        if (asked) toast.error(errorText(e));
      } finally {
        setChecking(false);
      }
    },
    [toast],
  );

  useEffect(() => {
    if (!native || !on) return;
    const first = setTimeout(() => setDue(true), 5000);
    const again = setInterval(() => setDue(true), EVERY);
    return () => {
      clearTimeout(first);
      clearInterval(again);
    };
  }, [on]);

  useEffect(() => {
    if (!due) return;
    setDue(false);
    check();
  }, [due, check]);

  const run = useCallback(
    async (alone: boolean) => {
      if (running.current) return;
      running.current = true;
      setInstalling(true);
      setProgress(null);
      let off: (() => void) | undefined;
      try {
        off = await listen<{ got: number; total: number | null }>('tiquet://update', (e) => {
          const { got, total } = e.payload;
          setProgress(total ? got / total : null);
        });
        await beforeNow.current?.().catch(() => {});
        // an update that installs never answers: Tiquet closes here
        await call('app.update.install');
        setFound(null);
        setUpToDate(true);
      } catch (e) {
        // one that went wrong on its own is tried again after the next check
        setReady(null);
        if (!alone) toast.error(errorText(e));
      } finally {
        off?.();
        running.current = false;
        setInstalling(false);
        setProgress(null);
      }
    },
    [toast],
  );

  const install = useCallback(() => run(false), [run]);

  // the installer comes down while the shop works; no internet right now
  // means the next check tries again
  useEffect(() => {
    if (!on || !auto || !found || ready === found || fetching.current) return;
    fetching.current = true;
    call<{ version: string } | null>('app.update.fetch')
      .then((r) => setReady(r?.version ?? null))
      .catch(() => {})
      .finally(() => {
        fetching.current = false;
      });
  }, [on, auto, found, ready, round]);

  // it runs once the till is free
  const alone = auto && tries(found) < TRIES;
  const waiting = on && alone && !!found && ready === found;
  useEffect(() => {
    if (!waiting) return;
    seen.current = Date.now();
    const touched = () => {
      seen.current = Date.now();
    };
    const kinds = ['keydown', 'pointerdown', 'wheel'];
    kinds.forEach((k) => window.addEventListener(k, touched, true));
    const t = setInterval(() => {
      if (holdNow.current || document.querySelector('.modal-back, .pay-back')) return;
      if (Date.now() - seen.current < (ticketNow.current ? LEFT : quiet)) return;
      if (found) tried(found);
      run(true);
    }, 5000);
    return () => {
      kinds.forEach((k) => window.removeEventListener(k, touched, true));
      clearInterval(t);
    };
  }, [waiting, quiet, found, run]);

  return { found, auto: alone, later, checking, upToDate, failed, installing, progress, check, install, hide: () => setLater(true) };
}

export function UpdateBar({ u }: { u: Updates }) {
  if (!u.found || (!u.installing && u.later)) return null;
  return (
    <div className="update-bar" role="status">
      <Download size={16} />
      {u.installing ? (
        <span>
          {u.progress !== null
            ? `${tr('Descargando la versión', 'Downloading version')} ${u.found} · ${Math.round(u.progress * 100)} %. `
            : `${tr('Instalando la versión', 'Installing version')} ${u.found}. `}
          {tr('Tiquet se cerrará y volverá a abrirse solo.', 'Tiquet will close and open again by itself.')}
        </span>
      ) : (
        <>
          <span>
            {tr(`Hay una versión nueva (${u.found}).`, `There is a new version (${u.found}).`)}
            {u.auto && ` ${tr('Se instala sola cuando la caja esté libre.', 'It installs itself when the till is free.')}`}
          </span>
          <button className="btn btn-primary sm" onClick={u.install}>
            {tr('Actualizar ahora', 'Update now')}
          </button>
          {!u.auto && (
            <button className="btn btn-ghost sm" onClick={u.hide}>
              {tr('Más tarde', 'Later')}
            </button>
          )}
        </>
      )}
    </div>
  );
}
