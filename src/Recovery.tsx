import { useState, type FormEvent } from 'react';
import { ArrowRight, Copy, KeyRound, Printer } from 'lucide-react';
import { call } from './api';
import { errorText, tr, warningText } from './i18n';
import { useToast } from './toast';
import type { User } from './types';
import { Field } from './ui';

// Shown exactly once per code: after setup, after a recovery and after making
// a new one. The core keeps only a hash, so this is the only chance to see it.
export function CodeView({ code, canPrint, onDone, doneLabel }: { code: string; canPrint: boolean; onDone: () => void; doneLabel: string }) {
  const toast = useToast();
  const [saved, setSaved] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code);
      toast.ok(tr('Código copiado', 'Code copied'));
    } catch {
      toast.error(tr('No se pudo copiar: apúntalo a mano', 'Could not copy: write it down by hand'));
    }
  };

  const print = async () => {
    try {
      const r = await call<{ warning: string | null }>('recovery.print', { code });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr('Enviado a la impresora', 'Sent to the printer'));
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  return (
    <div className="code-view">
      <div className="code-icon">
        <KeyRound size={22} />
      </div>
      <p>
        {tr(
          'Si algún día se olvida la contraseña de administrador, este código permite poner una nueva. Apúntalo o imprímelo y guárdalo fuera de la tienda.',
          'If the administrator password is ever forgotten, this code lets you set a new one. Write it down or print it and keep it away from the shop.',
        )}
      </p>
      <div className="code-box mono" aria-label={tr('Código de recuperación', 'Recovery code')}>
        {code}
      </div>
      <div className="code-actions">
        <button type="button" className="btn" onClick={copy}>
          <Copy size={16} />
          {tr('Copiar', 'Copy')}
        </button>
        {canPrint && (
          <button type="button" className="btn" onClick={print}>
            <Printer size={16} />
            {tr('Imprimir', 'Print')}
          </button>
        )}
      </div>
      <p className="muted small">{tr('Solo se muestra ahora. Cada vez que se usa, Tiquet da uno nuevo.', 'It is only shown now. Each time it is used, Tiquet gives a new one.')}</p>
      <label className="check">
        <input type="checkbox" checked={saved} onChange={(e) => setSaved(e.target.checked)} />
        <span>{tr('Lo he guardado en un lugar seguro', 'I have stored it somewhere safe')}</span>
      </label>
      <button type="button" className="btn btn-primary btn-lg block" disabled={!saved} onClick={onDone}>
        {doneLabel}
        <ArrowRight size={18} />
      </button>
    </div>
  );
}

export function RecoverForm({ user, onBack, onDone }: { user: User; onBack: () => void; onDone: (u: User, code: string) => void }) {
  const [code, setCode] = useState('');
  const [pw, setPw] = useState('');
  const [again, setAgain] = useState('');
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const short = pw.length > 0 && pw.length < 6;
  const mismatch = again.length > 0 && pw !== again;
  const valid = code.replace(/[^a-z0-9]/gi, '').length === 16 && pw.length >= 6 && pw === again;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!valid || busy) return;
    setBusy(true);
    setErr('');
    try {
      const r = await call<{ user: User; recoveryCode: string }>('recover', { code, userId: user.id, password: pw });
      onDone(r.user, r.recoveryCode);
    } catch (e2) {
      setErr(errorText(e2));
      setBusy(false);
    }
  };

  return (
    <form className="recover" onSubmit={submit}>
      <p className="muted">
        {tr('Escribe el código de recuperación que se guardó al configurar Tiquet y elige una contraseña nueva para', 'Type the recovery code saved when Tiquet was set up and choose a new password for')}{' '}
        <b>{user.name}</b>.
      </p>
      <Field label={tr('Código de recuperación', 'Recovery code')}>
        <input
          autoFocus
          className="mono code-input"
          value={code}
          onChange={(e) => setCode(e.target.value.toUpperCase())}
          placeholder="XXXX-XXXX-XXXX-XXXX"
          maxLength={24}
          spellCheck={false}
          autoComplete="off"
        />
      </Field>
      <div className="grid2">
        <Field label={tr('Contraseña nueva', 'New password')} error={short ? tr('Mínimo 6 caracteres', 'At least 6 characters') : undefined}>
          <input type="password" value={pw} onChange={(e) => setPw(e.target.value)} autoComplete="new-password" />
        </Field>
        <Field label={tr('Repítela', 'Repeat it')} error={mismatch ? tr('No coinciden', 'They do not match') : undefined}>
          <input type="password" value={again} onChange={(e) => setAgain(e.target.value)} autoComplete="new-password" />
        </Field>
      </div>
      {err && <p className="form-error">{err}</p>}
      <div className="recover-actions">
        <button type="button" className="btn btn-ghost" onClick={onBack}>
          {tr('Volver', 'Back')}
        </button>
        <button className="btn btn-primary" disabled={!valid || busy}>
          {tr('Poner contraseña nueva', 'Set new password')}
        </button>
      </div>
    </form>
  );
}
