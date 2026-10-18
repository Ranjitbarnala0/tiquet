import type { Lang, PrinterState } from './i18n';

export type Role = 'admin' | 'cashier';
export type Method = 'cash' | 'card' | 'bizum';
export type TicketKind = 'sale' | 'refund';

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
  canRecover: boolean;
  installId: string;
  users: User[];
  lang: Lang;
  shopName: string;
  session: User | null;
  version: string;
}

export interface Settings {
  shopName: string;
  legalName: string;
  shopAddress: string;
  shopPhone: string;
  shopNif: string;
  footer: string;
  ticketPrefix: string;
  defaultVatBp: number;
  showVat: boolean;
  printerName: string;
  printerKind: 'windows' | 'network';
  printerHost: string;
  printerPort: number;
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
  autoLockMin: number;
  training: boolean;
  verifactu: 'off' | 'test' | 'prod';
  cert: string;
  autostart: boolean;
  fullscreen: boolean;
  repairTerms: string;
}

export interface Customer {
  name: string;
  nif: string;
  address: string;
}

export interface CashSession {
  id: number;
  openedAt: string;
  openedBy: string;
  floatCents: number;
  closedAt: string | null;
  closedBy: string | null;
  expectedCents: number | null;
  countedCents: number | null;
  note: string | null;
}

export interface CashDrawer {
  session: CashSession | null;
  moves: { at: string; byName: string; cents: number; reason: string }[];
  cashCents: number;
  movesCents: number;
  expectedCents: number;
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
  // stored tickets only
  id?: number;
  refunded?: number;
  // the repair this line hands back, or the one it is the deposit for
  repairId?: number;
}

export interface Recent {
  productId: number | null;
  name: string;
  unitCents: number;
  vatBp: number;
  at: string;
  times: number;
}

export interface CartLine extends SaleLine {
  key: string;
}

export type PLine =
  | { t: 'text'; text: string; bold: boolean; size: 'normal' | 'tall' | 'big' }
  | { t: 'logo' }
  | { t: 'qr'; rows: string[]; module: number };

// where a ticket's billing record stands with the AEAT
export interface TicketAeat {
  env: 'test' | 'prod';
  state: 'pending' | 'Correcto' | 'AceptadoConErrores' | 'Incorrecto';
  code: string | null;
  message: string | null;
  csv: string | null;
}

export interface VerifactuProblem {
  recordId: number;
  ticketId: number;
  kind: 'alta' | 'anulacion';
  number: string;
  env: 'test' | 'prod';
  state: 'AceptadoConErrores' | 'Incorrecto';
  code: string | null;
  message: string | null;
  at: string;
}

export interface VerifactuStatus {
  env: 'test' | 'prod' | null;
  producer: boolean;
  pending: number;
  oldest: string | null;
  sent: number;
  problems: VerifactuProblem[];
  lastSend: string | null;
  lastError: string | null;
  clockDrift: number | null;
}

export interface CertEntry {
  thumbprint: string;
  name: string;
  issuer: string;
  expires: string;
  valid: boolean;
}

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

export interface PrintRow {
  at: string;
  copy: boolean;
  status: 'sent' | 'printed' | 'failed' | 'unknown';
  detail: string | null;
}

export interface TicketRef {
  id: number;
  number: string;
  issuedAt: string;
  totalCents: number;
  voided: boolean;
}

export interface TicketRecord {
  id: number;
  kind: TicketKind;
  number: string;
  issuedAt: string;
  userName: string;
  totalCents: number;
  itemCount: number;
  payment: Payment;
  voidedAt: string | null;
  voidedBy: string | null;
  voidReason: string | null;
  refundOf: TicketRef | null;
  refundReason: string | null;
  refunds: TicketRef[];
  lines: SaleLine[];
  prints: PrintRow[];
  customer: Customer | null;
  training: boolean;
  qr: string | null;
  aeat: TicketAeat | null;
}

export interface RefundPreview {
  lines: PLine[];
  totals: Totals;
  number: string;
}

export interface TicketSummary {
  id: number;
  kind: TicketKind;
  number: string;
  issuedAt: string;
  userName: string;
  totalCents: number;
  itemCount: number;
  paymentMethod: Method;
  voided: boolean;
  printed: 'printed' | 'unknown' | 'failed' | 'none';
  invoice: boolean;
}

export interface Finalized {
  ticket: TicketRecord;
  duplicate?: boolean;
  printed: boolean;
  printError: string | null;
  printWarning: string | null;
}

export type RepairStatus = 'received' | 'in_progress' | 'ready' | 'not_repaired' | 'delivered' | 'cancelled';

// what is typed when the phone is left
export interface RepairData {
  customerName: string;
  customerPhone: string;
  brand: string;
  model: string;
  colour: string;
  imei: string;
  fault: string;
  notes: string;
  estimateCents: number | null;
}

export interface RepairEvent {
  at: string;
  byName: string;
  kind: 'created' | 'status' | 'edited' | 'delivered' | 'reopened' | 'cancelled';
  status: RepairStatus;
  detail: { fields?: string[]; estimateFrom?: number | null; estimateTo?: number | null; ticket?: string; reason?: string } | null;
}

export interface Repair extends RepairData {
  id: number;
  number: string;
  createdAt: string;
  createdBy: string;
  status: RepairStatus;
  deposit: TicketRef | null;
  // what the shop still holds of the deposit
  depositCents: number;
  dueCents: number | null;
  ticket: TicketRef | null;
  closedAt: string | null;
  closedBy: string | null;
  cancelReason: string | null;
  lineName: string;
  events: RepairEvent[];
}

export interface RepairSummary {
  id: number;
  number: string;
  createdAt: string;
  customerName: string;
  customerPhone: string;
  device: string;
  status: RepairStatus;
  estimateCents: number | null;
}

export interface RepairDetail {
  repair: Repair;
  resguardo: PLine[];
  slip: PLine[];
  logo: Logo | null;
}

export interface RepairCreated {
  repair: Repair;
  deposit: Finalized | null;
  duplicate?: boolean;
  printed: boolean;
  printError: string | null;
  printWarning: string | null;
}

export interface PrinterEntry {
  name: string;
  port: string;
  driver: string;
  virtual: boolean;
  default: boolean;
}

export interface PrinterStatus {
  name: string;
  state: PrinterState;
  jobs: number;
  last: PrintRow | null;
}

export interface Summary {
  totalCents: number;
  salesCents: number;
  refundCents: number;
  tickets: number;
  refunds: number;
  items: number;
  voided: number;
  firstNumber: string | null;
  lastNumber: string | null;
  byMethod: { method: Method; count: number; totalCents: number }[];
  byDay: { date: string; count: number; totalCents: number }[];
  vat: VatRow[];
  top: { name: string; qty: number; totalCents: number }[];
  discounts: number;
  discountCents: number;
  changed: number;
  changedCents: number;
  changes: { issuedAt: string; number: string; userName: string; name: string; qty: number; listCents: number; unitCents: number }[];
}
