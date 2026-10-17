import { Clock3 } from 'lucide-react';
import { dateTime } from './format';
import { tr } from './i18n';
import { Modal } from './ui';

// The core refuses to date a ticket before the last one by more than an hour.
// A wrong PC clock is fixed in Windows; a right one is confirmed here once.
export function ClockModal({ last, onCancel, onConfirm }: { last: string; onCancel: () => void; onConfirm: () => void }) {
  const now = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  const local = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}T${p(now.getHours())}:${p(now.getMinutes())}:00`;
  return (
    <Modal
      title={tr('Revisa la fecha del ordenador', 'Check the computer date')}
      onClose={onCancel}
      actions={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>
            {tr('Cancelar', 'Cancel')}
          </button>
          <button className="btn btn-primary" onClick={onConfirm}>
            {tr('La fecha es correcta, seguir', 'The date is right, continue')}
          </button>
        </>
      }
    >
      <div className="clock-note">
        <Clock3 size={20} />
        <p>
          {tr('El último ticket es del', 'The last ticket is from')} <b>{dateTime(last)}</b>, {tr('pero el ordenador marca', 'but the computer says')}{' '}
          <b>{dateTime(local)}</b>.
        </p>
      </div>
      <p className="muted">
        {tr(
          'Si el reloj de Windows está mal, corrígelo antes de cobrar: la fecha sale en el ticket. Si la fecha del ordenador es la buena, sigue y no se volverá a preguntar.',
          'If the Windows clock is wrong, fix it before charging: the date prints on the ticket. If the computer date is right, continue and you will not be asked again.',
        )}
      </p>
    </Modal>
  );
}
