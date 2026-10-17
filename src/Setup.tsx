import { useState, type FormEvent } from 'react';
import { ArrowRight } from 'lucide-react';
import { call } from './api';
import { errorText, tr, type Lang } from './i18n';
import { useToast } from './toast';
import type { User } from './types';
import { Field, LangSwitch, Mark, Toggle } from './ui';

export function Setup({ lang, onLang, onDone }: { lang: Lang; onLang: (l: Lang) => void; onDone: (u: User) => void }) {
  const toast = useToast();
  const [f, setF] = useState({
    shopName: '',
    shopAddress: '',
    shopPhone: '',
    shopNif: '',
    adminName: '',
    password: '',
    confirm: '',
    samples: true,
  });
  const [busy, setBusy] = useState(false);
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });

  const mismatch = f.confirm.length > 0 && f.password !== f.confirm;
  const short = f.password.length > 0 && f.password.length < 4;
  const valid = f.shopName.trim() && f.adminName.trim() && f.password.length >= 4 && f.password === f.confirm;

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!valid || busy) return;
    setBusy(true);
    try {
      const r = await call<{ user: User }>('setup', { ...f, lang });
      onDone(r.user);
    } catch (err) {
      toast.error(errorText(err));
      setBusy(false);
    }
  };

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
          <Field label={tr('Dirección', 'Address')} hint={tr('Una línea por renglón del ticket', 'One line per ticket line')}>
            <textarea rows={2} value={f.shopAddress} onChange={set('shopAddress')} placeholder={'Calle Mayor, 1\n17600 Figueres (Girona)'} />
          </Field>
          <div className="grid2">
            <Field label={tr('Teléfono', 'Phone')}>
              <input value={f.shopPhone} onChange={set('shopPhone')} placeholder="972 000 000" />
            </Field>
            <Field label="NIF / CIF">
              <input value={f.shopNif} onChange={set('shopNif')} placeholder="B12345678" />
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
              error={short ? tr('Mínimo 4 caracteres', 'At least 4 characters') : undefined}
              hint={tr('Un PIN de 4 cifras vale', 'A 4-digit PIN is fine')}
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
