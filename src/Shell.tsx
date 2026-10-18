import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { BadgeCheck, ChartColumn, FlaskConical, Lock, Maximize2, Minimize2, Package, ReceiptText, Settings as SettingsIcon, ShoppingBag, TriangleAlert, Users as UsersIcon, Wallet } from 'lucide-react';
import { call, native } from './api';
import { Cash } from './Cash';
import { initials } from './format';
import { errorText, tr, type Lang } from './i18n';
import { Products } from './Products';
import { Reports } from './Reports';
import { Sale } from './Sale';
import { SettingsScreen } from './SettingsScreen';
import { Tickets } from './Tickets';
import { useToast } from './toast';
import type { ItemDraft } from './ItemDialog';
import type { Logo, Product, Settings, TicketRecord, User, VerifactuStatus } from './types';
import { Mark, Modal } from './ui';
import { UpdateBar, useUpdate } from './Update';
import { Users } from './Users';
import { sendError, stuck } from './Verifactu';

type Screen = 'sale' | 'tickets' | 'cash' | 'products' | 'reports' | 'users' | 'settings';
export type SettingsTab = 'shop' | 'ticket' | 'printer' | 'data' | 'verifactu';

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
  const [settings, setSettings] = useState<Settings | null>(null);
  const [logo, setLogo] = useState<Logo | null>(null);
  const [products, setProducts] = useState<Product[]>([]);
  const [loadErr, setLoadErr] = useState('');
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [leave, setLeave] = useState<null | (() => void)>(null);
  const [incoming, setIncoming] = useState<{ lines: ItemDraft[]; from: string; n: number } | null>(null);
  const [vf, setVf] = useState<VerifactuStatus | null>(null);
  // a ticket on the screen or being charged: no update is offered then
  const [saleBusy, setSaleBusy] = useState(false);
  const updates = useUpdate(saleBusy);
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

  // the screen locks itself too, so a till left alone does not stay open on
  // the last admin's session; the core enforces the same limit
  const lockMin = settings?.autoLockMin ?? 0;
  useEffect(() => {
    if (!lockMin) return;
    let last = Date.now();
    const seen = () => {
      last = Date.now();
    };
    const kinds = ['keydown', 'mousedown', 'wheel', 'touchstart'];
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
    if (!admin && screen !== 'sale' && screen !== 'tickets' && screen !== 'cash') setScreen('sale');
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

  // a new PC: if nobody has chosen a printer yet and one is clearly the
  // receipt printer, the core takes it; Ajustes can change it any time
  useEffect(() => {
    if (!admin) return;
    call<{ applied: boolean; name?: string; settings?: Settings }>('printers.auto')
      .then((r) => {
        if (!r.applied || !r.settings) return;
        const s = r.settings;
        setSettings(s);
        toast.ok(`${tr('Impresora de tickets detectada', 'Receipt printer found')}: ${r.name}`, {
          sticky: true,
          action: {
            label: tr('Imprimir prueba', 'Print a test'),
            run: () => call('printers.test', { settings: s, kind: 'page' }).catch((e) => toast.error(errorText(e))),
          },
        });
      })
      .catch(() => {});
  }, [admin, toast]);

  // installs from before recovery codes existed have none until an admin makes one
  useEffect(() => {
    if (!admin) return;
    call<{ set: boolean }>('recovery.status')
      .then((r) => {
        if (!r.set) {
          toast.info(tr('No hay código de recuperación: si se olvida la contraseña de administrador no habrá forma de entrar.', 'There is no recovery code: a forgotten administrator password would lock everyone out.'), {
            sticky: true,
            action: { label: tr('Crear', 'Create'), run: () => setScreen('users') },
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
    { id: 'tickets', label: tr('Tickets', 'Tickets'), icon: ReceiptText },
    { id: 'cash', label: tr('Caja', 'Till'), icon: Wallet },
  ];
  if (admin) {
    nav.push(
      { id: 'products', label: tr('Productos', 'Products'), icon: Package },
      { id: 'reports', label: tr('Informes', 'Reports'), icon: ChartColumn },
      { id: 'users', label: tr('Usuarios', 'Users'), icon: UsersIcon },
      { id: 'settings', label: tr('Ajustes', 'Settings'), icon: SettingsIcon },
    );
  }

  return (
    <div className={`shell${settings.training ? ' practice' : ''}`}>
      <header className="top">
        <div className="brand">
          <Mark size={26} />
          <span>Tiquet</span>
          {settings.shopName && <span className="brand-shop">{settings.shopName}</span>}
        </div>
        <nav className="nav">
          {nav.map((n) => (
            <button key={n.id} className={screen === n.id ? 'on' : ''} onClick={() => go(n.id)} title={n.label}>
              <n.icon size={17} />
              <span>{n.label}</span>
            </button>
          ))}
        </nav>
        <div className="top-right">
          <div className="me">
            <span className="avatar sm">{initials(user.name)}</span>
            <div>
              <b>{user.name}</b>
              <span>{admin ? tr('Administrador', 'Administrator') : tr('Cajero', 'Cashier')}</span>
            </div>
          </div>
          <button className="btn btn-ghost" onClick={() => guard(onLogout)} title={tr('Bloquear y cambiar de usuario', 'Lock and switch user')}>
            <Lock size={16} />
            <span>{tr('Bloquear', 'Lock')}</span>
          </button>
          {native && settings.fullscreen && (
            <button
              className="btn btn-ghost icon-only"
              onClick={() => fullscreen(!full)}
              title={full ? tr('Salir de pantalla completa', 'Leave full screen') : tr('Pantalla completa', 'Full screen')}
              aria-label={full ? tr('Salir de pantalla completa', 'Leave full screen') : tr('Pantalla completa', 'Full screen')}
            >
              {full ? <Minimize2 size={17} /> : <Maximize2 size={17} />}
            </button>
          )}
        </div>
      </header>
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
          onCash={() => go('cash')}
        />
        {screen === 'tickets' && (
          <Tickets
            user={user}
            settings={settings}
            logo={logo}
            onSetupPrinter={setupPrinter}
            onRepeat={(t: TicketRecord) => {
              const lines = t.lines
                .filter((l) => l.qty > 0)
                .map((l) => ({ name: l.name, qty: l.qty, unitCents: l.unitCents, vatBp: l.vatBp, productId: l.productId ?? undefined }));
              setIncoming((cur) => ({ lines, from: t.number, n: (cur?.n ?? 0) + 1 }));
              setScreen('sale');
            }}
          />
        )}
        {screen === 'cash' && <Cash user={user} />}
        {screen === 'products' && admin && <Products settings={settings} onChanged={loadProducts} />}
        {screen === 'reports' && admin && <Reports />}
        {screen === 'users' && admin && <Users me={user} onMe={setUser} />}
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
          />
        )}
      </main>
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
