import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { BadgeCheck, Download, FolderOpen, HardDrive, History, ImagePlus, LifeBuoy, Printer, ReceiptText, RefreshCw, ScanSearch, Store, Trash2 } from 'lucide-react';
import { call, native } from './api';
import { dateTime, rate, VAT_RATES } from './format';
import { errorText, printerStateText, tr, warningText, type Lang } from './i18n';
import { loadImage, rasterize, type LogoMode } from './logo';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { SettingsTab } from './Shell';
import type { Logo, PLine, PrinterEntry, Preview, Settings } from './types';
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
    call<{ printers: PrinterEntry[] }>('printers.list')
      .then((r) => setPrinters(r.printers))
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

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
    { id: 'data', label: tr('Datos y copias', 'Data & backups'), icon: HardDrive },
    { id: 'verifactu', label: 'VERI*FACTU', icon: BadgeCheck },
  ];

  const selected = printers?.find((p) => p.name === d.printerName);
  const selectedVirtual = d.printerKind === 'windows' && !!selected?.virtual;
  const network = d.printerKind === 'network';
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
                  label="NIF / CIF"
                  error={
                    !d.shopNif.trim()
                      ? tr('Sin NIF los tickets no son válidos', 'Without a NIF the tickets are not valid')
                      : !spanishNif(d.shopNif)
                        ? tr('Revisa el NIF: la letra no coincide', 'Check the NIF: the letter does not match')
                        : undefined
                  }
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
            </div>
          )}

          {tab === 'printer' && (
            <div className="stack">
              <Field label={tr('Tipo de impresora', 'Printer type')}>
                <Segmented
                  value={d.printerKind}
                  onChange={(v) => set('printerKind', v)}
                  options={[
                    { value: 'windows', label: tr('Impresora de Windows', 'Windows printer') },
                    { value: 'network', label: tr('Impresora de red (IP)', 'Network printer (IP)') },
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
              ) : (
                <Field label={tr('Impresora de tickets', 'Receipt printer')} hint={tr('La impresora tiene que estar instalada en Windows', 'The printer must be installed in Windows')}>
                  <div className="row-gap">
                    <select value={d.printerName} onChange={(e) => set('printerName', e.target.value)} className="grow">
                      <option value="">{tr('Ninguna (guardar tickets sin imprimir)', 'None (save tickets without printing)')}</option>
                      {d.printerName && printers && !selected && (
                        <option value={d.printerName}>
                          {d.printerName} ({tr('no encontrada', 'not found')})
                        </option>
                      )}
                      {printers?.map((p) => (
                        <option key={p.name} value={p.name} disabled={p.virtual && p.name !== d.printerName}>
                          {p.name}
                          {p.virtual ? ` (${tr('virtual, no imprime tickets', 'virtual, cannot print tickets')})` : ''}
                          {p.default && !p.virtual ? ` (${tr('predeterminada', 'default')})` : ''}
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
              {target === printerLabel(settings) && printerStatus && printerStatus.state !== 'none' && (
                <div className={`printer-pill static ${printerStatus.state}`}>
                  <i aria-hidden="true" />
                  <span>{printerStateText(printerStatus.state)}</span>
                  {network && <small>{target}</small>}
                  {printerStatus.jobs > 0 && (
                    <small>
                      {printerStatus.jobs} {tr('trabajos en la cola de Windows', 'jobs in the Windows queue')}
                    </small>
                  )}
                </div>
              )}
              {selectedVirtual && (
                <p className="form-error">
                  {tr(
                    'Esta es una impresora virtual (PDF, XPS…): no imprime tickets. Elige la impresora de tickets.',
                    'This is a virtual printer (PDF, XPS…): it cannot print tickets. Choose the receipt printer.',
                  )}
                </p>
              )}
              <Field label={tr('Papel', 'Paper')}>
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
              <div className="row-gap">
                <button className="btn btn-primary" onClick={() => test('page')} disabled={!target || !!busy}>
                  <Printer size={16} />
                  {tr('Imprimir página de prueba', 'Print test page')}
                </button>
                <button className="btn" onClick={() => test('drawer')} disabled={!target || !!busy}>
                  {tr('Abrir cajón', 'Open drawer')}
                </button>
              </div>
              {dirty && <p className="field-hint">{tr('Las pruebas usan estos ajustes aunque aún no estén guardados.', 'Tests use these settings even before you save them.')}</p>}
            </div>
          )}

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
              <Field
                label={tr('Bloquear la caja si no se usa', 'Lock the till when not in use')}
                hint={tr('Después hay que volver a escribir la contraseña. Evita que otra persona use una sesión de administrador abierta.', 'The password is needed again afterwards. Stops someone else using an administrator session left open.')}
              >
                <select value={d.autoLockMin} onChange={(e) => set('autoLockMin', Number(e.target.value))}>
                  {[0, 2, 5, 10, 15, 30, 60].map((m) => (
                    <option key={m} value={m}>
                      {m === 0 ? tr('Nunca', 'Never') : `${tr('Tras', 'After')} ${m} min`}
                    </option>
                  ))}
                </select>
              </Field>
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
