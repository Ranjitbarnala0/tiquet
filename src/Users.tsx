import { useCallback, useEffect, useState, type FormEvent } from 'react';
import { Plus, UserRound } from 'lucide-react';
import { call } from './api';
import { initials, shortDate } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { Role, User, UserRow } from './types';
import { Drawer, Empty, Field, Segmented, Toggle } from './ui';

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
  const [busy, setBusy] = useState(false);

  const load = useCallback(() => {
    call<UserRow[]>('users.list')
      .then(setList)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  useEffect(load, [load]);

  const isNew = edit?.id === null;
  const pwShort = !!edit && edit.password.length > 0 && edit.password.length < 4;
  const valid = !!edit && edit.name.trim().length > 0 && !pwShort && (!isNew || edit.password.length >= 4);

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
          <button className="btn btn-primary" onClick={() => setEdit({ id: null, name: '', role: 'cashier', password: '', active: true })}>
            <Plus size={16} />
            {tr('Nuevo usuario', 'New user')}
          </button>
        </div>
      </div>

      <div className="cards">
        {list?.map((u) => (
          <button key={u.id} className={`user-card${u.active ? '' : ' off'}`} onClick={() => setEdit({ id: u.id, name: u.name, role: u.role, password: '', active: u.active })}>
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

      {edit && (
        <Drawer
          title={isNew ? tr('Nuevo usuario', 'New user') : edit.name}
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
              hint={isNew ? tr('Mínimo 4 caracteres. Un PIN vale.', 'At least 4 characters. A PIN is fine.') : tr('Déjala vacía para no cambiarla', 'Leave empty to keep the current one')}
              error={pwShort ? tr('Mínimo 4 caracteres', 'At least 4 characters') : undefined}
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
