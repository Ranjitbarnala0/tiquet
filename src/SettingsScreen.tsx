import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { HardDrive, ImagePlus, Printer, ReceiptText, RefreshCw, Store, Trash2 } from 'lucide-react';
import { call } from './api';
import { dateTime, rate } from './format';
import { errorText, tr, type Lang } from './i18n';
import { loadImage, rasterize, type LogoMode } from './logo';
import { Paper } from './Paper';
import { useToast } from './toast';
import type { Logo, PLine, Preview, Settings } from './types';
import { Field, Segmented, Toggle } from './ui';

type Tab = 'shop' | 'ticket' | 'printer' | 'data';

interface BackupStatus {
  dir: string;
  defaultDir: string;
  last: string | null;
  lastPath: string | null;
  error: string | null;
  dataDir: string;
}

const CODEPAGES = [
  { id: 'cp858', label: () => tr('PC858 · Europa con € (recomendado)', 'PC858 · Western Europe with € (recommended)') },
  { id: 'cp1252', label: () => 'Windows-1252' },
  { id: 'cp850', label: () => tr('PC850 · sin símbolo €', 'PC850 · no € sign') },
  { id: 'ascii', label: () => tr('Sin acentos (máxima compatibilidad)', 'No accents (works everywhere)') },
];

interface Props {
  settings: Settings;
  onSaved: (s: Settings) => void;
  logo: Logo | null;
  onLogo: () => void;
  lang: Lang;
  onLang: (l: Lang) => void;
}

export function SettingsScreen({ settings, onSaved, logo, onLogo, lang, onLang }: Props) {
  const toast = useToast();
  const [d, setD] = useState<Settings>(settings);
  const [tab, setTab] = useState<Tab>('shop');
  const [preview, setPreview] = useState<PLine[] | null>(null);
  const [printers, setPrinters] = useState<{ printers: string[]; default: string | null } | null>(null);
  const [backup, setBackup] = useState<BackupStatus | null>(null);
  const [busy, setBusy] = useState('');
  const [img, setImg] = useState<HTMLImageElement | null>(null);
  const [logoSize, setLogoSize] = useState(55);
  const [logoMode, setLogoMode] = useState<LogoMode>('logo');
  const [logoLight, setLogoLight] = useState(0);
  const fileRef = useRef<HTMLInputElement>(null);

  const dirty = JSON.stringify(d) !== JSON.stringify(settings);
  const set = <K extends keyof Settings>(k: K, v: Settings[K]) => setD((prev) => ({ ...prev, [k]: v }));

  useEffect(() => {
    const t = setTimeout(() => {
      call<Preview>('sale.preview', { sample: true, settings: d })
        .then((p) => setPreview(p.lines))
        .catch((e) => toast.error(errorText(e)));
    }, 120);
    return () => clearTimeout(t);
  }, [d, logo, toast]);

  const loadPrinters = useCallback(() => {
    call<{ printers: string[]; default: string | null }>('printers.list')
      .then(setPrinters)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  const loadBackup = useCallback(() => {
    call<BackupStatus>('backup.status')
      .then(setBackup)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

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
      await call('printers.test', { settings: d, kind });
      toast.ok(kind === 'drawer' ? tr('Orden de apertura enviada', 'Open command sent') : tr('Prueba enviada a la impresora', 'Test sent to the printer'));
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy('');
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

  const tabs: { id: Tab; label: string; icon: typeof Store }[] = [
    { id: 'shop', label: tr('Comercio', 'Shop'), icon: Store },
    { id: 'ticket', label: tr('Ticket', 'Ticket'), icon: ReceiptText },
    { id: 'printer', label: tr('Impresora', 'Printer'), icon: Printer },
    { id: 'data', label: tr('Datos y copias', 'Data & backups'), icon: HardDrive },
  ];

  const printerOptions = printers ? [...new Set([...(d.printerName ? [d.printerName] : []), ...printers.printers])] : [];

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
              <Field label={tr('Dirección', 'Address')} hint={tr('Cada renglón es una línea del ticket', 'Each line here is one ticket line')}>
                <textarea rows={3} value={d.shopAddress} onChange={(e) => set('shopAddress', e.target.value)} />
              </Field>
              <div className="grid2">
                <Field label={tr('Teléfono', 'Phone')}>
                  <input value={d.shopPhone} onChange={(e) => set('shopPhone', e.target.value)} />
                </Field>
                <Field label="NIF / CIF">
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
                <Field label={tr('Prefijo de numeración', 'Numbering prefix')} hint={`${tr('Ejemplo', 'Example')}: ${d.ticketPrefix || 'T'}${new Date().getFullYear()}-000001`}>
                  <input value={d.ticketPrefix} onChange={(e) => set('ticketPrefix', e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 6))} />
                </Field>
                <Field label={tr('IVA por defecto', 'Default VAT')} hint={tr('Para artículos libres y productos nuevos', 'For custom items and new products')}>
                  <select value={d.defaultVatBp} onChange={(e) => set('defaultVatBp', Number(e.target.value))}>
                    {[2100, 1000, 400, 0].map((r) => (
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
              <Field label={tr('Impresora de tickets', 'Receipt printer')} hint={tr('La impresora tiene que estar instalada en Windows', 'The printer must be installed in Windows')}>
                <div className="row-gap">
                  <select value={d.printerName} onChange={(e) => set('printerName', e.target.value)} className="grow">
                    <option value="">{tr('Elige una impresora…', 'Choose a printer…')}</option>
                    {printerOptions.map((p) => (
                      <option key={p} value={p}>
                        {p}
                        {printers && !printers.printers.includes(p) ? ` (${tr('no encontrada', 'not found')})` : ''}
                        {printers?.default === p ? ` (${tr('predeterminada', 'default')})` : ''}
                      </option>
                    ))}
                  </select>
                  <button className="btn btn-ghost" onClick={loadPrinters} title={tr('Volver a buscar impresoras', 'Look for printers again')}>
                    <RefreshCw size={16} />
                  </button>
                </div>
              </Field>
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
                  <button className="btn" onClick={() => test('charset')} disabled={!d.printerName || !!busy}>
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
                <button className="btn btn-primary" onClick={() => test('page')} disabled={!d.printerName || !!busy}>
                  <Printer size={16} />
                  {tr('Imprimir página de prueba', 'Print test page')}
                </button>
                <button className="btn" onClick={() => test('drawer')} disabled={!d.printerName || !!busy}>
                  {tr('Abrir cajón', 'Open drawer')}
                </button>
              </div>
              {dirty && <p className="field-hint">{tr('Las pruebas usan estos ajustes aunque aún no estén guardados.', 'Tests use these settings even before you save them.')}</p>}
            </div>
          )}

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
              <Toggle checked={d.autoBackup} onChange={(v) => set('autoBackup', v)} label={tr('Copia de seguridad diaria', 'Daily backup')} hint={tr('Al primer inicio de sesión de cada día. Se guardan las últimas 60.', 'On the first sign-in of each day. The last 60 are kept.')} />
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
                <button className="btn" onClick={backupNow} disabled={busy === 'backup'}>
                  <HardDrive size={16} />
                  {tr('Hacer copia ahora', 'Back up now')}
                </button>
              </div>
              <p className="field-hint">
                {tr('Base de datos', 'Database')}: <span className="mono">{backup?.dataDir}</span>
              </p>
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

      <aside className="side settings-side">
        <div className="side-head">
          <span>{tr('Ticket de ejemplo', 'Sample ticket')}</span>
        </div>
        <div className="paper-scroll">{preview && <Paper lines={preview} logo={logo} cols={d.columns} />}</div>
      </aside>
    </div>
  );
}
