import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ClipboardCopy, FileDown, Mail, MessageCircle, Paperclip } from 'lucide-react';
import { call, native } from './api';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { Repair } from './types';
import { Field, Modal } from './ui';

interface WhatsAppOpened {
  url: string;
  opened: boolean;
  app: boolean;
}

// what the core answered after asking Windows to open WhatsApp
function whatsAppOpened(r: WhatsAppOpened, toast: ReturnType<typeof useToast>): boolean {
  if (!native) {
    window.open(r.url, '_blank', 'noopener');
    return true;
  }
  if (!r.opened) {
    toast.error(tr('No se pudo abrir WhatsApp en este equipo.', 'WhatsApp could not be opened on this computer.'));
    return false;
  }
  toast.ok(
    r.app
      ? tr('WhatsApp abierto con el mensaje escrito: pulsa Enviar en el chat.', 'WhatsApp is open with the message written: press Send in the chat.')
      : tr('WhatsApp Web abierto en el navegador: pulsa Enviar en el chat.', 'WhatsApp Web is open in the browser: press Send in the chat.'),
  );
  return true;
}

// Sends a ticket to the customer: WhatsApp with the ticket written in the
// chat, email with the PDF attached, or the PDF to keep or paste anywhere.
export function ShareDialog({ ticketId, number, onClose }: { ticketId: number; number: string; onClose: () => void }) {
  const toast = useToast();
  const [phone, setPhone] = useState('');
  const [email, setEmail] = useState('');
  const [busy, setBusy] = useState('');
  const [mailReady, setMailReady] = useState<boolean | null>(null);
  const phoneRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    phoneRef.current?.focus();
    call<{ ready: boolean }>('email.status')
      .then((r) => setMailReady(r.ready))
      .catch(() => setMailReady(false));
  }, []);

  const run = async (what: string, f: () => Promise<void>) => {
    if (busy) return;
    setBusy(what);
    try {
      await f();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const whatsapp = (e?: FormEvent) => {
    e?.preventDefault();
    if (!phone.trim()) return phoneRef.current?.focus();
    run('wa', async () => {
      const r = await call<WhatsAppOpened>('tickets.whatsapp', { id: ticketId, phone });
      if (whatsAppOpened(r, toast)) onClose();
    });
  };

  const mail = (e?: FormEvent) => {
    e?.preventDefault();
    if (!email.trim()) return;
    run('mail', async () => {
      const r = await call<{ sent: boolean; opened?: boolean; copied?: boolean; url?: string; path?: string }>('tickets.email', { id: ticketId, to: email.trim() });
      if (r.sent) {
        toast.ok(`${tr('Ticket enviado a', 'Ticket sent to')} ${email.trim()}`);
      } else if (!native && r.url) {
        window.open(r.url, '_blank', 'noopener');
      } else if (r.opened) {
        toast.info(
          r.copied
            ? tr('Se abrió tu programa de correo. El PDF del ticket está copiado: pégalo en el mensaje con Ctrl+V.', 'Your mail app is open. The ticket PDF is copied: paste it into the message with Ctrl+V.')
            : `${tr('Se abrió tu programa de correo. Adjunta el PDF:', 'Your mail app is open. Attach the PDF:')} ${r.path}`,
          { sticky: true },
        );
      } else {
        toast.info(
          `${tr('Este equipo no tiene programa de correo. El PDF está guardado en', 'This computer has no mail app. The PDF is saved in')} ${r.path}. ${tr(
            'Para enviarlo directamente desde Tiquet, configura tu correo en Ajustes > Enviar tickets.',
            'To send it straight from Tiquet, set up your mail in Settings > Send tickets.',
          )}`,
          { sticky: true },
        );
      }
      onClose();
    });
  };

  const pdf = (mode: 'reveal' | 'copy') =>
    run(mode, async () => {
      const r = await call<{ path: string; copied: boolean }>('tickets.pdf', { id: ticketId, reveal: mode === 'reveal', copy: mode === 'copy' });
      if (mode === 'copy' && r.copied) toast.ok(tr('PDF copiado: pégalo en WhatsApp o en un correo con Ctrl+V.', 'PDF copied: paste it into WhatsApp or an email with Ctrl+V.'));
      else toast.ok(`${tr('PDF guardado en', 'PDF saved to')} ${r.path}`);
    });

  const copyText = () =>
    run('text', async () => {
      const r = await call<{ text: string }>('tickets.text', { id: ticketId });
      await navigator.clipboard.writeText(r.text);
      toast.ok(tr('Texto del ticket copiado', 'Ticket text copied'));
    });

  return (
    <Modal title={`${tr('Enviar ticket', 'Send ticket')} ${number}`} onClose={onClose}>
      <div className="stack share">
        <form onSubmit={whatsapp}>
          <Field label={tr('WhatsApp del cliente', 'Customer WhatsApp')} hint={tr('El ticket se escribe en el chat; solo hay que pulsar Enviar.', 'The ticket is written in the chat; just press Send.')}>
            <div className="row-gap">
              <input ref={phoneRef} className="grow" value={phone} onChange={(e) => setPhone(e.target.value)} inputMode="tel" placeholder="612 345 678" maxLength={30} />
              <button className="btn btn-whatsapp" disabled={!phone.trim() || !!busy}>
                <MessageCircle size={16} />
                WhatsApp
              </button>
            </div>
          </Field>
        </form>
        <form onSubmit={mail}>
          <Field
            label={tr('Email del cliente', 'Customer email')}
            hint={
              mailReady
                ? tr('Sale desde tu correo con el ticket en PDF.', 'It goes from your mail account with the ticket as a PDF.')
                : tr('Se abre tu programa de correo con el PDF listo para pegar. Para enviarlo sin salir de Tiquet, configura tu correo en Ajustes.', 'Your mail app opens with the PDF ready to paste. To send it without leaving Tiquet, set up your mail in Settings.')
            }
          >
            <div className="row-gap">
              <input className="grow" type="email" value={email} onChange={(e) => setEmail(e.target.value)} placeholder="cliente@correo.com" maxLength={120} />
              <button className="btn" disabled={!email.trim() || !!busy}>
                <Mail size={16} />
                {busy === 'mail' ? tr('Enviando…', 'Sending…') : tr('Enviar', 'Send')}
              </button>
            </div>
          </Field>
        </form>
        <div className="row-gap wrap">
          <button className="btn btn-ghost" onClick={() => pdf('reveal')} disabled={!!busy}>
            <FileDown size={16} />
            {tr('Guardar PDF', 'Save PDF')}
          </button>
          {native && (
            <button className="btn btn-ghost" onClick={() => pdf('copy')} disabled={!!busy} title={tr('Para pegarlo con Ctrl+V en WhatsApp o en un correo', 'To paste with Ctrl+V into WhatsApp or an email')}>
              <Paperclip size={16} />
              {tr('Copiar PDF', 'Copy PDF')}
            </button>
          )}
          <button className="btn btn-ghost" onClick={copyText} disabled={!!busy}>
            <ClipboardCopy size={16} />
            {tr('Copiar texto', 'Copy text')}
          </button>
        </div>
      </div>
    </Modal>
  );
}

// The message a repair's status calls for, from the shop to the customer.
export function repairMessage(r: Repair, shop: string): string {
  const name = r.customerName.trim().split(/\s+/)[0] ?? '';
  const device = [r.brand, r.model].filter((s) => s.trim()).join(' ') || 'equipo';
  const hello = name ? `Hola ${name},` : 'Hola,';
  const due = r.dueCents !== null && r.dueCents > 0 ? ` Importe pendiente: ${(r.dueCents / 100).toLocaleString('es-ES', { minimumFractionDigits: 2 })} €.` : '';
  const sign = shop.trim() ? `\n${shop.trim()}` : '';
  switch (r.status) {
    case 'ready':
      return `${hello} tu ${device} ya está reparado y listo para recoger.${due} Nº de reparación: ${r.number}.${sign}`;
    case 'not_repaired':
      return `${hello} no hemos podido reparar tu ${device}. Puedes pasar a recogerlo cuando quieras. Nº de reparación: ${r.number}.${sign}`;
    case 'in_progress':
      return `${hello} ya estamos reparando tu ${device}. Te avisamos en cuanto esté listo. Nº de reparación: ${r.number}.${sign}`;
    default:
      return `${hello} hemos recibido tu ${device} para reparar. Te avisamos en cuanto esté listo. Nº de reparación: ${r.number}.${sign}`;
  }
}

// WhatsApp to the customer of a repair, with the message for its status
export function RepairWhatsApp({ repair, shop, disabled }: { repair: Repair; shop: string; disabled?: boolean }) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const send = async () => {
    setBusy(true);
    try {
      const r = await call<WhatsAppOpened>('share.whatsapp', { phone: repair.customerPhone, text: repairMessage(repair, shop) });
      whatsAppOpened(r, toast);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <button className="btn btn-whatsapp-ghost" onClick={send} disabled={disabled || busy || !repair.customerPhone.trim()} title={tr('Avisar al cliente por WhatsApp', 'Message the customer on WhatsApp')}>
      <MessageCircle size={16} />
      {repair.status === 'ready' ? tr('Avisar: listo', 'Tell: ready') : tr('WhatsApp', 'WhatsApp')}
    </button>
  );
}
