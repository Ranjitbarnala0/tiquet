import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ArrowRight } from 'lucide-react';
import { call } from './api';
import { initials } from './format';
import { errorText, tr, type Lang } from './i18n';
import type { Boot, User } from './types';
import { LangSwitch, Mark } from './ui';

const LAST = 'tiquet.lastUser';

function lastUser(users: User[]): User | null {
  let id = 0;
  try {
    id = Number(localStorage.getItem(LAST));
  } catch {
    // no storage: nobody is preselected
  }
  return users.find((u) => u.id === id) ?? (users.length === 1 ? users[0] : null);
}

export function Login({ boot, lang, onLang, onLogin }: { boot: Boot; lang: Lang; onLang: (l: Lang) => void; onLogin: (u: User) => void }) {
  const [sel, setSel] = useState<User | null>(() => lastUser(boot.users));
  const [pw, setPw] = useState('');
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [shake, setShake] = useState(0);
  const pwRef = useRef<HTMLInputElement>(null);

  useEffect(() => pwRef.current?.focus(), [sel, shake]);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!sel || !pw || busy) return;
    setBusy(true);
    setErr('');
    try {
      const r = await call<{ user: User }>('login', { userId: sel.id, password: pw });
      try {
        localStorage.setItem(LAST, String(r.user.id));
      } catch {
        // preselection is only a convenience
      }
      onLogin(r.user);
    } catch (e2) {
      setErr(errorText(e2));
      setPw('');
      setShake((s) => s + 1);
      setBusy(false);
    }
  };

  return (
    <div className="onboard">
      <div className="login-card">
        <div className="onboard-top">
          <div className="brand">
            <Mark size={30} />
            <span>Tiquet</span>
            {boot.shopName && <span className="brand-shop">{boot.shopName}</span>}
          </div>
          <LangSwitch lang={lang} onLang={onLang} />
        </div>
        <h1>{tr('¿Quién está en caja?', 'Who is at the till?')}</h1>
        <div className="who">
          {boot.users.map((u) => (
            <button
              key={u.id}
              type="button"
              className={`who-btn${sel?.id === u.id ? ' on' : ''}`}
              onClick={() => {
                setSel(u);
                setErr('');
                setPw('');
              }}
            >
              <span className="avatar">{initials(u.name)}</span>
              <span className="who-name">{u.name}</span>
              <span className="who-role">{u.role === 'admin' ? tr('Administrador', 'Administrator') : tr('Cajero', 'Cashier')}</span>
            </button>
          ))}
        </div>
        {sel && (
          <form onSubmit={submit} className={`login-form${shake ? ' shake' : ''}`} key={shake}>
            <label className="field">
              <span className="field-label">
                {tr('Contraseña de', 'Password for')} {sel.name}
              </span>
              <div className="pw-row">
                <input ref={pwRef} type="password" value={pw} onChange={(e) => setPw(e.target.value)} autoComplete="current-password" />
                <button className="btn btn-primary" disabled={!pw || busy} aria-label={tr('Entrar', 'Sign in')}>
                  <ArrowRight size={18} />
                </button>
              </div>
            </label>
            {err && <p className="form-error">{err}</p>}
          </form>
        )}
        <div className="login-foot">Tiquet {boot.version}</div>
      </div>
    </div>
  );
}
