import { useCallback, useEffect, useState } from 'react';
import { FolderOpen, History, RotateCcw, TriangleAlert } from 'lucide-react';
import { call, onSessionLost } from './api';
import { dateTime } from './format';
import { errorText, setLang, tr, type Lang } from './i18n';
import { Login } from './Login';
import { Setup } from './Setup';
import { Shell } from './Shell';
import { useToast } from './toast';
import type { Boot, User } from './types';

const LANG_KEY = 'tiquet.lang';

function storedLang(): Lang | null {
  try {
    const v = localStorage.getItem(LANG_KEY);
    return v === 'es' || v === 'en' ? v : null;
  } catch {
    return null;
  }
}

// a new PC starts in the language Windows uses: Spanish for Spain's
// languages, English otherwise
function windowsLang(): Lang {
  const l = (navigator.languages?.[0] ?? navigator.language ?? 'es').toLowerCase();
  return /^(es|ca|gl|eu)\b/.test(l) ? 'es' : 'en';
}

interface AppInfo {
  dataDir: string | null;
  logPath: string | null;
  backups: { path: string; name: string; modified: string }[];
}

// Shown when the database will not open. The packaged app can still list the
// backups in the default folder and put one in place of the damaged file.
function Fatal({ message }: { message: string }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    call<AppInfo>('app.info')
      .then(setInfo)
      .catch(() => {});
  }, []);

  const run = async (cmd: string, args: object = {}) => {
    if (busy) return;
    setBusy(true);
    setErr('');
    try {
      await call(cmd, args);
      location.reload();
    } catch (e) {
      setErr(errorText(e));
      setBusy(false);
    }
  };

  return (
    <div className="fatal">
      <TriangleAlert size={34} />
      <h2>{tr('No se pudo iniciar Tiquet', 'Tiquet could not start')}</h2>
      <p>{message}</p>
      {info && (
        <div className="fatal-paths">
          {info.dataDir && (
            <span>
              {tr('Datos', 'Data')}: <span className="mono">{info.dataDir}</span>
            </span>
          )}
          {info.logPath && (
            <span>
              {tr('Registro', 'Log')}: <span className="mono">{info.logPath}</span>
            </span>
          )}
        </div>
      )}
      <div className="fatal-actions">
        <button className="btn btn-primary" onClick={() => run('app.retry')} disabled={busy}>
          <RotateCcw size={16} />
          {tr('Reintentar', 'Try again')}
        </button>
        {info?.dataDir && (
          <button className="btn" onClick={() => call('app.reveal').catch(() => {})}>
            <FolderOpen size={16} />
            {tr('Abrir carpeta de datos', 'Open data folder')}
          </button>
        )}
      </div>
      {info && info.backups.length > 0 && (
        <div className="backup-list">
          {info.backups.slice(0, 8).map((b) => (
            <div key={b.path} className="backup-row">
              <div>
                <b>{dateTime(b.modified)}</b>
                <span>{b.name}</span>
              </div>
              <button className="btn btn-ghost sm" onClick={() => run('app.restore', { path: b.path })} disabled={busy}>
                <History size={15} />
                {tr('Restaurar', 'Restore')}
              </button>
            </div>
          ))}
        </div>
      )}
      {err && <p className="form-error">{err}</p>}
    </div>
  );
}

export function App() {
  const toast = useToast();
  const [boot, setBoot] = useState<Boot | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [user, setUser] = useState<User | null>(null);
  const [lang, setLangState] = useState<Lang>(() => {
    const l = storedLang() ?? windowsLang();
    setLang(l);
    return l;
  });

  const changeLang = useCallback((l: Lang) => {
    setLang(l);
    setLangState(l);
    try {
      localStorage.setItem(LANG_KEY, l);
    } catch {
      // the choice just won't survive a restart
    }
  }, []);

  const load = useCallback(async () => {
    const b = await call<Boot>('bootstrap');
    const l = storedLang() ?? (b.needsSetup ? windowsLang() : b.lang);
    setLang(l);
    setLangState(l);
    setBoot(b);
    setUser(b.session);
    return b;
  }, []);

  useEffect(() => {
    load().then(
      (b) => call('app.ready', { ok: true, version: b.version }).catch(() => {}),
      (e) => {
        setFatal(errorText(e));
        call('app.ready', { ok: false, error: String(e) }).catch(() => {});
      },
    );
  }, [load]);

  // browser shortcuts are switched off in WebView2 itself; the page menu
  // (back, reload, print) is not wanted outside text fields either
  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const menu = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest('input, textarea')) e.preventDefault();
    };
    window.addEventListener('contextmenu', menu);
    return () => window.removeEventListener('contextmenu', menu);
  }, []);

  // the core locked the session: back to the sign-in screen
  useEffect(() => {
    onSessionLost((code) => {
      setUser(null);
      load().catch((e) => setFatal(errorText(e)));
      if (code === 'E_SESSION_IDLE') toast.info(tr('La caja se bloqueó porque nadie la usaba. Vuelve a entrar.', 'The till locked because nobody was using it. Sign in again.'));
    });
    return () => onSessionLost(null);
  }, [load, toast]);

  const logout = useCallback(async () => {
    await call('logout').catch(() => {});
    setUser(null);
    load().catch((e) => setFatal(errorText(e)));
  }, [load]);

  if (fatal) return <Fatal message={fatal} />;
  if (!boot) return <div className="splash" />;
  if (boot.needsSetup) {
    return (
      <Setup
        lang={lang}
        onLang={changeLang}
        onDone={(u) => {
          setUser(u);
          load().catch((e) => setFatal(errorText(e)));
        }}
      />
    );
  }
  if (!user) return <Login boot={boot} lang={lang} onLang={changeLang} onLogin={setUser} />;
  return (
    <Shell key={user.id} installId={boot.installId} user={user} setUser={setUser} lang={lang} onLang={changeLang} onLogout={logout} />
  );
}
