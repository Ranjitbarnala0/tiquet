import { useCallback, useEffect, useState } from 'react';
import { FileText, RefreshCw, Send, Wifi } from 'lucide-react';
import { call, toError } from './api';
import { dateTime, shortDate } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { CertEntry, Settings, TicketAeat, VerifactuStatus } from './types';
import { Field, Modal, Segmented } from './ui';

// the sender keeps its last failure as "<when> <error code>"
export function sendError(raw: string | null): { at: string; text: string } | null {
  if (!raw) return null;
  const i = raw.indexOf(' ');
  return i < 0 ? { at: '', text: errorText(toError(raw)) } : { at: raw.slice(0, i), text: errorText(toError(raw.slice(i + 1))) };
}

// records that should have reached the AEAT by now: the till has to say so
export function stuck(st: VerifactuStatus): boolean {
  const late = !!st.oldest && Date.now() - Date.parse(st.oldest) > 10 * 60_000;
  return st.problems.length > 0 || (st.pending > 0 && (late || !!st.lastError));
}

export function aeatText(a: TicketAeat): string {
  const where = a.env === 'test' ? tr(' (pruebas)', ' (test)') : '';
  const why = [a.code, a.message].filter(Boolean).join(': ');
  const tail = why ? ` · ${why}` : '';
  switch (a.state) {
    case 'pending':
      return tr('VERI*FACTU: pendiente de envío a la AEAT', 'VERI*FACTU: waiting to be sent to the AEAT') + where;
    case 'Correcto':
      return tr('VERI*FACTU: registrado en la AEAT', 'VERI*FACTU: recorded by the AEAT') + where;
    case 'AceptadoConErrores':
      return tr('VERI*FACTU: registrado con avisos', 'VERI*FACTU: recorded with warnings') + where + tail;
    default:
      return tr('VERI*FACTU: rechazado por la AEAT', 'VERI*FACTU: refused by the AEAT') + where + tail;
  }
}

interface Props {
  d: Settings;
  set: <K extends keyof Settings>(k: K, v: Settings[K]) => void;
  onShop: () => void;
}

export function VerifactuTab({ d, set, onShop }: Props) {
  const toast = useToast();
  const [status, setStatus] = useState<VerifactuStatus | null>(null);
  const [certs, setCerts] = useState<CertEntry[] | null>(null);
  const [declaration, setDeclaration] = useState<string | null>(null);
  const [busy, setBusy] = useState('');

  const loadStatus = useCallback(() => {
    call<VerifactuStatus>('verifactu.status')
      .then(setStatus)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  const loadCerts = useCallback(() => {
    call<CertEntry[]>('verifactu.certs')
      .then(setCerts)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  useEffect(() => {
    loadStatus();
    loadCerts();
    const t = setInterval(loadStatus, 20_000);
    return () => clearInterval(t);
  }, [loadStatus, loadCerts]);

  const reach = async () => {
    setBusy('reach');
    try {
      const r = await call<{ status: number }>('verifactu.reach', { env: d.verifactu === 'test' ? 'test' : 'prod' });
      toast.ok(`${tr('La AEAT responde', 'The AEAT answers')} (HTTP ${r.status})`);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const correct = async (id: number) => {
    setBusy(`fix${id}`);
    try {
      await call('verifactu.correct', { id });
      toast.ok(tr('Registro corregido: se enviará enseguida', 'Record corrected: it will be sent shortly'));
      loadStatus();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const showDeclaration = () =>
    call<{ text: string }>('verifactu.declaration')
      .then((r) => setDeclaration(r.text))
      .catch((e) => toast.error(errorText(e)));

  const failure = sendError(status?.lastError ?? null);
  const known = certs?.some((c) => c.thumbprint === d.cert);
  const hint = {
    off: tr('Sin registros ni QR en el ticket, como hasta ahora.', 'No records and no QR on the ticket, as before.'),
    test: tr(
      'Cada ticket, devolución y anulación genera su registro y va al entorno de pruebas de la AEAT. Sirve para comprobar el certificado; el QR lleva a la web de pruebas.',
      'Every ticket, return and void makes its record, which goes to the AEAT’s test environment. Use it to check the certificate; the QR leads to the test site.',
    ),
    prod: tr(
      'Cada ticket, devolución y anulación genera su registro y se envía a la AEAT, que lo da por recibido. El ticket lleva el QR y la leyenda VERI*FACTU. Cuando ya sea obligatorio, una vez empezado debe seguir así hasta el 31 de diciembre.',
      'Every ticket, return and void makes its record, sent to the AEAT, which confirms it. The ticket carries the QR and the VERI*FACTU legend. Once it is mandatory, having started it must stay on until 31 December.',
    ),
  }[d.verifactu];

  return (
    <div className="stack">
      <Field label="VERI*FACTU" hint={hint}>
        <Segmented
          value={d.verifactu}
          onChange={(v) => set('verifactu', v)}
          options={[
            { value: 'off', label: tr('Apagado', 'Off') },
            { value: 'test', label: tr('Pruebas AEAT', 'AEAT test') },
            { value: 'prod', label: tr('Real', 'Live') },
          ]}
        />
      </Field>

      {status && !status.producer && d.verifactu !== 'off' && (
        <div className="clock-note">
          <p>{errorText(toError('E_PRODUCER'))}</p>
        </div>
      )}
      {d.verifactu !== 'off' && (!d.shopNif.trim() || !(d.legalName.trim() || d.shopName.trim())) && (
        <div className="info-card">
          <span>{tr('La AEAT necesita el NIF y el nombre del titular tal como constan en Hacienda.', 'The AEAT needs the NIF and the owner’s name as Hacienda has them.')}</span>
          <button className="btn btn-ghost sm" onClick={onShop}>
            {tr('Ir a Comercio', 'Go to Shop')}
          </button>
        </div>
      )}

      <Field
        label={tr('Certificado electrónico', 'Electronic certificate')}
        hint={tr(
          'El del titular (FNMT u otro cualificado), instalado en Windows para este usuario. Con él se envían los registros.',
          'The owner’s (FNMT or another qualified one), installed in Windows for this user. The records are sent with it.',
        )}
      >
        <div className="row-gap">
          <select value={d.cert} onChange={(e) => set('cert', e.target.value)} className="grow">
            <option value="">{certs && certs.length === 0 ? tr('No hay certificados instalados', 'No certificates installed') : tr('Elige uno', 'Choose one')}</option>
            {certs?.map((c) => (
              <option key={c.thumbprint} value={c.thumbprint}>
                {c.name} · {c.issuer} · {c.valid ? tr('válido hasta', 'valid until') : tr('caducado el', 'expired on')} {c.expires ? shortDate(c.expires) : '?'}
              </option>
            ))}
            {d.cert && certs && !known && <option value={d.cert}>{tr('Un certificado que ya no está en Windows', 'A certificate no longer in Windows')}</option>}
          </select>
          <button
            className="btn btn-ghost"
            onClick={loadCerts}
            title={tr('Volver a buscar certificados', 'Look for certificates again')}
            aria-label={tr('Volver a buscar certificados', 'Look for certificates again')}
          >
            <RefreshCw size={16} />
          </button>
        </div>
      </Field>

      {status && (
        <div className="vf-status">
          <div>
            <span className="muted">{tr('Sin enviar', 'Not sent yet')}</span>
            <b>{status.pending}</b>
            {status.oldest && (
              <span className="muted small-text">
                {tr('desde', 'since')} {dateTime(status.oldest)}
              </span>
            )}
          </div>
          <div>
            <span className="muted">{tr('Enviados', 'Sent')}</span>
            <b>{status.sent}</b>
          </div>
          <div>
            <span className="muted">{tr('Último envío', 'Last sent')}</span>
            <b className="small-text">{status.lastSend ? dateTime(status.lastSend) : tr('Ninguno', 'None')}</b>
          </div>
        </div>
      )}
      {failure && (
        <p className="form-error">
          {failure.at && `${dateTime(failure.at)} · `}
          {failure.text}. {tr('Tiquet lo vuelve a intentar solo; mientras, se puede seguir vendiendo.', 'Tiquet tries again by itself; selling carries on meanwhile.')}
        </p>
      )}
      {status?.clockDrift != null && (
        <p className="form-error">
          {tr('El reloj de este ordenador no va en hora según la AEAT', 'This computer’s clock is off according to the AEAT')}: {Math.round(status.clockDrift / 60)} min.{' '}
          {tr('Ponlo en hora en Windows (Ajustes > Hora e idioma).', 'Set it right in Windows (Settings > Time & language).')}
        </p>
      )}

      {status && status.problems.length > 0 && (
        <div className="field">
          <span className="field-label">{tr('Registros que la AEAT pide corregir', 'Records the AEAT asks to correct')}</span>
          <div className="backup-list">
            {status.problems.map((p) => (
              <div key={p.recordId} className="backup-row vf-problem">
                <div>
                  <b className="mono">
                    {p.number} · {p.kind === 'alta' ? tr('factura', 'invoice') : tr('anulación', 'void')}
                    {p.env === 'test' ? tr(' (pruebas)', ' (test)') : ''}
                  </b>
                  <p>
                    {p.state === 'Incorrecto' ? tr('Rechazado', 'Refused') : tr('Aceptado con errores', 'Accepted with errors')}
                    {p.code ? ` (${p.code})` : ''}
                    {p.message ? `: ${p.message}` : ''}
                  </p>
                </div>
                <button className="btn btn-ghost sm" onClick={() => correct(p.recordId)} disabled={!!busy}>
                  <Send size={15} />
                  {tr('Corregir y reenviar', 'Correct and resend')}
                </button>
              </div>
            ))}
          </div>
          <span className="field-hint">
            {tr(
              'Primero arregla lo que dice la AEAT (por ejemplo el nombre del titular en Comercio). Se envía un registro nuevo de subsanación; el ticket no cambia.',
              'First fix what the AEAT says (for example the owner’s name in Shop). A new correcting record is sent; the ticket does not change.',
            )}
          </span>
        </div>
      )}

      <div className="row-gap">
        <button className="btn" onClick={reach} disabled={busy === 'reach'}>
          <Wifi size={16} />
          {tr('Probar conexión con la AEAT', 'Test the connection to the AEAT')}
        </button>
        <button className="btn" onClick={showDeclaration}>
          <FileText size={16} />
          {tr('Declaración responsable', 'Declaración responsable')}
        </button>
      </div>

      {declaration && (
        <Modal wide title={tr('Declaración responsable', 'Declaración responsable')} onClose={() => setDeclaration(null)}>
          <div className="declaration">{declaration}</div>
        </Modal>
      )}
    </div>
  );
}
