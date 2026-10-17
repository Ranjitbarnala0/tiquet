import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { KeyRound, Plus, TriangleAlert, UserRound } from 'lucide-react';
import { call } from './api';
import { dateTime, initials, shortDate } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { Role, User, UserRow } from './types';
import { CodeView } from './Recovery';
import { Drawer, Empty, Field, Modal, Segmented, Toggle } from './ui';

interface Draft {
  id: number | null;
  name: string;
  role: Role;
  password: string;
  active: boolean;
}

export function Users({ me, onMe }: { me: User; onMe: (u: User) => void }) {
  const toast = useToast();
  const [list, setList] = useState<UserRow[] | null>(null);
  const [edit, setEdit] = useState<Draft | null>(null);
  const [opened, setOpened] = useState('');
  const [busy, setBusy] = useState(false);
  const [recovery, setRecovery] = useState<{ set: boolean; at: string | null } | null>(null);
  const [renew, setRenew] = useState<'ask' | string | null>(null);

  const load = useCallback(() => {
    call<UserRow[]>('users.list')
      .then(setList)
      .catch((e) => toast.error(errorText(e)));
    call<{ set: boolean; at: string | null }>('recovery.status')
      .then(setRecovery)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  useEffect(load, [load]);

  const newCode = async () => {
    try {
      const r = await call<{ code: string }>('recovery.new');
      setRenew(r.code);
      load();
    } catch (e) {
      toast.error(errorText(e));
      setRenew(null);
    }
  };

  const isNew = edit?.id === null;
  const dirty = !!edit && JSON.stringify(edit) !== opened;
  const open = (d: Draft) => {
    setEdit(d);
    setOpened(JSON.stringify(d));
  };
  const minPw = edit?.role === 'admin' ? 6 : 4;
  const pwShort = !!edit && edit.password.length > 0 && edit.password.length < minPw;
  const valid = !!edit && edit.name.trim().length > 0 && !pwShort && (!isNew || edit.password.length >= minPw);

  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!edit || !valid || busy) return;
    setBusy(true);
    try {
      const row = await call<UserRow>('users.save', { ...edit, id: edit.id ?? undefined });
      if (row.id === me.id) onMe({ id: row.id, name: row.name, role: row.role });
      toast.ok(tr('Usuario guardado', 'User saved'));
      setEdit(null);
      load();
    } catch (err) {
      toast.error(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page narrow">
      <div className="page-head">
        <div>
          <h1>{tr('Usuarios', 'Users')}</h1>
          <p className="muted">
            {tr(
              'Cada persona entra con su nombre y contraseña. Los cajeros venden y reimprimen; los administradores lo gestionan todo.',
              'Everyone signs in with their own name and password. Cashiers sell and reprint; administrators manage everything.',
            )}
          </p>
        </div>
        <div className="page-actions">
          <button className="btn btn-primary" onClick={() => open({ id: null, name: '', role: 'cashier', password: '', active: true })}>
            <Plus size={16} />
            {tr('Nuevo usuario', 'New user')}
          </button>
        </div>
      </div>

      {recovery && (
        <div className={`recovery-card${recovery.set ? '' : ' warn'}`}>
          {recovery.set ? <KeyRound size={20} /> : <TriangleAlert size={20} />}
          <div>
            <b>{tr('Código de recuperación', 'Recovery code')}</b>
            <span>
              {recovery.set
                ? `${tr('Creado el', 'Created on')} ${recovery.at ? dateTime(recovery.at) : ''}. ${tr('Sirve para entrar si se olvida la contraseña de administrador.', 'It lets you in if the administrator password is forgotten.')}`
                : tr(
                    'No hay ninguno. Sin él, si se olvida la contraseña de administrador no habrá forma de entrar.',
                    'There is none. Without it, a forgotten administrator password means no way back in.',
                  )}
            </span>
          </div>
          <button className={`btn${recovery.set ? '' : ' btn-primary'}`} onClick={() => (recovery.set ? setRenew('ask') : newCode())}>
            {recovery.set ? tr('Crear uno nuevo', 'Make a new one') : tr('Crear código', 'Create code')}
          </button>
        </div>
      )}

      <div className="cards">
        {list?.map((u) => (
          <button key={u.id} className={`user-card${u.active ? '' : ' off'}`} onClick={() => open({ id: u.id, name: u.name, role: u.role, password: '', active: u.active })}>
            <span className="avatar">{initials(u.name)}</span>
            <span className="user-card-text">
              <b>
                {u.name}
                {u.id === me.id && <span className="badge">{tr('Tú', 'You')}</span>}
              </b>
              <span>
                {u.role === 'admin' ? tr('Administrador', 'Administrator') : tr('Cajero', 'Cashier')}
                {!u.active && ` · ${tr('Desactivado', 'Disabled')}`}
                {` · ${tr('desde', 'since')} ${shortDate(u.createdAt)}`}
              </span>
            </span>
          </button>
        ))}
        {list && list.length === 0 && <Empty icon={<UserRound size={28} />} title={tr('Sin usuarios', 'No users')} />}
      </div>

      {renew === 'ask' && (
        <Modal
          title={tr('¿Crear un código nuevo?', 'Make a new code?')}
          onClose={() => setRenew(null)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setRenew(null)}>
                {tr('Cancelar', 'Cancel')}
              </button>
              <button className="btn btn-primary" onClick={newCode}>
                {tr('Crear código nuevo', 'Make new code')}
              </button>
            </>
          }
        >
          <p className="muted">
            {tr('El código que tengas guardado dejará de valer al momento.', 'The code you have stored stops working right away.')}
          </p>
        </Modal>
      )}

      {renew && renew !== 'ask' && (
        <Modal locked title={tr('Código de recuperación nuevo', 'New recovery code')} onClose={() => setRenew(null)}>
          <CodeView code={renew} canPrint onDone={() => setRenew(null)} doneLabel={tr('Hecho', 'Done')} />
        </Modal>
      )}

      {edit && (
        <Drawer
          title={isNew ? tr('Nuevo usuario', 'New user') : edit.name}
          dirty={dirty}
          onClose={() => setEdit(null)}
          footer={
            <>
              <span />
              <button className="btn btn-primary" onClick={() => save()} disabled={!valid || busy}>
                {tr('Guardar', 'Save')}
              </button>
            </>
          }
        >
          <form className="stack" onSubmit={save}>
            <Field label={tr('Nombre', 'Name')} hint={tr('Sale en el ticket como «Le atendió»', 'Printed on tickets as the cashier')}>
              <input autoFocus value={edit.name} onChange={(e) => setEdit({ ...edit, name: e.target.value })} maxLength={40} />
            </Field>
            <Field label={tr('Rol', 'Role')}>
              <Segmented
                value={edit.role}
                onChange={(role) => setEdit({ ...edit, role })}
                options={[
                  { value: 'cashier', label: tr('Cajero', 'Cashier') },
                  { value: 'admin', label: tr('Administrador', 'Administrator') },
                ]}
              />
            </Field>
            <Field
              label={isNew ? tr('Contraseña', 'Password') : tr('Nueva contraseña', 'New password')}
              hint={
                isNew
                  ? edit.role === 'admin'
                    ? tr('Mínimo 6 caracteres para un administrador.', 'At least 6 characters for an administrator.')
                    : tr('Mínimo 4 caracteres. Un PIN vale.', 'At least 4 characters. A PIN is fine.')
                  : tr('Déjala vacía para no cambiarla', 'Leave empty to keep the current one')
              }
              error={pwShort ? tr(`Mínimo ${minPw} caracteres`, `At least ${minPw} characters`) : undefined}
            >
              <input type="password" value={edit.password} onChange={(e) => setEdit({ ...edit, password: e.target.value })} autoComplete="new-password" />
            </Field>
            {!isNew && (
              <Toggle
                checked={edit.active}
                onChange={(active) => setEdit({ ...edit, active })}
                label={tr('Puede entrar', 'Can sign in')}
                hint={tr('Desactívalo en lugar de borrarlo: sus tickets siguen a su nombre', 'Disable instead of deleting: their tickets keep their name')}
              />
            )}
            <button type="submit" hidden />
          </form>
        </Drawer>
      )}
    </div>
  );
}
