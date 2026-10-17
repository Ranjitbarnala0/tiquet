import { useState, type FormEvent } from 'react';
import { ArrowRight } from 'lucide-react';
import { call } from './api';
import { errorText, tr, type Lang } from './i18n';
import { spanishNif } from './nif';
import { CodeView } from './Recovery';
import { useToast } from './toast';
import type { User } from './types';
import { Field, LangSwitch, Mark, Toggle } from './ui';

export function Setup({ lang, onLang, onDone }: { lang: Lang; onLang: (l: Lang) => void; onDone: (u: User) => void }) {
  const toast = useToast();
  const [f, setF] = useState({
    shopName: '',
    legalName: '',
    shopAddress: '',
    shopPhone: '',
    shopNif: '',
    adminName: '',
    password: '',
    confirm: '',
    samples: true,
  });
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<{ user: User; code: string } | null>(null);
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });

  const mismatch = f.confirm.length > 0 && f.password !== f.confirm;
  const short = f.password.length > 0 && f.password.length < 6;
  const badNif = f.shopNif.trim().length > 0 && !spanishNif(f.shopNif);
  const valid = f.shopName.trim() && spanishNif(f.shopNif) && f.adminName.trim() && f.password.length >= 6 && f.password === f.confirm;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!valid || busy) return;
    setBusy(true);
    try {
      const r = await call<{ user: User; recoveryCode: string }>('setup', { ...f, lang });
      setDone({ user: r.user, code: r.recoveryCode });
    } catch (err) {
      toast.error(errorText(err));
      setBusy(false);
    }
  };

  if (done) {
    return (
      <div className="onboard">
        <div className="onboard-card">
          <div className="onboard-top">
            <div className="brand">
              <Mark size={30} />
              <span>Tiquet</span>
            </div>
          </div>
          <h1>{tr('Tu código de recuperación', 'Your recovery code')}</h1>
          <CodeView code={done.code} canPrint={false} onDone={() => onDone(done.user)} doneLabel={tr('Empezar a vender', 'Start selling')} />
        </div>
      </div>
    );
  }

  return (
    <div className="onboard">
      <form className="onboard-card" onSubmit={submit}>
        <div className="onboard-top">
          <div className="brand">
            <Mark size={30} />
            <span>Tiquet</span>
          </div>
          <LangSwitch lang={lang} onLang={onLang} />
        </div>
        <h1>{tr('Configura tu comercio', 'Set up your shop')}</h1>
        <p className="lede">
          {tr(
            'Estos datos salen en la cabecera de cada ticket. Podrás cambiarlos cuando quieras en Ajustes.',
            'These details print at the top of every ticket. You can change them any time in Settings.',
          )}
        </p>

        <section className="form-section">
          <h2>{tr('Comercio', 'Shop')}</h2>
          <Field label={tr('Nombre del comercio', 'Shop name')}>
            <input autoFocus value={f.shopName} onChange={set('shopName')} placeholder="Mi Tienda" maxLength={60} />
          </Field>
          <Field
            label={tr('Titular o razón social', 'Owner or company name')}
            hint={tr('El nombre completo del titular o de la empresa, si el comercio usa otro nombre', 'The full name of the owner or company, if the shop trades under another name')}
          >
            <input value={f.legalName} onChange={set('legalName')} maxLength={80} />
          </Field>
          <Field label={tr('Dirección', 'Address')} hint={tr('Una línea por renglón del ticket', 'One line per ticket line')}>
            <textarea rows={2} value={f.shopAddress} onChange={set('shopAddress')} placeholder={'Calle Mayor, 1\n17600 Figueres (Girona)'} />
          </Field>
          <div className="grid2">
            <Field label={tr('Teléfono', 'Phone')}>
              <input value={f.shopPhone} onChange={set('shopPhone')} placeholder="972 000 000" />
            </Field>
            <Field label="NIF / CIF" error={badNif ? tr('Revisa el NIF: la letra no coincide', 'Check the NIF: the letter does not match') : undefined} hint={tr('Sale en cada ticket', 'Printed on every ticket')}>
              <input value={f.shopNif} onChange={(e) => setF({ ...f, shopNif: e.target.value.toUpperCase() })} placeholder="B12345674" />
            </Field>
          </div>
        </section>

        <section className="form-section">
          <h2>{tr('Administrador', 'Administrator')}</h2>
          <Field label={tr('Tu nombre', 'Your name')} hint={tr('Sale en el ticket como «Le atendió»', 'Printed on the ticket as the cashier')}>
            <input value={f.adminName} onChange={set('adminName')} maxLength={40} />
          </Field>
          <div className="grid2">
            <Field
              label={tr('Contraseña', 'Password')}
              error={short ? tr('Mínimo 6 caracteres', 'At least 6 characters') : undefined}
              hint={tr('Mínimo 6 caracteres; un PIN de 6 cifras vale', 'At least 6 characters; a 6-digit PIN is fine')}
            >
              <input type="password" value={f.password} onChange={set('password')} autoComplete="new-password" />
            </Field>
            <Field label={tr('Repite la contraseña', 'Repeat password')} error={mismatch ? tr('No coinciden', 'They do not match') : undefined}>
              <input type="password" value={f.confirm} onChange={set('confirm')} autoComplete="new-password" />
            </Field>
          </div>
        </section>

        <Toggle
          checked={f.samples}
          onChange={(v) => setF({ ...f, samples: v })}
          label={tr('Añadir productos de ejemplo', 'Add sample products')}
          hint={tr('Cables, fundas, cargadores… Para probar ya. Se pueden archivar después.', 'Cables, cases, chargers… to try it right away. You can archive them later.')}
        />

        <button className="btn btn-primary btn-lg block" disabled={!valid || busy}>
          {tr('Empezar a vender', 'Start selling')}
          <ArrowRight size={18} />
        </button>
      </form>
    </div>
  );
}
