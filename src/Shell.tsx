import { useCallback, useEffect, useState } from 'react';
import { ChartColumn, Lock, Package, ReceiptText, Settings as SettingsIcon, ShoppingBag, Users as UsersIcon } from 'lucide-react';
import { call } from './api';
import { initials } from './format';
import { errorText, tr, type Lang } from './i18n';
import { Products } from './Products';
import { Reports } from './Reports';
import { Sale } from './Sale';
import { SettingsScreen } from './SettingsScreen';
import { Tickets } from './Tickets';
import { useToast } from './toast';
import type { Logo, Product, Settings, User } from './types';
import { Mark } from './ui';
import { Users } from './Users';

type Screen = 'sale' | 'tickets' | 'products' | 'reports' | 'users' | 'settings';

interface Props {
  user: User;
  setUser: (u: User) => void;
  lang: Lang;
  onLang: (l: Lang) => void;
  onLogout: () => void;
}

export function Shell({ user, setUser, lang, onLang, onLogout }: Props) {
  const toast = useToast();
  const [screen, setScreen] = useState<Screen>('sale');
  const [settings, setSettings] = useState<Settings | null>(null);
  const [logo, setLogo] = useState<Logo | null>(null);
  const [products, setProducts] = useState<Product[]>([]);
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

  useEffect(() => {
    call<Settings>('settings.get')
      .then(setSettings)
      .catch((e) => toast.error(errorText(e)));
    loadLogo();
    loadProducts();
  }, [toast, loadLogo, loadProducts]);

  useEffect(() => {
    if (!admin && screen !== 'sale' && screen !== 'tickets') setScreen('sale');
  }, [admin, screen]);

  if (!settings) return <div className="splash" />;

  const nav: { id: Screen; label: string; icon: typeof ShoppingBag }[] = [
    { id: 'sale', label: tr('Venta', 'Sale'), icon: ShoppingBag },
    { id: 'tickets', label: tr('Tickets', 'Tickets'), icon: ReceiptText },
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
    <div className="shell">
      <header className="top">
        <div className="brand">
          <Mark size={26} />
          <span>Tiquet</span>
          {settings.shopName && <span className="brand-shop">{settings.shopName}</span>}
        </div>
        <nav className="nav">
          {nav.map((n) => (
            <button key={n.id} className={screen === n.id ? 'on' : ''} onClick={() => setScreen(n.id)} title={n.label}>
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
          <button className="btn btn-ghost" onClick={onLogout} title={tr('Bloquear y cambiar de usuario', 'Lock and switch user')}>
            <Lock size={16} />
            <span>{tr('Bloquear', 'Lock')}</span>
          </button>
        </div>
      </header>
      <main className="main">
        <Sale active={screen === 'sale'} user={user} settings={settings} logo={logo} products={products} onSold={loadProducts} />
        {screen === 'tickets' && <Tickets user={user} settings={settings} logo={logo} />}
        {screen === 'products' && admin && <Products settings={settings} onChanged={loadProducts} />}
        {screen === 'reports' && admin && <Reports />}
        {screen === 'users' && admin && <Users me={user} onMe={setUser} />}
        {screen === 'settings' && admin && (
          <SettingsScreen settings={settings} onSaved={setSettings} logo={logo} onLogo={loadLogo} lang={lang} onLang={onLang} />
        )}
      </main>
    </div>
  );
}
