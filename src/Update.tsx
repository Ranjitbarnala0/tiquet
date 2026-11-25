import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Download } from 'lucide-react';
import { call, native } from './api';
import { errorText, tr } from './i18n';
import { useToast } from './toast';

// how often GitHub is asked, and how long nobody has to touch the till before
// a new version installs itself
const EVERY = 5 * 60_000;
const QUIET = 60_000;
// With a ticket on the screen it waits longer, for someone who only stepped
// away. A ticket forgotten there must not stop updates: it is saved, and it
// comes back when Tiquet opens again.
const LEFT = 5 * 60_000;
const FOCUS_GAP = 90_000;
const FETCH_RETRY = 30_000;
// Installing closes Tiquet, and only the installer opens it again. If an
// antivirus stopped the installer Tiquet would just vanish, so a version gets
// a few tries alone; after that it waits for someone to press the button.
const TRIES = 3;
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

function untried(version: string) {
  try {
    const n = tries(version);
    if (n <= 1) localStorage.removeItem(TRIES_KEY);
    else localStorage.setItem(TRIES_KEY, JSON.stringify({ version, n: n - 1 }));
  } catch {
    // ignore storage failures
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
  // the control server wants this PC on a newer version: it is looked for now
  // and installs itself as soon as the till is free, whatever Ajustes says
  force?: boolean;
}

// with an update asked for, the till is free sooner
const FORCED_QUIET = 15_000;

// Asks GitHub for a newer Tiquet a few seconds after start and every quarter
// of an hour, whatever is on the screen: asking and downloading get in nobody's
// way. With `auto` the new version runs once nobody has touched the till for a
// while and no window is open; a ticket on the screen only makes that wait
// longer. Installing closes Tiquet; the installer runs silently and opens the
// new version.
export function useUpdate(ticket: boolean, { on = true, auto: autoSet = false, quiet: quietSet = QUIET, hold = false, before, force = false }: Opts = {}) {
  const auto = autoSet || force;
  const quiet = force ? Math.min(quietSet, FORCED_QUIET) : quietSet;
  const toast = useToast();
  const [found, setFound] = useState<string | null>(null);
  const [later, setLater] = useState(false);
  const [checking, setChecking] = useState(false);
  const [upToDate, setUpToDate] = useState(false);
  const [failed, setFailed] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [progress, setProgress] = useState<number | null>(null);
  const [due, setDue] = useState(false);
  // the version downloaded and waiting, and how many checks have been made
  const [ready, setReady] = useState<string | null>(null);
  const [round, setRound] = useState(0);
  const running = useRef(false);
  const fetching = useRef(false);
  const lastCheck = useRef(0);
  const seen = useRef(Date.now());
  const ticketNow = useRef(ticket);
  ticketNow.current = ticket;
  const holdNow = useRef(hold);
  holdNow.current = hold;
  const beforeNow = useRef(before);
  beforeNow.current = before;

  const check = useCallback(
    async (asked = false) => {
      lastCheck.current = Date.now();
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

  // track user activity continuously so already-idle tills don't restart the wait after download
  useEffect(() => {
    if (!on) return;
    seen.current = Date.now();
    const touched = () => {
      seen.current = Date.now();
    };
    const kinds = ['keydown', 'pointerdown', 'wheel'];
    kinds.forEach((k) => window.addEventListener(k, touched, true));
    return () => kinds.forEach((k) => window.removeEventListener(k, touched, true));
  }, [on]);

  useEffect(() => {
    if (!native || !on) return;
    const first = setTimeout(() => setDue(true), 3000);
    const again = setInterval(() => setDue(true), EVERY);
    const maybeCheck = () => {
      if (document.visibilityState === 'hidden') return;
      if (Date.now() - lastCheck.current >= FOCUS_GAP) setDue(true);
    };
    window.addEventListener('focus', maybeCheck);
    window.addEventListener('online', maybeCheck);
    document.addEventListener('visibilitychange', maybeCheck);
    return () => {
      clearTimeout(first);
      clearInterval(again);
      window.removeEventListener('focus', maybeCheck);
      window.removeEventListener('online', maybeCheck);
      document.removeEventListener('visibilitychange', maybeCheck);
    };
  }, [on]);

  useEffect(() => {
    if (!due) return;
    setDue(false);
    check();
  }, [due, check]);

  // asked for by the control server: no need to wait for the next round
  useEffect(() => {
    if (native && on && force) setDue(true);
  }, [on, force]);

  const run = useCallback(
    async (alone: boolean, target?: string | null) => {
      if (running.current) return;
      running.current = true;
      setInstalling(true);
      setProgress(null);
      let off: (() => void) | undefined;
      let counted = false;
      try {
        off = await listen<{ got: number; total: number | null }>('tiquet://update', (e) => {
          const { got, total } = e.payload;
          setProgress(total ? got / total : null);
        });
        await beforeNow.current?.().catch(() => {});
        if (alone && target) {
          tried(target);
          counted = true;
        }
        // an update that installs never answers: Tiquet closes here
        await call('app.update.install');
        setFound(null);
        setUpToDate(true);
      } catch (e) {
        if (counted && target) untried(target);
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

  const install = useCallback(() => run(false, found), [run, found]);

  // the installer comes down while the shop works; a failed fetch retries shortly
  useEffect(() => {
    if (!on || !auto || !found || ready === found || fetching.current) return;
    fetching.current = true;
    setDownloading(true);
    let alive = true;
    let retryTimer: number | undefined;
    let off: (() => void) | undefined;
    listen<{ got: number; total: number | null }>('tiquet://update', (e) => {
      if (!alive) return;
      const { got, total } = e.payload;
      setProgress(total ? got / total : null);
    }).then((fn) => {
      if (alive) off = fn;
      else fn();
    });
    call<{ version: string } | null>('app.update.fetch')
      .then((r) => {
        if (alive) setReady(r?.version ?? null);
      })
      .catch(() => {
        if (alive) retryTimer = window.setTimeout(() => setRound((n) => n + 1), FETCH_RETRY);
      })
      .finally(() => {
        fetching.current = false;
        off?.();
        if (alive) {
          setDownloading(false);
          if (!running.current) setProgress(null);
        }
      });
    return () => {
      alive = false;
      fetching.current = false;
      if (retryTimer !== undefined) clearTimeout(retryTimer);
      off?.();
    };
  }, [on, auto, found, ready, round]);

  // it runs once the till is free
  const alone = auto && tries(found) < TRIES;
  const waiting = on && alone && !!found && ready === found;
  useEffect(() => {
    if (!waiting) return;
    const t = setInterval(() => {
      if (holdNow.current || document.querySelector('.modal-back, .pay-back')) return;
      if (Date.now() - seen.current < (ticketNow.current ? (force ? QUIET : LEFT) : quiet)) return;
      run(true, found);
    }, 5000);
    return () => clearInterval(t);
  }, [waiting, quiet, found, run, force]);

  return { found, auto: alone, later, checking, upToDate, failed, downloading, installing, progress, check, install, hide: () => setLater(true) };
}

export function UpdateBar({ u }: { u: Updates }) {
  if (!u.found || (!u.installing && !u.downloading && u.later)) return null;
  return (
    <div className="update-bar" role="status">
      <Download size={16} />
      {u.installing || u.downloading ? (
        <span>
          {u.progress !== null
            ? `${tr('Descargando la versión', 'Downloading version')} ${u.found} · ${Math.round(u.progress * 100)} %. `
            : u.installing
              ? `${tr('Instalando la versión', 'Installing version')} ${u.found}. `
              : `${tr('Descargando la versión', 'Downloading version')} ${u.found}… `}
          {u.installing
            ? tr('Tiquet se cerrará y volverá a abrirse solo.', 'Tiquet will close and open again by itself.')
            : tr('Se instalará sola en cuanto la caja esté libre.', 'It will install itself as soon as the till is free.')}
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
