import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ArrowRight } from 'lucide-react';
import { call } from './api';
import { initials } from './format';
import { errorText, tr, type Lang } from './i18n';
import { CodeView, RecoverForm } from './Recovery';
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
  const [mode, setMode] = useState<'pw' | 'forgot' | 'recover'>('pw');
  const [recovered, setRecovered] = useState<{ user: User; code: string } | null>(null);
  const pwRef = useRef<HTMLInputElement>(null);

  useEffect(() => pwRef.current?.focus(), [sel, shake, mode]);

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
        {recovered ? (
          <>
            <h1>{tr('Contraseña cambiada', 'Password changed')}</h1>
            <p className="lede">{tr('Este es tu código de recuperación nuevo. El anterior ya no vale.', 'This is your new recovery code. The old one no longer works.')}</p>
            <CodeView code={recovered.code} canPrint onDone={() => onLogin(recovered.user)} doneLabel={tr('Entrar', 'Sign in')} />
          </>
        ) : mode === 'recover' && sel ? (
          <>
            <h1>{tr('Contraseña olvidada', 'Forgotten password')}</h1>
            <RecoverForm user={sel} onBack={() => setMode('pw')} onDone={(user, code) => setRecovered({ user, code })} />
          </>
        ) : (
          <>
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
                    setMode('pw');
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
                {mode === 'forgot' ? (
                  <p className="forgot-note">
                    {sel.role !== 'admin'
                      ? tr('Pide a un administrador que te ponga una contraseña nueva en Usuarios.', 'Ask an administrator to set a new password for you in Users.')
                      : tr(
                          'Este equipo no tiene código de recuperación. Si hay otro administrador, puede ponerte una contraseña nueva en Usuarios.',
                          'This computer has no recovery code. If there is another administrator, they can set a new password for you in Users.',
                        )}
                  </p>
                ) : (
                  <button type="button" className="link forgot" onClick={() => setMode(sel.role === 'admin' && boot.canRecover ? 'recover' : 'forgot')}>
                    {tr('¿Has olvidado la contraseña?', 'Forgot your password?')}
                  </button>
                )}
              </form>
            )}
          </>
        )}
        <div className="login-foot">Tiquet {boot.version}</div>
      </div>
    </div>
  );
}
