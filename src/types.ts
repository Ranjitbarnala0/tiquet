import type { Lang } from './i18n';

export type Role = 'admin' | 'cashier';
export type Method = 'cash' | 'card' | 'bizum';

export interface User {
  id: number;
  name: string;
  role: Role;
}

export interface UserRow extends User {
  active: boolean;
  createdAt: string;
}

export interface Boot {
  needsSetup: boolean;
  users: User[];
  lang: Lang;
  shopName: string;
  session: User | null;
  version: string;
}

export interface Settings {
  shopName: string;
  shopAddress: string;
  shopPhone: string;
  shopNif: string;
  footer: string;
  ticketPrefix: string;
  defaultVatBp: number;
  showVat: boolean;
  printerName: string;
  columns: number;
  codepage: string;
  cut: boolean;
  feedLines: number;
  drawer: 'cash' | 'always' | 'never';
  cancelChinese: boolean;
  copies: number;
  backupDir: string;
  autoBackup: boolean;
  lang: Lang;
}

export interface Logo {
  w: number;
  h: number;
  bits: string;
  png: string;
}

export interface Product {
  id: number;
  name: string;
  priceCents: number;
  vatBp: number;
  barcode: string;
  favorite: boolean;
  active: boolean;
  soldQty: number;
}

export interface SaleLine {
  productId?: number;
  name: string;
  qty: number;
  unitCents: number;
  vatBp: number;
}

export interface CartLine extends SaleLine {
  key: string;
}

export type PLine = { t: 'text'; text: string; bold: boolean; size: 'normal' | 'tall' | 'big' } | { t: 'logo' };

export interface VatRow {
  vatBp: number;
  baseCents: number;
  vatCents: number;
  totalCents: number;
}

export interface Totals {
  totalCents: number;
  itemCount: number;
  vat: VatRow[];
}

export interface Payment {
  method: Method;
  tenderedCents: number;
  changeCents: number;
}

export interface Preview {
  lines: PLine[];
  totals: Totals;
  number: string;
  payment: Payment | null;
  paymentError: string | null;
}

export interface TicketRecord {
  id: number;
  number: string;
  issuedAt: string;
  userName: string;
  totalCents: number;
  itemCount: number;
  payment: Payment;
  voidedAt: string | null;
  voidedBy: string | null;
  voidReason: string | null;
  lines: SaleLine[];
}

export interface TicketSummary {
  id: number;
  number: string;
  issuedAt: string;
  userName: string;
  totalCents: number;
  itemCount: number;
  paymentMethod: Method;
  voided: boolean;
}

export interface Finalized {
  ticket: TicketRecord;
  printed: boolean;
  printError: string | null;
}

export interface Summary {
  totalCents: number;
  tickets: number;
  items: number;
  voided: number;
  firstNumber: string | null;
  lastNumber: string | null;
  byMethod: { method: Method; count: number; totalCents: number }[];
  byDay: { date: string; count: number; totalCents: number }[];
  vat: VatRow[];
  top: { name: string; qty: number; totalCents: number }[];
}
