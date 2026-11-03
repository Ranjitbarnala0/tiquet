import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { BadgeCheck, Download, FolderOpen, HardDrive, History, ImagePlus, LifeBuoy, Mail, Printer, ReceiptText, RefreshCw, ScanSearch, Store, Trash2 } from 'lucide-react';
import { call, native } from './api';
import { dateTime, rate, VAT_RATES } from './format';
import { errorText, printerStateText, tr, warningText, type Lang } from './i18n';
import { loadImage, rasterize, type LogoMode } from './logo';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { SettingsTab } from './Shell';
import type { Logo, MailConfig, PLine, PortEntry, PrinterEntry, Preview, Settings, UserRow } from './types';
import { spanishNif } from './nif';
import type { Updates } from './Update';
import { printerLabel, usePrinterStatus } from './usePrinter';
import { Field, Modal, Segmented, Toggle } from './ui';
import { VerifactuTab } from './Verifactu';

interface BackupStatus {
  dir: string;
  defaultDir: string;
  last: string | null;
  lastPath: string | null;
  error: string | null;
  dataDir: string;
  logPath: string | null;
}

interface BackupFile {
  path: string;
  name: string;
  bytes: number;
  modified: string;
}

interface Restored {
  safetyCopy: string;
  missing: { first: string; last: string }[];
}

const size = (b: number) => (b < 1_000_000 ? `${Math.max(1, Math.round(b / 1000))} KB` : `${(b / 1_000_000).toLocaleString('es-ES', { maximumFractionDigits: 1 })} MB`);

const CODEPAGES = [
  { id: 'cp858', label: () => tr('PC858 · Europa con € (recomendado)', 'PC858 · Western Europe with € (recommended)') },
  { id: 'cp1252', label: () => 'Windows-1252' },
  { id: 'cp850', label: () => tr('PC850 · sin símbolo €', 'PC850 · no € sign') },
  { id: 'ascii', label: () => tr('Sin acentos (máxima compatibilidad)', 'No accents (works everywhere)') },
];

interface Props {
  onRestored: () => void;
  onDirty: (dirty: boolean) => void;
  request: { tab: SettingsTab; n: number };
  settings: Settings;
  onSaved: (s: Settings) => void;
  logo: Logo | null;
  onLogo: () => void;
  lang: Lang;
  onLang: (l: Lang) => void;
  version: string;
  updates: Updates;
  saleBusy: boolean;
}

export function SettingsScreen({ onRestored, onDirty, request, settings, onSaved, logo, onLogo, lang, onLang, version, updates, saleBusy }: Props) {
  const toast = useToast();
  const [d, setD] = useState<Settings>(settings);
  const [tab, setTab] = useState<SettingsTab>(request.tab);
  const [preview, setPreview] = useState<PLine[] | null>(null);
  const [printers, setPrinters] = useState<PrinterEntry[] | null>(null);
  const [ports, setPorts] = useState<PortEntry[] | null>(null);
  const [users, setUsers] = useState<UserRow[] | null>(null);
  const { status: printerStatus, refresh: refreshStatus } = usePrinterStatus(tab === 'printer', printerLabel(settings));

  useEffect(() => setTab(request.tab), [request]);
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const [files, setFiles] = useState<BackupFile[] | null>(null);
  const [restoring, setRestoring] = useState<BackupFile | null>(null);
  const [restored, setRestored] = useState<Restored | null>(null);
  const [busy, setBusy] = useState('');
  const [img, setImg] = useState<HTMLImageElement | null>(null);
  const [logoSize, setLogoSize] = useState(55);
  const [logoMode, setLogoMode] = useState<LogoMode>('logo');
  const [logoLight, setLogoLight] = useState(0);
  const fileRef = useRef<HTMLInputElement>(null);

  const dirty = JSON.stringify(d) !== JSON.stringify(settings);
  useEffect(() => onDirty(dirty), [dirty, onDirty]);
  useEffect(() => () => onDirty(false), [onDirty]);
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setD((prev) => ({ ...prev, [k]: v }));

  useEffect(() => {
    const t = setTimeout(() => {
      call<Preview>('sale.preview', { sample: true, settings: d })
        .then((p) => setPreview(p.lines))
        .catch((e) => toast.error(errorText(e)));
    }, 120);
    return () => clearTimeout(t);
  }, [d, logo, toast]);

  // looks for the receipt printer among those installed; the choice still
  // has to be saved
  const detect = async () => {
    try {
      const r = await call<{ best: string | null; columns: number; candidates: string[] }>('printers.detect');
      loadPrinters();
      if (r.best) {
        set('printerName', r.best);
        set('printerKind', 'windows');
        set('columns', r.columns);
        toast.ok(`${tr('Encontrada', 'Found')}: ${r.best}. ${tr('Guarda los ajustes para usarla.', 'Save the settings to use it.')}`);
      } else if (r.candidates.length > 1) {
        toast.info(`${tr('Hay varias que podrían ser de tickets', 'Several could be receipt printers')}: ${r.candidates.join(', ')}. ${tr('Elige una en la lista.', 'Pick one from the list.')}`);
      } else {
        toast.info(tr('No se encontró ninguna impresora de tickets. Conéctala, enciéndela e instala su driver en Windows.', 'No receipt printer found. Connect it, switch it on and install its driver in Windows.'));
      }
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const loadPrinters = useCallback(() => {
    call<{ printers: PrinterEntry[]; ports: PortEntry[] }>('printers.list')
      .then((r) => {
        setPrinters(r.printers);
        setPorts(r.ports);
      })
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  // a receipt printer gets raw tickets; any other prints them as a document
  const pickPrinter = (name: string) => {
    const p = printers?.find((x) => x.name === name);
    // the paper its driver is set up for, when it says
    setD((prev) => ({ ...prev, printerName: name, printerKind: p ? (p.receipt ? 'windows' : 'document') : prev.printerKind, columns: p?.columns ?? prev.columns }));
  };

  useEffect(() => {
    if (tab !== 'data') return;
    call<UserRow[]>('users.list')
      .then(setUsers)
      .catch(() => setUsers([]));
  }, [tab]);

  const loadBackup = useCallback(() => {
    call<BackupStatus>('backup.status')
      .then(setBackup)
      .catch((e) => toast.error(errorText(e)));
    call<BackupFile[]>('backup.list')
      .then(setFiles)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  const wipe = async () => {
    if (busy) return;
    setBusy('wipe');
    try {
      const r = await call<{ deleted: number }>('training.wipe');
      toast.ok(tr(`${r.deleted} tickets de prueba borrados`, `${r.deleted} practice tickets deleted`));
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const reveal = (what: 'data' | 'backups') => call('app.reveal', { what }).catch((e) => toast.error(errorText(e)));

  const restore = async () => {
    if (!restoring || busy) return;
    setBusy('restore');
    try {
      setRestored(await call<Restored>('backup.restore', { path: restoring.path }));
      setRestoring(null);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  useEffect(() => {
    loadPrinters();
    loadBackup();
  }, [loadPrinters, loadBackup]);

  const save = useCallback(async () => {
    if (busy) return;
    setBusy('save');
    try {
      const s = await call<Settings>('settings.save', d);
      onSaved(s);
      setD(s);
      toast.ok(tr('Ajustes guardados', 'Settings saved'));
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  }, [busy, d, onSaved, toast]);

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key.toLowerCase() === 's') {
        e.preventDefault();
        if (dirty) save();
      }
    };
    window.addEventListener('keydown', k);
    return () => window.removeEventListener('keydown', k);
  }, [dirty, save]);

  const test = async (kind: 'page' | 'charset' | 'drawer') => {
    setBusy(kind);
    try {
      const r = await call<{ warning: string | null }>('printers.test', { settings: d, kind });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(kind === 'drawer' ? tr('Orden de apertura enviada', 'Open command sent') : tr('Prueba enviada a la impresora', 'Test sent to the printer'));
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
      refreshStatus();
    }
  };

  const draftLogo = useMemo(
    () => (img ? rasterize(img, (Math.min(d.columns * 12, 576) * logoSize) / 100, logoMode, logoLight) : null),
    [img, d.columns, logoSize, logoMode, logoLight],
  );

  const saveLogo = async () => {
    if (!draftLogo) return;
    setBusy('logo');
    try {
      await call('logo.set', draftLogo);
      setImg(null);
      onLogo();
      toast.ok(tr('Logo guardado', 'Logo saved'));
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const clearLogo = async () => {
    try {
      await call('logo.clear');
      onLogo();
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const diagnose = async () => {
    setBusy('diag');
    try {
      const r = await call<{ path: string }>('diagnostics.save');
      toast.ok(`${tr('Diagnóstico guardado en', 'Diagnostics saved to')} ${r.path}`);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const backupNow = async () => {
    setBusy('backup');
    try {
      const r = await call<{ path: string }>('backup.now');
      toast.ok(`${tr('Copia guardada en', 'Backup saved to')} ${r.path}`);
      loadBackup();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const tabs: { id: SettingsTab; label: string; icon: typeof Store }[] = [
    { id: 'shop', label: tr('Comercio', 'Shop'), icon: Store },
    { id: 'ticket', label: tr('Ticket', 'Ticket'), icon: ReceiptText },
    { id: 'printer', label: tr('Impresora', 'Printer'), icon: Printer },
    { id: 'mail', label: tr('Enviar tickets', 'Send tickets'), icon: Mail },
    { id: 'data', label: tr('Datos y copias', 'Data & backups'), icon: HardDrive },
    { id: 'verifactu', label: 'VERI*FACTU', icon: BadgeCheck },
  ];

  const selected = printers?.find((p) => p.name === d.printerName);
  const selectedVirtual = d.printerKind === 'windows' && !!selected && (selected.virtual || !selected.receipt);
  const network = d.printerKind === 'network';
  const serial = d.printerKind === 'serial';
  const doc = d.printerKind === 'document';
  // fax and OneNote open windows of their own for every job
  const opensWindows = (p: PrinterEntry) => /fax|onenote/i.test(`${p.name} ${p.port} ${p.driver}`);
  const target = printerLabel(d);

  return (
    <div className="settings">
      <section className="settings-main">
        <div className="page-head">
          <div>
            <h1>{tr('Ajustes', 'Settings')}</h1>
            <p className="muted">{tr('La vista previa de la derecha cambia mientras escribes.', 'The preview on the right updates as you type.')}</p>
          </div>
        </div>
        <div className="tabs">
          {tabs.map((t) => (
            <button key={t.id} className={tab === t.id ? 'on' : ''} onClick={() => setTab(t.id)}>
              <t.icon size={16} />
              {t.label}
            </button>
          ))}
        </div>

        <div className="settings-body">
          {tab === 'shop' && (
            <div className="stack">
              <Field label={tr('Nombre del comercio', 'Shop name')} hint={tr('En grande, arriba del ticket', 'Large, at the top of the ticket')}>
                <input value={d.shopName} onChange={(e) => set('shopName', e.target.value)} maxLength={60} />
              </Field>
              <Field
                label={tr('Titular o razón social', 'Owner or company name')}
                hint={tr('Debajo del nombre, si el comercio usa un nombre comercial', 'Under the name, when the shop trades under another name')}
              >
                <input value={d.legalName} onChange={(e) => set('legalName', e.target.value)} maxLength={80} />
              </Field>
              <Field label={tr('Dirección', 'Address')} hint={tr('Cada renglón es una línea del ticket', 'Each line here is one ticket line')}>
                <textarea rows={3} value={d.shopAddress} onChange={(e) => set('shopAddress', e.target.value)} />
              </Field>
              <div className="grid2">
                <Field label={tr('Teléfono', 'Phone')}>
                  <input value={d.shopPhone} onChange={(e) => set('shopPhone', e.target.value)} />
                </Field>
                <Field
                  label={tr('NIF / CIF (opcional)', 'NIF / CIF (optional)')}
                  error={d.shopNif.trim() && !spanishNif(d.shopNif) ? tr('Revisa el NIF: la letra no coincide', 'Check the NIF: the letter does not match') : undefined}
                  hint={!d.shopNif.trim() ? tr('Sin NIF los tickets salen como ticket sencillo, no como factura.', 'Without a NIF, tickets print as simple tickets, not invoices.') : undefined}
                >
                  <input value={d.shopNif} onChange={(e) => set('shopNif', e.target.value.toUpperCase())} />
                </Field>
              </div>
            </div>
          )}

          {tab === 'ticket' && (
            <div className="stack">
              <Field label={tr('Pie del ticket', 'Ticket footer')} hint={tr('Agradecimiento, horario, política de cambios…', 'Thank-you note, opening hours, returns policy…')}>
                <textarea rows={3} value={d.footer} onChange={(e) => set('footer', e.target.value)} />
              </Field>
              <Field
                label={tr('Condiciones del resguardo de reparación', 'Repair receipt conditions')}
                hint={tr('Salen en el resguardo que se lleva el cliente al dejar un equipo', 'Printed on the receipt customers get when they leave a device')}
              >
                <textarea rows={3} value={d.repairTerms} onChange={(e) => set('repairTerms', e.target.value)} maxLength={600} />
              </Field>

              <div className="field">
                <span className="field-label">Logo</span>
                <div className="logo-box">
                  {img && draftLogo ? (
                    <>
                      <div className="logo-stage">
                        <img src={draftLogo.png} alt="" style={{ width: `${(draftLogo.w / Math.min(d.columns * 12, 576)) * 100}%` }} />
                      </div>
                      <div className="logo-controls">
                        <Segmented
                          value={logoMode}
                          onChange={setLogoMode}
                          options={[
                            { value: 'logo', label: tr('Logo o texto', 'Logo or text') },
                            { value: 'photo', label: tr('Foto', 'Photo') },
                          ]}
                        />
                        <label className="range">
                          <span>{tr('Tamaño', 'Size')}</span>
                          <input type="range" min={20} max={100} value={logoSize} onChange={(e) => setLogoSize(Number(e.target.value))} />
                        </label>
                        <label className="range">
                          <span>{tr('Brillo', 'Brightness')}</span>
                          <input type="range" min={-80} max={80} value={logoLight} onChange={(e) => setLogoLight(Number(e.target.value))} />
                        </label>
                        <div className="row-gap">
                          <button className="btn btn-ghost" onClick={() => setImg(null)}>
                            {tr('Cancelar', 'Cancel')}
                          </button>
                          <button className="btn btn-primary" onClick={saveLogo} disabled={busy === 'logo'}>
                            {tr('Usar este logo', 'Use this logo')}
                          </button>
                        </div>
                      </div>
                    </>
                  ) : (
                    <>
                      <div className="logo-stage">
                        {logo ? (
                          <img src={logo.png} alt="" style={{ width: `${(logo.w / Math.min(d.columns * 12, 576)) * 100}%` }} />
                        ) : (
                          <span className="muted">{tr('Sin logo', 'No logo')}</span>
                        )}
                      </div>
                      <div className="row-gap">
                        <button className="btn" onClick={() => fileRef.current?.click()}>
                          <ImagePlus size={16} />
                          {logo ? tr('Cambiar imagen', 'Change image') : tr('Subir imagen', 'Upload image')}
                        </button>
                        {logo && (
                          <button className="btn btn-ghost" onClick={clearLogo}>
                            <Trash2 size={16} />
                            {tr('Quitar', 'Remove')}
                          </button>
                        )}
                      </div>
                    </>
                  )}
                  <input
                    ref={fileRef}
                    type="file"
                    accept="image/*"
                    hidden
                    onChange={async (e) => {
                      const f = e.target.files?.[0];
                      e.target.value = '';
                      if (!f) return;
                      try {
                        setImg(await loadImage(f));
                        setLogoMode(f.type === 'image/jpeg' ? 'photo' : 'logo');
                        setLogoLight(0);
                      } catch {
                        toast.error(tr('No se pudo leer la imagen', 'The image could not be read'));
                      }
                    }}
                  />
                </div>
                <span className="field-hint">{tr('PNG con fondo blanco o transparente queda mejor. Se imprime en blanco y negro.', 'A PNG on white or transparent works best. It prints in black and white.')}</span>
              </div>

              <div className="grid2">
                <Field
                  label={tr('Prefijo de numeración', 'Numbering prefix')}
                  hint={`${tr('Ejemplo', 'Example')}: ${d.ticketPrefix || 'T'}${new Date().getFullYear()}-000001`}
                  error={['RE', 'REP'].includes(d.ticketPrefix) ? tr('RE y REP son de las reparaciones: elige otro', 'RE and REP belong to repairs: choose another') : undefined}
                >
                  <input value={d.ticketPrefix} onChange={(e) => set('ticketPrefix', e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 6))} />
                </Field>
                <Field label={tr('IVA por defecto', 'Default VAT')} hint={tr('Para artículos libres y productos nuevos', 'For custom items and new products')}>
                  <select value={d.defaultVatBp} onChange={(e) => set('defaultVatBp', Number(e.target.value))}>
                    {[...new Set([...VAT_RATES, d.defaultVatBp])].map((r) => (
                      <option key={r} value={r}>
                        {rate(r)}
                      </option>
                    ))}
                  </select>
                </Field>
              </div>
              <Toggle checked={d.showVat} onChange={(v) => set('showVat', v)} label={tr('Imprimir desglose de IVA', 'Print VAT breakdown')} hint={tr('Base, cuota y total por tipo de IVA', 'Net, VAT and total per rate')} />
              <Field label={tr('Datos del comercio en el ticket', 'Shop details on the ticket')} hint={tr('Quita lo que no quieras imprimir. Sin NIF, el ticket sale como «TICKET» en vez de factura simplificada. Las facturas con datos del cliente siempre llevan NIF y dirección.', 'Leave out what you do not want printed. Without the NIF the ticket says "TICKET" instead of simplified invoice. Invoices with customer details always carry the NIF and address.')}>
                <div className="grid2">
                  <Toggle checked={d.ticketNif || d.verifactu !== 'off'} onChange={(v) => set('ticketNif', v)} label={tr('NIF', 'NIF')} hint={d.verifactu !== 'off' ? tr('Obligatorio con VERI*FACTU', 'Required with VERI*FACTU') : undefined} />
                  <Toggle checked={d.ticketAddress} onChange={(v) => set('ticketAddress', v)} label={tr('Dirección', 'Address')} />
                  <Toggle checked={d.ticketPhone} onChange={(v) => set('ticketPhone', v)} label={tr('Teléfono', 'Phone')} />
                  <Toggle checked={d.ticketCashier} onChange={(v) => set('ticketCashier', v)} label={tr('«Le atendió» (cajero)', '"Served by" (cashier)')} />
                </div>
              </Field>
            </div>
          )}

          {tab === 'printer' && (
            <div className="stack">
              <Field label={tr('Cómo está conectada', 'How it is connected')}>
                <Segmented
                  value={d.printerKind}
                  onChange={(v) => set('printerKind', v)}
                  options={[
                    { value: 'windows', label: tr('De tickets (USB)', 'Receipt (USB)') },
                    { value: 'document', label: tr('Normal o PDF', 'Office or PDF') },
                    { value: 'network', label: tr('Red (IP)', 'Network (IP)') },
                    { value: 'serial', label: 'Bluetooth / COM' },
                  ]}
                />
              </Field>
              {network ? (
                <div className="grid-host">
                  <Field
                    label={tr('Dirección IP de la impresora', 'Printer IP address')}
                    hint={tr('La imprime la propia impresora en su hoja de configuración (en muchas, encendiéndola con el botón FEED pulsado).', 'The printer prints it on its own configuration sheet (on many, by switching it on with FEED held down).')}
                  >
                    <input value={d.printerHost} onChange={(e) => set('printerHost', e.target.value.trim())} placeholder="192.168.1.100" className="mono" spellCheck={false} />
                  </Field>
                  <Field label={tr('Puerto', 'Port')} hint={tr('Casi siempre 9100', 'Almost always 9100')}>
                    <input
                      type="number"
                      min={1}
                      max={65535}
                      value={d.printerPort}
                      onChange={(e) => set('printerPort', Math.max(1, Math.min(65535, Math.round(Number(e.target.value)) || 9100)))}
                    />
                  </Field>
                </div>
              ) : serial ? (
                <div className="grid-host">
                  <Field
                    label={tr('Puerto COM', 'COM port')}
                    hint={tr(
                      'Empareja antes la impresora en Configuración de Windows > Bluetooth. Windows le da un puerto COM «saliente»: es ese.',
                      'Pair the printer first in Windows Settings > Bluetooth. Windows gives it an "outgoing" COM port: that is the one.',
                    )}
                  >
                    <div className="row-gap">
                      <select value={d.printerCom} onChange={(e) => set('printerCom', e.target.value)} className="grow">
                        <option value="">{tr('Elige el puerto', 'Choose the port')}</option>
                        {d.printerCom && ports && !ports.some((p) => p.name === d.printerCom) && (
                          <option value={d.printerCom}>
                            {d.printerCom} ({tr('no conectado ahora', 'not connected now')})
                          </option>
                        )}
                        {ports?.map((p) => (
                          <option key={p.name} value={p.name}>
                            {p.name} · {p.kind === 'bluetooth' ? 'Bluetooth' : p.kind === 'usb' ? 'USB' : tr('serie', 'serial')}
                          </option>
                        ))}
                        {['COM1', 'COM2', 'COM3', 'COM4', 'COM5', 'COM6', 'COM7', 'COM8', 'COM9']
                          .filter((c) => c !== d.printerCom && !ports?.some((p) => p.name === c))
                          .map((c) => (
                            <option key={c} value={c}>
                              {c}
                            </option>
                          ))}
                      </select>
                      <button className="btn btn-ghost" onClick={loadPrinters} title={tr('Volver a buscar puertos', 'Look for ports again')} aria-label={tr('Volver a buscar puertos', 'Look for ports again')}>
                        <RefreshCw size={16} />
                      </button>
                    </div>
                  </Field>
                  <Field label={tr('Velocidad', 'Speed')} hint={tr('Solo cuenta por cable; Bluetooth la ignora', 'Only matters on a cable; Bluetooth ignores it')}>
                    <select value={d.printerBaud} onChange={(e) => set('printerBaud', Number(e.target.value))}>
                      {[9600, 19200, 38400, 57600, 115200].map((b) => (
                        <option key={b} value={b}>
                          {b}
                        </option>
                      ))}
                    </select>
                  </Field>
                </div>
              ) : (
                <Field
                  label={doc ? tr('Impresora', 'Printer') : tr('Impresora de tickets', 'Receipt printer')}
                  hint={tr('Todas las impresoras instaladas en Windows. Al elegir una, Tiquet sabe si es de tickets o normal.', 'Every printer installed in Windows. Pick one and Tiquet knows whether it is a receipt printer or a normal one.')}
                >
                  <div className="row-gap">
                    <select value={d.printerName} onChange={(e) => pickPrinter(e.target.value)} className="grow">
                      <option value="">{tr('Ninguna (guardar tickets sin imprimir)', 'None (save tickets without printing)')}</option>
                      {d.printerName && printers && !selected && (
                        <option value={d.printerName}>
                          {d.printerName} ({tr('no encontrada', 'not found')})
                        </option>
                      )}
                      {printers?.map((p) => (
                        <option key={p.name} value={p.name} disabled={opensWindows(p) && p.name !== d.printerName}>
                          {p.name}
                          {p.receipt ? ` · ${tr('tickets', 'receipt')}${p.columns ? ` ${p.columns === 32 ? '58' : '80'} mm` : ''}` : p.virtual ? ` · ${tr('guarda archivos', 'saves files')}` : ''}
                          {p.default ? ` (${tr('predeterminada', 'default')})` : ''}
                        </option>
                      ))}
                    </select>
                    <button className="btn btn-ghost" onClick={loadPrinters} title={tr('Volver a buscar impresoras', 'Look for printers again')} aria-label={tr('Volver a buscar impresoras', 'Look for printers again')}>
                      <RefreshCw size={16} />
                    </button>
                    <button className="btn" onClick={detect} title={tr('Elegir sola la impresora de tickets', 'Pick the receipt printer automatically')}>
                      <ScanSearch size={16} />
                      {tr('Detectar', 'Detect')}
                    </button>
                  </div>
                </Field>
              )}
              <p className="field-hint">
                {d.printerKind === 'windows'
                  ? tr(
                      'Impresora de tickets: corta el papel, abre el cajón y usa su juego de caracteres. Si se conecta otra, Tiquet la toma sola.',
                      'Receipt printer: cuts the paper, opens the drawer and uses its character set. If another one is plugged in, Tiquet takes it on its own.',
                    )
                  : doc
                    ? tr(
                        'Se imprime como documento con el driver de Windows: vale cualquier impresora (A4, etiquetas…). Con «Microsoft Print to PDF» cada ticket se guarda en PDF en Documentos\\Tiquet\\PDF.',
                        'Printed as a document through the Windows driver: any printer works (A4, labels…). With "Microsoft Print to PDF" each ticket is saved as a PDF in Documents\\Tiquet\\PDF.',
                      )
                    : network
                      ? tr('Impresora de tickets con su propio cable de red o wifi.', 'A receipt printer on its own network cable or wifi.')
                      : tr('Impresoras de tickets Bluetooth o con cable serie/USB-serie.', 'Bluetooth receipt printers, or on a serial / USB-serial cable.')}
              </p>
              {target === printerLabel(settings) && printerStatus && printerStatus.state !== 'none' && (
                <div className={`printer-pill static ${printerStatus.state}`}>
                  <i aria-hidden="true" />
                  <span>{printerStateText(printerStatus.state)}</span>
                  {(network || serial) && <small>{target}</small>}
                  {printerStatus.jobs > 0 && (
                    <small>
                      {printerStatus.jobs} {tr('trabajos en la cola de Windows', 'jobs in the Windows queue')}
                    </small>
                  )}
                </div>
              )}
              {selectedVirtual && (
                <div className="info-card">
                  <div>
                    <b>{tr('No parece una impresora de tickets', 'This does not look like a receipt printer')}</b>
                    <span className="muted">{tr('Si es una impresora normal o de PDF, imprime los tickets como documento.', 'If it is a normal or PDF printer, print tickets as a document.')}</span>
                  </div>
                  <button className="btn" onClick={() => set('printerKind', 'document')}>
                    {tr('Imprimir como documento', 'Print as a document')}
                  </button>
                </div>
              )}
              <Field label={doc ? tr('Ancho del ticket', 'Ticket width') : tr('Papel', 'Paper')}>
                <Segmented
                  value={d.columns}
                  onChange={(v) => set('columns', v)}
                  options={[
                    { value: 48, label: '80 mm · 48' },
                    { value: 42, label: '80 mm · 42' },
                    { value: 32, label: '58 mm · 32' },
                  ]}
                />
              </Field>
              {doc ? (
                <Field label={tr('Copias por venta', 'Copies per sale')}>
                  <Segmented value={d.copies} onChange={(v) => set('copies', v)} options={[1, 2, 3].map((n) => ({ value: n, label: String(n) }))} />
                </Field>
              ) : (
                <>
                  <Field label={tr('Juego de caracteres', 'Character set')} hint={tr('Si salen símbolos raros en vez de tildes, imprime la prueba de caracteres y elige la línea que se lea bien.', 'If accents print as odd symbols, print the character test and pick the line that reads correctly.')}>
                    <div className="row-gap">
                      <select value={d.codepage} onChange={(e) => set('codepage', e.target.value)} className="grow">
                        {CODEPAGES.map((c) => (
                          <option key={c.id} value={c.id}>
                            {c.label()}
                          </option>
                        ))}
                      </select>
                      <button className="btn" onClick={() => test('charset')} disabled={!target || !!busy}>
                        {tr('Prueba de caracteres', 'Character test')}
                      </button>
                    </div>
                  </Field>
                  <div className="grid2">
                    <Field label={tr('Cajón portamonedas', 'Cash drawer')}>
                      <select value={d.drawer} onChange={(e) => set('drawer', e.target.value as Settings['drawer'])}>
                        <option value="cash">{tr('Abrir al cobrar en efectivo', 'Open on cash sales')}</option>
                        <option value="always">{tr('Abrir en cada venta', 'Open on every sale')}</option>
                        <option value="never">{tr('No abrir', 'Never open')}</option>
                      </select>
                    </Field>
                    <Field label={tr('Copias por venta', 'Copies per sale')}>
                      <Segmented value={d.copies} onChange={(v) => set('copies', v)} options={[1, 2, 3].map((n) => ({ value: n, label: String(n) }))} />
                    </Field>
                  </div>
                  <div className="grid2">
                    <Toggle checked={d.cut} onChange={(v) => set('cut', v)} label={tr('Cortar el papel', 'Cut the paper')} />
                    <Field label={tr('Avance antes del corte', 'Feed before cut')} hint={tr('Líneas en blanco al final', 'Blank lines at the end')}>
                      <input type="number" min={0} max={12} value={d.feedLines} onChange={(e) => set('feedLines', Math.max(0, Math.min(12, Number(e.target.value) || 0)))} />
                    </Field>
                  </div>
                  <Toggle
                    checked={d.cancelChinese}
                    onChange={(v) => set('cancelChinese', v)}
                    label={tr('Desactivar modo chino al imprimir', 'Turn off Chinese mode when printing')}
                    hint={tr('Necesario en la mayoría de impresoras genéricas (salen ideogramas en vez de tildes)', 'Needed by most generic printers (accents print as ideograms otherwise)')}
                  />
                </>
              )}
              <div className="row-gap">
                <button className="btn btn-primary" onClick={() => test('page')} disabled={!target || !!busy}>
                  <Printer size={16} />
                  {tr('Imprimir página de prueba', 'Print test page')}
                </button>
                {!doc && (
                  <button className="btn" onClick={() => test('drawer')} disabled={!target || !!busy}>
                    {tr('Abrir cajón', 'Open drawer')}
                  </button>
                )}
              </div>
              {dirty && <p className="field-hint">{tr('Las pruebas usan estos ajustes aunque aún no estén guardados.', 'Tests use these settings even before you save them.')}</p>}
            </div>
          )}

          {tab === 'mail' && <MailTab />}

          {tab === 'verifactu' && <VerifactuTab d={d} set={set} onShop={() => setTab('shop')} />}

          {tab === 'data' && (
            <div className="stack">
              <Field label={tr('Idioma de la aplicación', 'App language')} hint={tr('El ticket siempre se imprime en español', 'Tickets always print in Spanish')}>
                <Segmented
                  value={lang}
                  onChange={(v) => {
                    onLang(v);
                    set('lang', v);
                  }}
                  options={[
                    { value: 'es', label: 'Español' },
                    { value: 'en', label: 'English' },
                  ]}
                />
              </Field>
              <Toggle
                checked={d.training}
                onChange={(v) => set('training', v)}
                label={tr('Modo prueba', 'Training mode')}
                hint={tr(
                  'Para aprender o enseñar: las ventas llevan su propia numeración, salen marcadas como prueba y no cuentan en informes ni en la caja.',
                  'For learning or teaching: sales get their own numbering, print marked as practice and never count in reports or the till.',
                )}
              />
              {settings.training && (
                <div className="info-card">
                  <div>
                    <b>{tr('Ventas de prueba', 'Practice sales')}</b>
                    <span className="muted">{tr('Se pueden borrar todas al terminar de practicar.', 'They can all be deleted when practice is over.')}</span>
                  </div>
                  <button className="btn btn-danger-ghost" onClick={wipe} disabled={busy === 'wipe'}>
                    {tr('Borrar ventas de prueba', 'Delete practice sales')}
                  </button>
                </div>
              )}
              <div className="grid2">
                <Field
                  label={tr('Al abrir Tiquet', 'When Tiquet opens')}
                  hint={tr('Entrar directamente, sin escribir la contraseña. «Bloquear» la sigue pidiendo.', 'Go straight in, with no password typed. "Lock" still asks for it.')}
                >
                  <select value={d.autoLogin} onChange={(e) => set('autoLogin', Number(e.target.value))}>
                    <option value={0}>{tr('Pedir contraseña', 'Ask for the password')}</option>
                    {(users ?? [])
                      .filter((u) => u.active || u.id === d.autoLogin)
                      .map((u) => (
                        <option key={u.id} value={u.id}>
                          {tr('Entrar como', 'Sign in as')} {u.name}
                        </option>
                      ))}
                  </select>
                </Field>
                <Field
                  label={tr('Bloquear la caja si no se usa', 'Lock the till when not in use')}
                  hint={tr('Mientras alguien use la pantalla no se bloquea nunca.', 'It never locks while someone is using the screen.')}
                >
                  <select value={d.autoLockMin} onChange={(e) => set('autoLockMin', Number(e.target.value))}>
                    {[0, 5, 10, 15, 30, 60, 120, 240].map((m) => (
                      <option key={m} value={m}>
                        {m === 0 ? tr('Nunca', 'Never') : m < 60 ? `${tr('Tras', 'After')} ${m} min` : `${tr('Tras', 'After')} ${m / 60} h`}
                      </option>
                    ))}
                    {![0, 5, 10, 15, 30, 60, 120, 240].includes(d.autoLockMin) && <option value={d.autoLockMin}>{`${tr('Tras', 'After')} ${d.autoLockMin} min`}</option>}
                  </select>
                </Field>
              </div>
              <Toggle
                checked={d.autostart}
                onChange={(v) => set('autostart', v)}
                label={tr('Abrir Tiquet al encender el ordenador', 'Open Tiquet when the computer starts')}
                hint={tr('Se abre solo al entrar en Windows con este usuario.', 'It opens by itself when this user signs in to Windows.')}
              />
              <Toggle
                checked={d.fullscreen}
                onChange={(v) => set('fullscreen', v)}
                label={tr('Pantalla completa', 'Full screen')}
                hint={tr('Tiquet ocupa toda la pantalla, también la barra de Windows. Se sale con el botón de arriba a la derecha.', 'Tiquet fills the whole screen, Windows taskbar included. The button at the top right leaves it.')}
              />
              <Toggle
                checked={d.autoBackup}
                onChange={(v) => set('autoBackup', v)}
                label={tr('Copia de seguridad diaria', 'Daily backup')}
                hint={tr('Una al día, al entrar o con la primera venta. Se guardan las últimas 60.', 'One a day, at sign-in or with the first sale. The last 60 are kept.')}
              />
              <Field label={tr('Carpeta de copias', 'Backup folder')} hint={tr('Vacío = carpeta por defecto. Puede ser un USB o una carpeta de OneDrive.', 'Empty = default folder. Can be a USB drive or a OneDrive folder.')}>
                <input value={d.backupDir} onChange={(e) => set('backupDir', e.target.value)} placeholder={backup?.defaultDir} className="mono small-text" />
              </Field>
              <div className="info-card">
                <div>
                  <span className="muted">{tr('Última copia', 'Last backup')}</span>
                  <b>{backup?.last ? dateTime(backup.last) : tr('Ninguna todavía', 'None yet')}</b>
                  {backup?.lastPath && <span className="mono small-text muted">{backup.lastPath}</span>}
                  {backup?.error && <span className="form-error">{backup.error}</span>}
                </div>
                <div className="info-actions">
                  <button className="btn" onClick={backupNow} disabled={busy === 'backup'}>
                    <HardDrive size={16} />
                    {tr('Hacer copia ahora', 'Back up now')}
                  </button>
                  <button className="btn btn-ghost sm" onClick={() => reveal('backups')}>
                    <FolderOpen size={15} />
                    {tr('Abrir carpeta', 'Open folder')}
                  </button>
                </div>
              </div>

              <div className="info-card">
                <div>
                  <span className="muted">{tr('Versión de Tiquet', 'Tiquet version')}</span>
                  <b>{version}</b>
                  {updates.found ? (
                    <span className="ok-text">{tr(`Hay una versión nueva: ${updates.found}`, `A new version is out: ${updates.found}`)}</span>
                  ) : (
                    updates.upToDate && <span className="muted">{tr('Es la versión más reciente.', 'This is the latest version.')}</span>
                  )}
                  {updates.found && saleBusy && <span className="field-hint">{tr('Termina o vacía el ticket de Venta para actualizar.', 'Finish or clear the ticket in Sale to update.')}</span>}
                </div>
                <div className="info-actions">
                  {updates.found ? (
                    <button className="btn btn-primary" onClick={updates.install} disabled={updates.installing || saleBusy}>
                      <Download size={16} />
                      {updates.installing ? tr('Actualizando…', 'Updating…') : tr('Actualizar ahora', 'Update now')}
                    </button>
                  ) : (
                    <button className="btn" onClick={() => updates.check(true)} disabled={!native || updates.checking}>
                      <RefreshCw size={16} />
                      {updates.checking ? tr('Buscando…', 'Checking…') : tr('Buscar actualización', 'Check for updates')}
                    </button>
                  )}
                </div>
              </div>

              <div className="info-card">
                <div>
                  <b>{tr('Diagnóstico para soporte', 'Diagnostics for support')}</b>
                  <span className="muted">
                    {tr(
                      'Un .zip con el registro de actividad, los ajustes y el estado de Tiquet, para enviarlo a quien te da soporte. No lleva la base de datos ni contraseñas.',
                      'A .zip with the activity log, the settings and the state of Tiquet, to send to whoever gives you support. It holds no database and no passwords.',
                    )}
                  </span>
                </div>
                <div className="info-actions">
                  <button className="btn" onClick={diagnose} disabled={busy === 'diag'}>
                    <LifeBuoy size={16} />
                    {tr('Guardar diagnóstico', 'Save diagnostics')}
                  </button>
                </div>
              </div>

              <div className="field">
                <span className="field-label">{tr('Restaurar una copia', 'Restore a backup')}</span>
                {files && files.length > 0 ? (
                  <div className="backup-list">
                    {files.map((f) => (
                      <div key={f.path} className="backup-row">
                        <div>
                          <b>{dateTime(f.modified)}</b>
                          <span title={f.path}>
                            {f.name} · {size(f.bytes)}
                          </span>
                        </div>
                        <button className="btn btn-ghost sm" onClick={() => setRestoring(f)}>
                          <History size={15} />
                          {tr('Restaurar', 'Restore')}
                        </button>
                      </div>
                    ))}
                  </div>
                ) : (
                  <span className="field-hint">{tr('Todavía no hay copias en la carpeta.', 'There are no backups in the folder yet.')}</span>
                )}
              </div>

              {backup && (
                <>
                  <div className="path-row">
                    <span>
                      {tr('Datos', 'Data')}: <span className="mono">{backup.dataDir}</span>
                    </span>
                    <button className="btn btn-ghost sm" onClick={() => reveal('data')}>
                      <FolderOpen size={15} />
                      {tr('Abrir carpeta', 'Open folder')}
                    </button>
                  </div>
                  {backup.logPath && (
                    <p className="field-hint">
                      {tr('Registro de actividad', 'Activity log')}: <span className="mono">{backup.logPath}</span>
                    </p>
                  )}
                </>
              )}
            </div>
          )}
        </div>

        {dirty && (
          <div className="savebar">
            <span>{tr('Cambios sin guardar', 'Unsaved changes')}</span>
            <div className="row-gap">
              <button className="btn btn-ghost" onClick={() => setD(settings)}>
                {tr('Descartar', 'Discard')}
              </button>
              <button className="btn btn-primary" onClick={save} disabled={busy === 'save'}>
                {tr('Guardar cambios', 'Save changes')}
                <kbd className="on-dark">Ctrl+S</kbd>
              </button>
            </div>
          </div>
        )}
      </section>

      {restoring && (
        <Modal
          title={tr('¿Restaurar esta copia?', 'Restore this backup?')}
          onClose={() => setRestoring(null)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setRestoring(null)}>
                {tr('Cancelar', 'Cancel')}
              </button>
              <button className="btn btn-danger" onClick={restore} disabled={busy === 'restore'}>
                <History size={16} />
                {tr('Restaurar', 'Restore')}
              </button>
            </>
          }
        >
          <p>
            {tr('Los datos actuales se sustituyen por la copia del', 'The current data is replaced by the backup from')} <b>{dateTime(restoring.modified)}</b>.
          </p>
          <p className="muted">
            {tr(
              'Antes se guarda una copia de los datos actuales, por si hay que volver atrás. Después habrá que volver a entrar.',
              'The current data is copied first, in case you need to go back. You will have to sign in again afterwards.',
            )}
          </p>
        </Modal>
      )}

      {restored && (
        <Modal locked title={tr('Copia restaurada', 'Backup restored')} onClose={onRestored} actions={<button className="btn btn-primary" onClick={onRestored}>{tr('Volver a entrar', 'Sign in again')}</button>}>
          {restored.missing.length > 0 && (
            <div className="clock-note">
              <History size={20} />
              <p>
                {tr('La copia no tiene los tickets o reparaciones', 'The backup does not have the tickets or repairs')}{' '}
                {restored.missing.map((m, i) => (
                  <span key={m.first}>
                    {i > 0 && ', '}
                    <b className="mono">
                      {m.first} – {m.last}
                    </b>
                  </span>
                ))}
                .{' '}
                {tr(
                  'Ya se entregaron, así que la numeración sigue después del último para no repetir ninguno.',
                  'They were already handed out, so numbering carries on after the last one so none is repeated.',
                )}
              </p>
            </div>
          )}
          <p className="muted">
            {tr('Los datos de antes de restaurar están en', 'The data from before the restore is in')} <span className="mono small-text">{restored.safetyCopy}</span>
          </p>
        </Modal>
      )}

      <aside className="side settings-side">
        <div className="side-head">
          <span>{tr('Ticket de ejemplo', 'Sample ticket')}</span>
        </div>
        <div className="paper-scroll">{preview && <Paper lines={preview} logo={logo} cols={d.columns} />}</div>
      </aside>
    </div>
  );
}

const MAIL_PRESETS: { label: string; host: string; port: number; security: MailConfig['security'] }[] = [
  { label: 'Gmail', host: 'smtp.gmail.com', port: 587, security: 'starttls' },
  { label: 'Outlook / Microsoft 365', host: 'smtp.office365.com', port: 587, security: 'starttls' },
  { label: 'Yahoo', host: 'smtp.mail.yahoo.com', port: 465, security: 'tls' },
  { label: 'IONOS', host: 'smtp.ionos.es', port: 587, security: 'starttls' },
];

// The shop's own mail account, so tickets go out by email straight from
// Tiquet with the PDF attached. Saved apart from the other settings: the
// password is encrypted for this Windows user and never comes back here.
function MailTab() {
  const toast = useToast();
  const [cfg, setCfg] = useState<MailConfig | null>(null);
  const [hasPassword, setHasPassword] = useState(false);
  const [pass, setPass] = useState('');
  const [testTo, setTestTo] = useState('');
  const [busy, setBusy] = useState('');

  useEffect(() => {
    call<{ config: MailConfig; hasPassword: boolean }>('email.get')
      .then((r) => {
        setCfg(r.config);
        setHasPassword(r.hasPassword);
        setTestTo(r.config.from || r.config.user);
      })
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  if (!cfg) return null;
  const put = <K extends keyof MailConfig>(k: K, v: MailConfig[K]) => setCfg({ ...cfg, [k]: v });

  const store = async () => {
    const r = await call<{ config: MailConfig; hasPassword: boolean }>('email.save', { config: cfg, password: pass || undefined });
    setCfg(r.config);
    setHasPassword(r.hasPassword);
    setPass('');
    if (!testTo) setTestTo(r.config.from || r.config.user);
  };

  const run = async (what: string, f: () => Promise<void>) => {
    setBusy(what);
    try {
      await f();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
    }
  };

  const save = () => run('save', async () => {
    await store();
    toast.ok(tr('Correo guardado', 'Mail saved'));
  });

  const test = () => run('test', async () => {
    await store();
    await call('email.test', { to: testTo });
    toast.ok(`${tr('Correo de prueba enviado a', 'Test email sent to')} ${testTo}`);
  });

  return (
    <div className="stack">
      <p className="muted">
        {tr(
          'Con el correo de la tienda configurado, «Enviar» en un ticket lo manda por email con el PDF adjunto, sin abrir ningún otro programa. Sin configurar, se abre el programa de correo del equipo.',
          'With the shop’s mail set up, "Send" on a ticket emails it with the PDF attached, with no other program opened. Without it, the computer’s mail app opens.',
        )}
      </p>
      <Field label={tr('Tu proveedor de correo', 'Your mail provider')}>
        <div className="row-gap">
          {MAIL_PRESETS.map((p) => (
            <button key={p.label} className={`btn${cfg.host === p.host ? ' btn-primary' : ''}`} onClick={() => setCfg({ ...cfg, host: p.host, port: p.port, security: p.security })}>
              {p.label}
            </button>
          ))}
        </div>
      </Field>
      <div className="grid-host">
        <Field label={tr('Servidor de salida (SMTP)', 'Outgoing server (SMTP)')}>
          <input value={cfg.host} onChange={(e) => put('host', e.target.value.trim())} placeholder="smtp.gmail.com" className="mono" spellCheck={false} />
        </Field>
        <Field label={tr('Puerto', 'Port')}>
          <input type="number" min={1} max={65535} value={cfg.port} onChange={(e) => put('port', Math.max(1, Math.min(65535, Math.round(Number(e.target.value)) || 587)))} />
        </Field>
      </div>
      <Field label={tr('Seguridad', 'Security')}>
        <Segmented
          value={cfg.security}
          onChange={(v) => put('security', v)}
          options={[
            { value: 'starttls', label: 'STARTTLS · 587' },
            { value: 'tls', label: 'SSL/TLS · 465' },
            { value: 'none', label: tr('Ninguna', 'None') },
          ]}
        />
      </Field>
      <div className="grid2">
        <Field label={tr('Usuario (el email de la tienda)', 'User (the shop’s email)')}>
          <input value={cfg.user} onChange={(e) => put('user', e.target.value.trim())} placeholder="tienda@gmail.com" spellCheck={false} autoComplete="off" />
        </Field>
        <Field
          label={tr('Contraseña', 'Password')}
          hint={
            hasPassword
              ? tr('Guardada. Escribe otra solo para cambiarla.', 'Saved. Type another only to change it.')
              : tr('Gmail y Outlook piden una «contraseña de aplicación» (se crea en la seguridad de la cuenta, con la verificación en dos pasos activada).', 'Gmail and Outlook need an "app password" (made in the account’s security page, with two-step verification on).')
          }
        >
          <input type="password" value={pass} onChange={(e) => setPass(e.target.value)} placeholder={hasPassword ? '••••••••' : ''} autoComplete="new-password" />
        </Field>
      </div>
      <Field label={tr('Remitente (opcional)', 'Sender (optional)')} hint={tr('Solo si los clientes deben ver otra dirección', 'Only if customers should see another address')}>
        <input value={cfg.from} onChange={(e) => put('from', e.target.value.trim())} placeholder={cfg.user || 'tienda@gmail.com'} spellCheck={false} />
      </Field>
      <div className="row-gap">
        <button className="btn btn-primary" onClick={save} disabled={!!busy}>
          {busy === 'save' ? tr('Guardando…', 'Saving…') : tr('Guardar correo', 'Save mail')}
        </button>
        <input className="grow" type="email" value={testTo} onChange={(e) => setTestTo(e.target.value)} placeholder={tr('email para la prueba', 'email for the test')} />
        <button className="btn" onClick={test} disabled={!!busy || !cfg.host || !cfg.user || !testTo.trim() || (!hasPassword && !pass)}>
          <Mail size={16} />
          {busy === 'test' ? tr('Enviando…', 'Sending…') : tr('Enviar prueba', 'Send a test')}
        </button>
      </div>
    </div>
  );
}
