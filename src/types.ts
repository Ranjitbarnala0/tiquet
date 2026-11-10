import type { Lang, PrinterState } from './i18n';

export type Role = 'admin' | 'cashier';
export type Method = 'cash' | 'card' | 'bizum';
export type BookMethod = Method | 'transfer';
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
  autoUpdate: boolean;
  // several PCs in one shop: this one alone, the main till, or one that
  // sells with the main till's data
  lan?: { role: 'single' | 'main' | 'secondary'; shop?: string; addr?: string; down?: boolean } | null;
}

// a card terminal or a recharge machine, named by the shop
export interface Machine {
  id: number;
  name: string;
  card: boolean;
  recharge: boolean;
  // its recharges are paid in cash at the counter, even if it also takes cards
  rechargeCash: boolean;
  rechargeExpense: boolean;
  commissionBp: number;
  // how much of the sales it takes counts as an expense
  takingsBp: number;
  removed: boolean;
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
  ticketNif: boolean;
  ticketAddress: boolean;
  ticketPhone: boolean;
  ticketCashier: boolean;
  printerName: string;
  printerKind: PrinterKind;
  printerHost: string;
  printerPort: number;
  printerCom: string;
  printerBaud: number;
  columns: number;
  // the columns follow the printer, 58 mm when nothing says
  paperAuto: boolean;
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
  autoLogin: number;
  bookStaffDelete: boolean;
  autoUpdate: boolean;
  staffTotals: boolean;
  staffRefund: boolean;
  payBizum: boolean;
  showRecharge: boolean;
  showExpense: boolean;
  showRepairs: boolean;
  dayCells: string;
  productOrder: 'new' | 'az';
  expenseConcepts: string;
  expenseMethod: 'cash' | 'card';
  floatCents: number;
  machines: Machine[];
  machineAsk: boolean;
  cardMachine: number;
  // what the recharge window starts on: a machine, or 0 for cash
  rechargeMachine: number;
  rechargeAmounts: string;
  rechargeOperators: string;
  rechargeCashExpense: boolean;
  rechargeCommissionBp: number;
  rechargePrint: boolean;
  training: boolean;
  verifactu: 'off' | 'test' | 'prod';
  cert: string;
  autostart: boolean;
  fullscreen: boolean;
  repairTerms: string;
  // this PC's printer for A4 invoices; empty prints them as receipts
  invoicePrinter: string;
  invoiceTerms: string;
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
  // recharges paid in cash, and expenses paid from the drawer
  rechargeCents: number;
  expenseCents: number;
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
  machine: number | null;
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
  paymentChanges: { at: string; byName: string; from: Method; to: Method; fromMachine: number | null; toMachine: number | null }[];
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

// what is paid of a repair: everything, a part (or something and no price
// yet), nothing, nothing and no price yet; none once cancelled
export type PayState = 'paid' | 'partial' | 'unpaid' | 'unpriced' | 'none';

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
  pay: PayState;
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
  depositCents: number;
  dueCents: number | null;
  pay: PayState;
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

// windows: receipt printer (raw ESC/POS) · document: any printer through
// its driver · network: IP · serial: COM port (Bluetooth, USB-serial)
export type PrinterKind = 'windows' | 'document' | 'network' | 'serial';

export interface PrinterEntry {
  name: string;
  port: string;
  driver: string;
  virtual: boolean;
  default: boolean;
  receipt: boolean;
  // characters per line for the paper its driver is set up for
  columns: number | null;
}

export interface PortEntry {
  name: string;
  kind: 'bluetooth' | 'usb' | 'serial';
}

export interface MailConfig {
  host: string;
  port: number;
  security: 'starttls' | 'tls' | 'none';
  user: string;
  from: string;
}

export interface PrinterStatus {
  name: string;
  state: PrinterState;
  jobs: number;
  last: PrintRow | null;
}

// a line of the day: a ticket, or a recharge or an expense typed in
export interface DayRow {
  source: 'ticket' | 'recharge' | 'expense' | 'sale';
  id: number;
  at: string;
  concept: string;
  cents: number;
  method: BookMethod;
  byName: string;
  supplier: string;
  supplierNif: string;
  invoiceRef: string;
  vatBp: number;
  machine: number | null;
  costCents: number;
  number: string | null;
  refund: boolean;
  invoice: boolean;
  printed: 'printed' | 'unknown' | 'failed' | 'none' | null;
  gone: boolean;
  deletedBy: string | null;
  deletedAt: string | null;
  deleteReason: string | null;
}

export interface DaySummary {
  cashCents: number;
  cardCents: number;
  bizumCents: number;
  otherCents: number;
  rechargeCents: number;
  totalCents: number;
  saleCount: number;
  refundCount: number;
  rechargeCount: number;
  purchaseCents: number;
  purchaseCount: number;
  rechargeCostCents: number;
  machineCostCents: number;
  expenseCents: number;
  netCents: number;
  drawerCents: number;
  machines: MachineSum[];
}

// sales and recharges on one machine; id 0 is cards from before there were machines
export interface MachineSum {
  id: number;
  salesCents: number;
  rechargeCents: number;
  count: number;
}

export interface DayList {
  rows: DayRow[];
  // none when the cashiers do not see the totals
  summary: DaySummary | null;
  from: string;
  to: string;
  capped: boolean;
  staffDelete: boolean;
  anyDay: boolean;
}

export interface DayGone {
  tickets: number;
  entries: number;
  kept: number;
}

export interface Summary {
  day: DaySummary;
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
