import { useCallback, useEffect, useMemo, useState } from "react";
import {
  FileDown,
  FileText,
  Plus,
  Printer,
  ReceiptText,
  Search,
  Share2,
  Trash2,
  TriangleAlert,
} from "lucide-react";
import { call, toError } from "./api";
import { dateTime, eur, fmt, parseMoney, rate, VAT_RATES } from "./format";
import {
  errorText,
  isPrinterSetup,
  isTransient,
  tr,
  warningText,
} from "./i18n";
import { cardMachines } from "./machines";
import { customerNif } from "./nif";
import { ShareDialog } from "./Share";
import { useToast } from "./toast";
import type { Method, Product, Settings, TicketRecord } from "./types";
import { Drawer, Empty, Field, Segmented } from "./ui";

// Facturas: the full invoices the shop made out to a customer, on A4. New ones
// are charged here like any sale, and come out of the A4 printer by themselves.

interface InvoiceRow {
  id: number;
  kind: "sale" | "refund";
  number: string;
  issuedAt: string;
  userName: string;
  totalCents: number;
  paymentMethod: Method;
  voided: boolean;
  customerName: string;
  customerNif: string;
}

interface Finalized {
  ticket: TicketRecord;
  printed: boolean;
  printError: string | null;
  printWarning: string | null;
}

interface LineDraft {
  key: number;
  name: string;
  imei: string;
  qty: string;
  price: string;
  vatBp: number;
  productId?: number;
}

const newId = () =>
  crypto.randomUUID?.() ??
  `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

let keys = 0;
const blankLine = (vatBp: number): LineDraft => ({
  key: ++keys,
  name: "",
  imei: "",
  qty: "1",
  price: "",
  vatBp,
});

const METHOD: Record<Method, () => string> = {
  cash: () => tr("Efectivo", "Cash"),
  card: () => tr("Tarjeta", "Card"),
  bizum: () => "Bizum",
};

export function Invoices({
  settings,
  products,
  onSetupPrinter,
}: {
  settings: Settings;
  products: Product[];
  onSetupPrinter?: () => void;
}) {
  const toast = useToast();
  const [q, setQ] = useState("");
  const [list, setList] = useState<InvoiceRow[] | null>(null);
  const [selId, setSelId] = useState<number | null>(null);
  const [rec, setRec] = useState<TicketRecord | null>(null);
  const [making, setMaking] = useState(false);
  const [sharing, setSharing] = useState(false);
  const [busy, setBusy] = useState("");

  const load = useCallback(
    (keep?: number) => {
      call<InvoiceRow[]>("invoices.list", { q: q.trim() })
        .then((l) => {
          setList(l);
          setSelId(
            (id) =>
              keep ??
              (id !== null && l.some((r) => r.id === id)
                ? id
                : (l[0]?.id ?? null)),
          );
        })
        .catch((e) => toast.error(errorText(e)));
    },
    [q, toast],
  );

  useEffect(() => {
    const t = window.setTimeout(() => load(), 150);
    return () => window.clearTimeout(t);
  }, [load]);

  useEffect(() => {
    if (selId === null) return setRec(null);
    call<{ ticket: TicketRecord }>("tickets.get", { id: selId })
      .then((r) => setRec(r.ticket))
      .catch((e) => toast.error(errorText(e)));
  }, [selId, toast]);

  const print = async (format: "a4" | "ticket") => {
    if (!rec || busy) return;
    setBusy(format);
    try {
      const r = await call<{ warning: string | null }>("tickets.print", {
        id: rec.id,
        format,
      });
      if (r.warning) toast.info(warningText(r.warning));
      else toast.ok(tr("Impresa", "Printed"));
    } catch (e) {
      const setup =
        (e as { code?: string }).code === "E_NO_A4_PRINTER" ||
        isPrinterSetup(e);
      if (setup && onSetupPrinter)
        toast.error(errorText(e), {
          action: { label: tr("Configurar", "Set up"), run: onSetupPrinter },
        });
      else if (isTransient(e))
        toast.error(errorText(e), {
          action: {
            label: tr("Reintentar", "Retry"),
            run: () => print(format),
          },
        });
      else toast.error(errorText(e));
    } finally {
      setBusy("");
    }
  };

  const pdf = async () => {
    if (!rec || busy) return;
    setBusy("pdf");
    try {
      const r = await call<{ path: string }>("tickets.pdf", {
        id: rec.id,
        reveal: true,
      });
      toast.ok(`${tr("Guardada en", "Saved in")} ${r.path}`);
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy("");
    }
  };

  const noA4 = !settings.invoicePrinter;
  const noNif = !settings.shopNif.trim();

  return (
    <div className="split">
      <section className="list-pane">
        <div className="pane-head">
          <button
            className="btn btn-primary block"
            onClick={() => setMaking(true)}
            disabled={noNif}
          >
            <Plus size={16} />
            {tr("Nueva factura", "New invoice")}
          </button>
          <div className="search-sm">
            <Search size={16} />
            <input
              value={q}
              onChange={(e) => setQ(e.target.value)}
              placeholder={tr(
                "Nº, cliente, NIF o importe",
                "Number, customer, NIF or amount",
              )}
            />
          </div>
          <div className="pane-sub">
            <span>
              {list?.length ?? 0}{" "}
              {(list?.length ?? 0) === 1
                ? tr("factura", "invoice")
                : tr("facturas", "invoices")}
            </span>
          </div>
        </div>
        {noNif && (
          <p className="invoice-warn">
            <TriangleAlert size={16} />
            {tr(
              "Para hacer facturas hace falta el NIF de la tienda: Ajustes › Comercio.",
              "Invoices need the shop’s NIF: Settings › Shop.",
            )}
          </p>
        )}
        <div className="list">
          {list?.map((r) => (
            <button
              key={r.id}
              className={`ticket-row${selId === r.id ? " on" : ""}${r.voided ? " voided" : ""}`}
              onClick={() => setSelId(r.id)}
            >
              <div className="ticket-row-main">
                <b className="mono">{r.number}</b>
                <span>
                  {r.customerName} ·{" "}
                  <span className="mono">{r.customerNif}</span>
                </span>
                <span>{dateTime(r.issuedAt)}</span>
              </div>
              <div className="ticket-row-side">
                {r.kind === "refund" && (
                  <span className="badge amber">
                    {tr("Rectificativa", "Credit note")}
                  </span>
                )}
                {r.voided && (
                  <span className="badge red">{tr("Anulada", "Voided")}</span>
                )}
                <b>{eur(r.totalCents)}</b>
              </div>
            </button>
          ))}
          {list && list.length === 0 && (
            <Empty
              icon={<FileText size={28} />}
              title={
                q
                  ? tr("Ninguna factura con eso", "No invoice matches")
                  : tr("Aún no hay facturas", "No invoices yet")
              }
              text={tr(
                "Una factura lleva los datos del cliente y sale en una hoja A4. Se hace aquí o al cobrar en Venta, con «Factura».",
                'An invoice carries the customer’s details and comes out on an A4 page. Make one here, or when charging in Sale with "Invoice".',
              )}
            />
          )}
        </div>
      </section>

      <section className="detail-pane">
        {rec ? (
          <div className="invoice-view">
            <div className="detail-head">
              <div>
                <h2 className="mono">
                  {rec.kind === "refund"
                    ? tr("Factura rectificativa", "Credit note")
                    : tr("Factura", "Invoice")}{" "}
                  {rec.number}
                </h2>
                <p className="muted">
                  {dateTime(rec.issuedAt)} · {rec.userName} ·{" "}
                  {METHOD[rec.payment.method]()}
                </p>
                {rec.voidedAt && (
                  <p className="void-note">
                    {tr("Anulada", "Voided")}: {rec.voidReason}
                  </p>
                )}
              </div>
            </div>
            {rec.customer && (
              <div className="invoice-customer">
                <span>{tr("Cliente", "Customer")}</span>
                <b>{rec.customer.name}</b>
                <span className="mono">{rec.customer.nif}</span>
                {rec.customer.address && (
                  <span className="pre">{rec.customer.address}</span>
                )}
              </div>
            )}
            <table className="invoice-lines">
              <thead>
                <tr>
                  <th>{tr("Descripción", "Description")}</th>
                  <th>{tr("Cant.", "Qty")}</th>
                  <th>{tr("Precio", "Price")}</th>
                  <th>IVA</th>
                  <th>{tr("Importe", "Amount")}</th>
                </tr>
              </thead>
              <tbody>
                {rec.lines.map((l, i) => (
                  <tr key={l.id ?? i}>
                    <td>{l.name}</td>
                    <td>{l.qty}</td>
                    <td>{fmt(l.unitCents)}</td>
                    <td>{rate(l.vatBp)}</td>
                    <td>{fmt(l.qty * l.unitCents)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="invoice-total">
              <span>{tr("Total", "Total")}</span>
              <b>{eur(rec.totalCents)}</b>
            </div>
            <div className="invoice-actions">
              <button
                className="btn btn-primary"
                onClick={() => print("a4")}
                disabled={!!busy}
              >
                <Printer size={16} />
                {busy === "a4"
                  ? tr("Imprimiendo…", "Printing…")
                  : tr("Imprimir A4", "Print A4")}
              </button>
              <button className="btn" onClick={pdf} disabled={!!busy}>
                <FileDown size={16} />
                PDF
              </button>
              <button
                className="btn"
                onClick={() => setSharing(true)}
                disabled={!!busy}
              >
                <Share2 size={16} />
                {tr("Enviar", "Send")}
              </button>
              <button
                className="btn btn-ghost"
                onClick={() => print("ticket")}
                disabled={!!busy}
              >
                <ReceiptText size={16} />
                {tr("Como ticket", "As a receipt")}
              </button>
            </div>
            {noA4 && (
              <p className="muted small">
                {tr(
                  "Este PC no tiene impresora A4 elegida: las facturas salen por la de tickets. Elígela en Ajustes › Impresora.",
                  "This PC has no A4 printer chosen: invoices come out of the receipt printer. Choose one in Settings › Printer.",
                )}
              </p>
            )}
          </div>
        ) : (
          <Empty
            icon={<FileText size={28} />}
            title={tr("Elige una factura", "Pick an invoice")}
          />
        )}
      </section>

      {making && (
        <NewInvoice
          settings={settings}
          products={products}
          onClose={() => setMaking(false)}
          onDone={(r) => {
            setMaking(false);
            setQ("");
            load(r.ticket.id);
            if (r.printError)
              toast.error(
                `${r.ticket.number}: ${errorText(toError(r.printError))}`,
              );
            else if (r.printWarning) toast.info(warningText(r.printWarning));
            else
              toast.ok(
                `${tr("Factura", "Invoice")} ${r.ticket.number} ${r.printed ? tr("cobrada e impresa", "charged and printed") : tr("cobrada", "charged")}`,
              );
          }}
        />
      )}
      {sharing && rec && (
        <ShareDialog
          ticketId={rec.id}
          number={rec.number}
          onClose={() => setSharing(false)}
        />
      )}
    </div>
  );
}

// The customer, what is sold (a phone with its IMEI on the same line), and
// how it is paid. Charged like any sale, so it counts in Hoy and the till.
function NewInvoice({
  settings,
  products,
  onClose,
  onDone,
}: {
  settings: Settings;
  products: Product[];
  onClose: () => void;
  onDone: (r: Finalized) => void;
}) {
  const toast = useToast();
  const [customer, setCustomer] = useState({ name: "", nif: "", address: "" });
  const [lines, setLines] = useState<LineDraft[]>(() => [
    blankLine(settings.defaultVatBp),
  ]);
  const [method, setMethod] = useState<Method>("card");
  const cards = useMemo(() => cardMachines(settings), [settings]);
  const [machine, setMachine] = useState<number>(() =>
    cards.some((m) => m.id === settings.cardMachine)
      ? settings.cardMachine
      : (cards[0]?.id ?? 0),
  );
  const [busy, setBusy] = useState(false);
  const [saleId] = useState(newId);
  const byName = useMemo(
    () =>
      new Map(
        products.filter((p) => p.active).map((p) => [p.name.toLowerCase(), p]),
      ),
    [products],
  );

  const set = (key: number, patch: Partial<LineDraft>) =>
    setLines((cur) => cur.map((l) => (l.key === key ? { ...l, ...patch } : l)));
  const named = (key: number, name: string) => {
    // a product of the catalogue brings its price and VAT
    const p = byName.get(name.trim().toLowerCase());
    set(
      key,
      p
        ? { name, productId: p.id, price: fmt(p.priceCents), vatBp: p.vatBp }
        : { name, productId: undefined },
    );
  };

  const parsed = lines.map((l) => ({
    l,
    qty: Number.parseInt(l.qty, 10),
    cents: parseMoney(l.price),
  }));
  const filled = parsed.filter((p) => p.l.name.trim() || p.l.price.trim());
  const badLine = (p: (typeof parsed)[number]) =>
    !p.l.name.trim() ||
    !Number.isFinite(p.qty) ||
    p.qty < 1 ||
    p.qty > 999 ||
    p.cents === null ||
    p.cents <= 0;
  const total = filled.reduce(
    (s, p) => s + (badLine(p) ? 0 : p.qty * (p.cents ?? 0)),
    0,
  );
  const nifBad = customer.nif.trim() !== "" && !customerNif(customer.nif);
  const ready =
    customer.name.trim() &&
    customerNif(customer.nif) &&
    filled.length > 0 &&
    !filled.some(badLine);

  const charge = async () => {
    if (!ready || busy) return;
    setBusy(true);
    try {
      const out = filled.map((p) => ({
        name: p.l.imei.trim()
          ? `${p.l.name.trim()} · IMEI ${p.l.imei.trim()}`
          : p.l.name.trim(),
        qty: p.qty,
        unitCents: p.cents,
        vatBp: p.l.vatBp,
        productId: p.l.productId,
      }));
      const r = await call<Finalized>("sale.finalize", {
        lines: out,
        payment: { method, machine: method === "card" ? machine : undefined },
        customer: {
          name: customer.name.trim(),
          nif: customer.nif.trim(),
          address: customer.address.trim(),
        },
        saleId,
        expectedCents: total,
        print: true,
      });
      onDone(r);
    } catch (e) {
      toast.error(errorText(e));
      setBusy(false);
    }
  };

  return (
    <Drawer
      title={tr("Nueva factura", "New invoice")}
      onClose={onClose}
      dirty={!!(customer.name || customer.nif || filled.length)}
      wide
      footer={
        <>
          <div className="invoice-due">
            <span>{tr("Total", "Total")}</span>
            <b>{eur(total)}</b>
          </div>
          <button
            className="btn btn-primary btn-lg"
            onClick={charge}
            disabled={!ready || busy}
          >
            <FileText size={18} />
            {busy
              ? tr("Cobrando…", "Charging…")
              : tr("Cobrar y hacer la factura", "Charge and make the invoice")}
          </button>
        </>
      }
    >
      <div className="invoice-form">
        <section className="form-section">
          <h2>{tr("Cliente", "Customer")}</h2>
          <div className="grid2">
            <Field label={tr("Nombre o razón social", "Name or company")}>
              <input
                autoFocus
                value={customer.name}
                onChange={(e) =>
                  setCustomer({ ...customer, name: e.target.value })
                }
                maxLength={120}
              />
            </Field>
            <Field
              label="NIF / CIF / NIE"
              error={nifBad ? tr("Revisa el NIF", "Check the NIF") : undefined}
            >
              <input
                value={customer.nif}
                onChange={(e) =>
                  setCustomer({
                    ...customer,
                    nif: e.target.value.toUpperCase(),
                  })
                }
                placeholder="B12345674"
              />
            </Field>
          </div>
          <Field
            label={tr("Dirección", "Address")}
            hint={tr(
              "Opcional; una línea por renglón",
              "Optional; one line per line",
            )}
          >
            <textarea
              rows={2}
              value={customer.address}
              onChange={(e) =>
                setCustomer({ ...customer, address: e.target.value })
              }
            />
          </Field>
        </section>

        <section className="form-section">
          <h2>{tr("Qué se vende", "What is sold")}</h2>
          <datalist id="invoice-products">
            {products
              .filter((p) => p.active)
              .slice(0, 400)
              .map((p) => (
                <option key={p.id} value={p.name} />
              ))}
          </datalist>
          <div className="invoice-form-lines">
            {lines.map((l) => {
              const p = parsed.find((x) => x.l.key === l.key)!;
              const touched = l.name.trim() || l.price.trim();
              return (
                <div className="invoice-form-line" key={l.key}>
                  <Field label={tr("Artículo", "Item")}>
                    <input
                      list="invoice-products"
                      value={l.name}
                      onChange={(e) => named(l.key, e.target.value)}
                      placeholder={tr(
                        "iPhone 15 128 GB Negro",
                        "iPhone 15 128 GB Black",
                      )}
                      maxLength={100}
                    />
                  </Field>
                  <Field label={tr("IMEI / Nº de serie", "IMEI / serial")}>
                    <input
                      value={l.imei}
                      onChange={(e) =>
                        set(l.key, { imei: e.target.value.replace(/\s+/g, "") })
                      }
                      inputMode="numeric"
                      maxLength={40}
                    />
                  </Field>
                  <Field label={tr("Cant.", "Qty")}>
                    <input
                      value={l.qty}
                      onChange={(e) =>
                        set(l.key, { qty: e.target.value.replace(/\D/g, "") })
                      }
                      inputMode="numeric"
                      className="qty"
                    />
                  </Field>
                  <Field
                    label={tr("Precio con IVA", "Price incl. VAT")}
                    error={
                      touched &&
                      (p.cents === null || (p.cents ?? 0) <= 0) &&
                      l.price.trim()
                        ? tr("Importe no válido", "Invalid amount")
                        : undefined
                    }
                  >
                    <div className="money-input">
                      <input
                        value={l.price}
                        onChange={(e) => set(l.key, { price: e.target.value })}
                        inputMode="decimal"
                        placeholder="0,00"
                      />
                      <span>€</span>
                    </div>
                  </Field>
                  <Field label="IVA">
                    <select
                      value={l.vatBp}
                      onChange={(e) =>
                        set(l.key, { vatBp: Number(e.target.value) })
                      }
                    >
                      {VAT_RATES.map((v) => (
                        <option key={v} value={v}>
                          {rate(v)}
                        </option>
                      ))}
                    </select>
                  </Field>
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={tr("Quitar la línea", "Remove the line")}
                    onClick={() =>
                      setLines((cur) =>
                        cur.length > 1
                          ? cur.filter((x) => x.key !== l.key)
                          : [blankLine(settings.defaultVatBp)],
                      )
                    }
                  >
                    <Trash2 size={16} />
                  </button>
                </div>
              );
            })}
          </div>
          <button
            type="button"
            className="btn btn-ghost"
            onClick={() =>
              setLines((cur) => [...cur, blankLine(settings.defaultVatBp)])
            }
          >
            <Plus size={16} />
            {tr("Otra línea", "Another line")}
          </button>
        </section>

        <section className="form-section">
          <h2>{tr("Forma de pago", "Payment")}</h2>
          <Segmented
            value={method}
            onChange={setMethod}
            options={(["cash", "card", "bizum"] as Method[]).map((m) => ({
              value: m,
              label: METHOD[m](),
            }))}
          />
          {method === "card" && cards.length > 1 && (
            <div className="picks">
              {cards.map((m) => (
                <button
                  key={m.id}
                  type="button"
                  className={`pick${machine === m.id ? " on" : ""}`}
                  onClick={() => setMachine(m.id)}
                >
                  {m.name}
                </button>
              ))}
            </div>
          )}
          <p className="muted small">
            {settings.invoicePrinter
              ? tr(
                  `Sale sola por la impresora A4 (${settings.invoicePrinter}).`,
                  `It comes out of the A4 printer (${settings.invoicePrinter}) by itself.`,
                )
              : tr(
                  "Sale por la impresora de tickets: este PC no tiene impresora A4 elegida.",
                  "It comes out of the receipt printer: this PC has no A4 printer chosen.",
                )}
          </p>
        </section>
      </div>
    </Drawer>
  );
}
