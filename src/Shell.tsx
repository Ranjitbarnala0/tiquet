import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { BadgeCheck, CalendarDays, FlaskConical, Lock, Maximize2, Minimize2, Package, Settings as SettingsIcon, ShoppingBag, TriangleAlert, Wrench } from 'lucide-react';
import { call, native } from './api';
import { Day, type DayView } from './Day';
import { initials } from './format';
import { errorText, tr, type Lang } from './i18n';
import { Products } from './Products';
import { Repairs } from './Repairs';
import { Sale } from './Sale';
import { SettingsScreen } from './SettingsScreen';
import { useToast } from './toast';
import type { ItemDraft } from './ItemDialog';
import type { Logo, Product, Settings, TicketRecord, User, VerifactuStatus } from './types';
import { Mark, Modal } from './ui';
import { UpdateBar, useUpdate } from './Update';
import { sendError, stuck } from './Verifactu';

type Screen = 'sale' | 'day' | 'repairs' | 'products' | 'settings';
export type SettingsTab = 'shop' | 'sale' | 'machines' | 'ticket' | 'printer' | 'mail' | 'users' | 'data' | 'verifactu';

const VERSION_KEY = 'tiquet.version';

interface Props {
  installId: string;
  version: string;
  user: User;
  setUser: (u: User) => void;
  lang: Lang;
  onLang: (l: Lang) => void;
  onLogout: () => void;
}

export function Shell({ installId, version, user, setUser, lang, onLang, onLogout }: Props) {
  const toast = useToast();
  const [screen, setScreen] = useState<Screen>('sale');
  const [tabRequest, setTabRequest] = useState<{ tab: SettingsTab; n: number }>({ tab: 'shop', n: 0 });
  const [dayRequest] = useState<{ view: DayView; n: number }>({ view: 'moves', n: 0 });
  const [settings, setSettings] = useState<Settings | null>(null);
  const [logo, setLogo] = useState<Logo | null>(null);
  const [products, setProducts] = useState<Product[]>([]);
  const [loadErr, setLoadErr] = useState('');
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [leave, setLeave] = useState<null | (() => void)>(null);
  const [incoming, setIncoming] = useState<{ lines: ItemDraft[]; from: string; n: number; replaces?: { id: number; number: string } } | null>(null);
  const [vf, setVf] = useState<VerifactuStatus | null>(null);
  // a ticket on the screen or being charged: no update is offered then
  const [saleBusy, setSaleBusy] = useState(false);
  // whoever is signed in carries on after the update, with no password to type
  const keepSession = useCallback(() => call('session.keep'), []);
  const updates = useUpdate(saleBusy || settingsDirty, { auto: !!settings?.autoUpdate, before: keepSession });
  const [full, setFull] = useState(false);
  const admin = user.role === 'admin';

  const loadProducts = useCallback(() => {
    call<Product[]>('products.list')
      .then(setProducts)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  const loadLogo = useCallback(() => {
    call<Logo | null>('logo.get')
      .then(setLogo)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  const loadSettings = useCallback(() => {
    setLoadErr('');
    call<Settings>('settings.get')
      .then(setSettings)
      .catch((e) => setLoadErr(errorText(e)));
  }, []);

  useEffect(() => {
    loadSettings();
    loadLogo();
    loadProducts();
  }, [loadSettings, loadLogo, loadProducts]);

  // Tiquet updates itself: the first screen after one says so
  useEffect(() => {
    try {
      const was = localStorage.getItem(VERSION_KEY);
      localStorage.setItem(VERSION_KEY, version);
      if (was && was !== version) toast.ok(`${tr('Tiquet se ha actualizado a la versión', 'Tiquet was updated to version')} ${version}`);
    } catch {
      // it just will not say so
    }
  }, [version, toast]);

  // Only when chosen in Ajustes: the screen locks itself too, so a till left
  // alone does not stay open on the last admin's session. The core keeps the
  // same limit and hears about every touch, key and click, so someone using
  // the screen is never asked for the password.
  const lockMin = settings?.autoLockMin ?? 0;
  useEffect(() => {
    if (!lockMin) return;
    let last = Date.now();
    let told = Date.now();
    const seen = () => {
      last = Date.now();
      if (last - told > 30_000) {
        told = last;
        call('session.ping').catch(() => {});
      }
    };
    const kinds = ['keydown', 'mousedown', 'wheel', 'touchstart', 'pointermove'];
    kinds.forEach((k) => window.addEventListener(k, seen, true));
    const t = setInterval(() => {
      if (Date.now() - last < lockMin * 60_000) return;
      toast.info(tr('La caja se bloqueó porque nadie la usaba.', 'The till locked because nobody was using it.'));
      onLogout();
    }, 15_000);
    return () => {
      kinds.forEach((k) => window.removeEventListener(k, seen, true));
      clearInterval(t);
    };
  }, [lockMin, onLogout, toast]);

  // unsaved Ajustes: the core holds the window open and asks us first
  useEffect(() => {
    if ('__TAURI_INTERNALS__' in window) call('app.dirty', { dirty: settingsDirty }).catch(() => {});
  }, [settingsDirty]);

  useEffect(() => {
    if (!('__TAURI_INTERNALS__' in window)) return;
    let off: (() => void) | undefined;
    listen('tiquet://close-requested', () => setLeave(() => () => call('app.exit').catch(() => {}))).then((u) => (off = u));
    return () => off?.();
  }, []);

  useEffect(() => {
    if (!admin && !['sale', 'day', 'repairs'].includes(screen)) setScreen('sale');
  }, [admin, screen]);

  // the core opened the window full screen if Ajustes says so; only the
  // button in the header leaves it, Esc never does
  const fullscreenSet = !!settings?.fullscreen;
  useEffect(() => {
    if (!native || !fullscreenSet) return;
    call<{ on: boolean }>('app.fullscreen')
      .then((r) => setFull(r.on))
      .catch(() => {});
  }, [fullscreenSet]);

  const fullscreen = useCallback(
    (on: boolean) =>
      call<{ on: boolean }>('app.fullscreen', { on })
        .then((r) => setFull(r.on))
        .catch((e) => toast.error(errorText(e))),
    [toast],
  );

  // records that cannot reach the AEAT are shown to whoever is at the till
  // (OM HAC/1177/2024 art. 16); selling goes on regardless
  const vfOn = !!settings && settings.verifactu !== 'off';
  useEffect(() => {
    if (!vfOn) {
      setVf(null);
      return;
    }
    const load = () =>
      call<VerifactuStatus>('verifactu.status')
        .then(setVf)
        .catch(() => {});
    load();
    const t = setInterval(load, 60_000);
    return () => clearInterval(t);
  }, [vfOn]);

  // a backup that failed is told to the admin at sign-in, not left in Ajustes
  useEffect(() => {
    if (!admin) return;
    call<{ error: string | null }>('backup.status')
      .then((b) => {
        if (b.error) {
          toast.error(`${tr('La última copia de seguridad falló', 'The last backup failed')}: ${b.error}`, {
            sticky: true,
            action: {
              label: tr('Ver', 'View'),
              run: () => {
                setTabRequest((r) => ({ tab: 'data', n: r.n + 1 }));
                setScreen('settings');
              },
            },
          });
        }
      })
      .catch(() => {});
  }, [admin, toast]);

  // Plug and print: with no printer set up, or the one set up gone (another
  // USB port renames it), a receipt printer that appears is taken on its own,
  // checked now and every few seconds. Ajustes can change it any time.
  useEffect(() => {
    const look = () =>
      call<{ applied: boolean; paper?: boolean; name?: string; columns?: number; replaced?: string | null; settings?: Settings }>('printers.auto')
        .then((r) => {
          // the core has already saved it: shown even when this effect ran
          // again while the answer was on its way, or it would never show
          if (!r.applied || !r.settings) return;
          const s = r.settings;
          setSettings(s);
          const mm = s.columns === 32 ? '58' : '80';
          const paper = ` · ${tr('papel', 'paper')} ${mm} mm`;
          const text = r.paper
            ? `${tr('Tickets ajustados al papel de', 'Tickets now fit the paper of')} ${r.name}: ${mm} mm, ${s.columns} ${tr('caracteres por línea', 'characters a line')}`
            : r.replaced
              ? `${tr('La impresora', 'The printer')} «${r.replaced}» ${tr('ya no está; ahora se imprime en', 'is gone; tickets now print on')} ${r.name}${paper}`
              : `${tr('Impresora de tickets conectada', 'Receipt printer connected')}: ${r.name}${paper}`;
          toast.ok(text, {
            sticky: true,
            action: admin
              ? {
                  label: tr('Imprimir prueba', 'Print a test'),
                  run: () => call('printers.test', { settings: s, kind: 'page' }).catch((e) => toast.error(errorText(e))),
                }
              : undefined,
          });
        })
        .catch(() => {});
    look();
    const t = setInterval(look, 20_000);
    return () => clearInterval(t);
  }, [admin, toast]);

  // installs from before recovery codes existed have none until an admin makes one
  useEffect(() => {
    if (!admin) return;
    call<{ set: boolean }>('recovery.status')
      .then((r) => {
        if (!r.set) {
          toast.info(tr('No hay código de recuperación: si se olvida la contraseña de administrador no habrá forma de entrar.', 'There is no recovery code: a forgotten administrator password would lock everyone out.'), {
            sticky: true,
            action: {
              label: tr('Crear', 'Create'),
              run: () => {
                setTabRequest((r) => ({ tab: 'users', n: r.n + 1 }));
                setScreen('settings');
              },
            },
          });
        }
      })
      .catch(() => {});
  }, [admin, toast]);

  if (!settings) {
    if (!loadErr) return <div className="splash" />;
    return (
      <div className="fatal">
        <TriangleAlert size={34} />
        <h2>{tr('No se pudieron cargar los ajustes', 'Settings could not be loaded')}</h2>
        <p>{loadErr}</p>
        <div className="fatal-actions">
          <button className="btn btn-primary" onClick={loadSettings}>
            {tr('Reintentar', 'Try again')}
          </button>
          <button className="btn" onClick={onLogout}>
            <Lock size={16} />
            {tr('Bloquear', 'Lock')}
          </button>
        </div>
      </div>
    );
  }

  // leaving Ajustes with changes not saved asks first
  const guard = (then: () => void) => {
    if (screen === 'settings' && settingsDirty) setLeave(() => then);
    else then();
  };
  const go = (to: Screen, tab?: SettingsTab) => {
    const move = () => {
      if (tab) setTabRequest((r) => ({ tab, n: r.n + 1 }));
      setScreen(to);
    };
    if (to === 'settings') move();
    else guard(move);
  };
  const setupPrinter = admin ? () => go('settings', 'printer') : undefined;

  const nav: { id: Screen; label: string; icon: typeof ShoppingBag }[] = [
    { id: 'sale', label: tr('Venta', 'Sale'), icon: ShoppingBag },
    { id: 'day', label: tr('Hoy', 'Today'), icon: CalendarDays },
  ];
  if (settings.showRepairs) nav.push({ id: 'repairs', label: tr('Reparaciones', 'Repairs'), icon: Wrench });
  if (admin) {
    nav.push({ id: 'products', label: tr('Productos', 'Products'), icon: Package }, { id: 'settings', label: tr('Ajustes', 'Settings'), icon: SettingsIcon });
  }

  return (
    <div className={`shell${settings.training ? ' practice' : ''}`}>
      <aside className="rail">
        <div className="rail-brand" title={settings.shopName || 'Tiquet'}>
          <Mark size={38} />
        </div>
        <nav className="rail-nav">
          {nav.map((n) => (
            <button key={n.id} className={screen === n.id ? 'on' : ''} onClick={() => go(n.id)} aria-current={screen === n.id ? 'page' : undefined}>
              <n.icon size={22} />
              <span>{n.label}</span>
            </button>
          ))}
        </nav>
        <div className="rail-foot">
          {native && settings.fullscreen && (
            <button
              className="rail-tool"
              onClick={() => fullscreen(!full)}
              title={full ? tr('Salir de pantalla completa', 'Leave full screen') : tr('Pantalla completa', 'Full screen')}
              aria-label={full ? tr('Salir de pantalla completa', 'Leave full screen') : tr('Pantalla completa', 'Full screen')}
            >
              {full ? <Minimize2 size={18} /> : <Maximize2 size={18} />}
            </button>
          )}
          <button className="rail-me" onClick={() => guard(onLogout)} title={`${user.name} · ${tr('Bloquear y cambiar de usuario', 'Lock and switch user')}`}>
            <span className="avatar sm">{initials(user.name)}</span>
            <Lock size={13} />
          </button>
        </div>
      </aside>
      <div className="stage">
        <div className="bars">
          <UpdateBar u={updates} saleBusy={saleBusy} />
          {vf && stuck(vf) && (
            <div className={`vf-bar${vf.problems.length > 0 ? ' bad' : ''}`} role="status">
              <BadgeCheck size={16} />
              <span>
                {vf.problems.length > 0
                  ? tr(`VERI*FACTU: la AEAT pide corregir ${vf.problems.length} registro(s).`, `VERI*FACTU: the AEAT asks for ${vf.problems.length} record(s) to be corrected.`)
                  : tr(`VERI*FACTU: ${vf.pending} registro(s) sin enviar a la AEAT.`, `VERI*FACTU: ${vf.pending} record(s) not sent to the AEAT yet.`)}{' '}
                {sendError(vf.lastError)?.text}
              </span>
              {admin && (
                <button className="btn btn-ghost sm" onClick={() => go('settings', 'verifactu')}>
                  {tr('Ver', 'View')}
                </button>
              )}
            </div>
          )}
          {settings.training && (
            <div className="training-bar" role="status">
              <FlaskConical size={16} />
              <span>{tr('Modo prueba: las ventas no son reales, salen marcadas y no cuentan en ningún informe.', 'Training mode: sales are not real, print marked and never count in any report.')}</span>
              {admin && (
                <button
                  className="btn btn-ghost sm"
                  onClick={() =>
                    call<Settings>('settings.save', { training: false })
                      .then(setSettings)
                      .catch((e) => toast.error(errorText(e)))
                  }
                >
                  {tr('Salir del modo prueba', 'Leave training mode')}
                </button>
              )}
            </div>
          )}
        </div>
        <main className="main">
          <Sale
            cartKey={`tiquet.cart.${installId}.${user.id}`}
            active={screen === 'sale'}
            admin={admin}
            incoming={incoming}
            onCatalogue={loadProducts}
            settings={settings}
            logo={logo}
            products={products}
            onSold={loadProducts}
            onSetupPrinter={setupPrinter}
            onBusy={setSaleBusy}
            onDay={() => go('day')}
          />
          {screen === 'day' && (
            <Day
              user={user}
              settings={settings}
              logo={logo}
              request={dayRequest}
              onSetupPrinter={setupPrinter}
              onRepeat={(t: TicketRecord, correct: boolean) => {
                const lines = t.lines
                  .filter((l) => l.qty > 0 && !l.repairId)
                  .map((l) => ({ name: l.name, qty: l.qty, unitCents: l.unitCents, vatBp: l.vatBp, productId: l.productId ?? undefined }));
                setIncoming((cur) => ({ lines, from: t.number, n: (cur?.n ?? 0) + 1, replaces: correct ? { id: t.id, number: t.number } : undefined }));
                setScreen('sale');
              }}
            />
          )}
          {screen === 'repairs' && (
            <Repairs
              user={user}
              settings={settings}
              logo={logo}
              onSetupPrinter={setupPrinter}
              onCharge={(line, number) => {
                setIncoming((cur) => ({ lines: [line], from: number, n: (cur?.n ?? 0) + 1 }));
                setScreen('sale');
              }}
            />
          )}
          {screen === 'products' && admin && <Products settings={settings} onChanged={loadProducts} />}
          {screen === 'settings' && admin && (
            <SettingsScreen
              onRestored={onLogout}
              onDirty={setSettingsDirty}
              request={tabRequest}
              settings={settings}
              onSaved={(s) => {
                if (native && s.fullscreen !== settings.fullscreen) fullscreen(s.fullscreen);
                setSettings(s);
              }}
              version={version}
              updates={updates}
              saleBusy={saleBusy}
              logo={logo}
              onLogo={loadLogo}
              lang={lang}
              onLang={onLang}
              me={user}
              onMe={setUser}
            />
          )}
        </main>
      </div>
      {leave && (
        <Modal
          title={tr('Hay cambios sin guardar', 'There are unsaved changes')}
          onClose={() => setLeave(null)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setLeave(null)}>
                {tr('Seguir editando', 'Keep editing')}
              </button>
              <button
                className="btn btn-danger"
                onClick={() => {
                  const then = leave;
                  setLeave(null);
                  setSettingsDirty(false);
                  then();
                }}
              >
                {tr('Descartar cambios', 'Discard changes')}
              </button>
            </>
          }
        >
          <p className="muted">{tr('Los cambios de Ajustes se perderán si sales sin guardarlos.', 'The changes in Settings are lost if you leave without saving them.')}</p>
        </Modal>
      )}
    </div>
  );
}
