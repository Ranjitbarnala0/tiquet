import { useCallback, useEffect, useState } from 'react';
import { TriangleAlert } from 'lucide-react';
import { call } from './api';
import { errorText, setLang, tr, type Lang } from './i18n';
import { Login } from './Login';
import { Setup } from './Setup';
import { Shell } from './Shell';
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

export function App() {
  const [boot, setBoot] = useState<Boot | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [user, setUser] = useState<User | null>(null);
  const [lang, setLangState] = useState<Lang>(() => {
    const l = storedLang() ?? 'es';
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
    const l = storedLang() ?? b.lang;
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

  // a packaged app should not behave like a browser tab
  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    const keys = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      const browserKey =
        e.key === 'F5' ||
        e.key === 'F7' ||
        (e.ctrlKey && ['r', 'p', 'f', 'g', 'u', 'j', 'h'].includes(k)) ||
        (e.ctrlKey && e.shiftKey && ['i', 'c', 'j', 'r'].includes(k));
      if (browserKey) e.preventDefault();
    };
    const menu = (e: MouseEvent) => {
      if (!(e.target as HTMLElement).closest('input, textarea')) e.preventDefault();
    };
    window.addEventListener('keydown', keys);
    window.addEventListener('contextmenu', menu);
    return () => {
      window.removeEventListener('keydown', keys);
      window.removeEventListener('contextmenu', menu);
    };
  }, []);

  const logout = useCallback(async () => {
    await call('logout').catch(() => {});
    setUser(null);
    load().catch((e) => setFatal(errorText(e)));
  }, [load]);

  if (fatal) {
    return (
      <div className="fatal">
        <TriangleAlert size={34} />
        <h2>{tr('No se pudo iniciar Tiquet', 'Tiquet could not start')}</h2>
        <p>{fatal}</p>
      </div>
    );
  }
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
  return <Shell key={user.id} user={user} setUser={setUser} lang={lang} onLang={changeLang} onLogout={logout} />;
}
