import { useCallback, useEffect, useState } from "react";
import {
  Link2,
  Monitor,
  RefreshCw,
  Search,
  Unplug,
  Wifi,
  WifiOff,
} from "lucide-react";
import { call } from "./api";
import { dateTime } from "./format";
import { errorText, tr } from "./i18n";
import { useToast } from "./toast";
import { Field, Modal, Toggle } from "./ui";

// Several PCs in one shop: one till keeps the data and the others, on the
// same WiFi, sell with it. Each PC signs in with a user and password of the
// shop and prints on its own printer.

export interface LanClient {
  id: string;
  pc: string;
  ip: string;
  seen: string;
  user: string | null;
}

export type LanRole =
  | { role: "single" }
  | { role: "main"; port: number; addresses: string[]; clients: LanClient[] }
  | {
      role: "secondary";
      shop: string;
      addr: string;
      signedIn?: boolean;
      down?: boolean;
    };

interface Found {
  addr: string;
  shop: string;
  install: string;
  version: string;
}

const PORT = ":47713";
const bare = (addr: string) =>
  addr.endsWith(PORT) ? addr.slice(0, -PORT.length) : addr;

// Looks for the shop's main till on this WiFi; a click joins it. Its address
// can be typed too, as the main till shows it in Ajustes › Cajas.
export function JoinShop({ onJoined }: { onJoined: () => void }) {
  const toast = useToast();
  const [found, setFound] = useState<Found[] | null>(null);
  const [looking, setLooking] = useState(false);
  const [addr, setAddr] = useState("");
  const [busy, setBusy] = useState(false);

  const look = useCallback(async () => {
    setLooking(true);
    try {
      setFound(await call<Found[]>("lan.find"));
    } catch (e) {
      toast.error(errorText(e));
      setFound([]);
    } finally {
      setLooking(false);
    }
  }, [toast]);

  useEffect(() => {
    look();
  }, [look]);

  const join = async (to: string) => {
    if (busy || !to.trim()) return;
    setBusy(true);
    try {
      await call("lan.connect", { addr: to.trim() });
      onJoined();
    } catch (e) {
      toast.error(errorText(e));
      setBusy(false);
    }
  };

  return (
    <div className="lan-join">
      <div className="lan-found">
        {looking && (
          <p className="lan-looking">
            <RefreshCw size={16} className="lan-spin" />
            {tr(
              "Buscando la caja principal en esta WiFi…",
              "Looking for the main till on this WiFi…",
            )}
          </p>
        )}
        {!looking && found?.length === 0 && (
          <p className="lan-none">
            {tr(
              "No se encuentra ninguna. En la caja principal: Ajustes › Cajas › «Compartir esta caja» encendido, y las dos en la misma WiFi.",
              'None found. On the main till: Settings › Tills › "Share this till" on, and both on the same WiFi.',
            )}
          </p>
        )}
        {found?.map((f) => (
          <button
            key={f.install}
            type="button"
            className="lan-shop"
            onClick={() => join(f.addr)}
            disabled={busy}
          >
            <Monitor size={22} />
            <span>
              <b>{f.shop || tr("Caja sin nombre", "Unnamed till")}</b>
              <small>
                {bare(f.addr)} · Tiquet {f.version}
              </small>
            </span>
            <Link2 size={18} />
          </button>
        ))}
      </div>
      <div className="lan-manual">
        <Field
          label={tr("O escribe su dirección", "Or type its address")}
          hint={tr(
            "La caja principal la muestra en Ajustes › Cajas",
            "The main till shows it in Settings › Tills",
          )}
        >
          <input
            value={addr}
            onChange={(e) => setAddr(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                join(addr);
              }
            }}
            placeholder="192.168.1.20"
            inputMode="decimal"
          />
        </Field>
        <button
          type="button"
          className="btn btn-primary"
          onClick={() => join(addr)}
          disabled={busy || !addr.trim()}
        >
          <Link2 size={16} />
          {tr("Conectar", "Connect")}
        </button>
        <button
          type="button"
          className="btn btn-ghost"
          onClick={look}
          disabled={looking || busy}
        >
          <Search size={16} />
          {tr("Buscar otra vez", "Search again")}
        </button>
      </div>
    </div>
  );
}

// This PC works with a main till that does not answer: it tries again on its
// own, and can go back to working alone.
export function LanDown({
  shop,
  addr,
  onRetry,
}: {
  shop: string;
  addr: string;
  onRetry: () => void;
}) {
  const [alone, setAlone] = useState(false);
  useEffect(() => {
    const t = window.setInterval(onRetry, 5000);
    return () => window.clearInterval(t);
  }, [onRetry]);
  return (
    <div className="fatal">
      <WifiOff size={34} />
      <h2>
        {tr(
          "No se llega a la caja principal",
          "The main till cannot be reached",
        )}
      </h2>
      <p>
        {tr(
          `Esta caja vende con los datos de «${shop}» (${bare(addr)}). Comprueba que ese PC está encendido, con Tiquet abierto y en la misma WiFi.`,
          `This till sells with the data of "${shop}" (${bare(addr)}). Check that PC is on, with Tiquet open and on the same WiFi.`,
        )}
      </p>
      <p className="muted">
        {tr(
          "Se vuelve a intentar sola cada 5 segundos.",
          "It tries again by itself every 5 seconds.",
        )}
      </p>
      <div className="fatal-actions">
        <button className="btn btn-primary" onClick={onRetry}>
          <RefreshCw size={16} />
          {tr("Reintentar", "Try again")}
        </button>
        <button className="btn" onClick={() => setAlone(true)}>
          <Unplug size={16} />
          {tr("Trabajar sin la caja principal", "Work without the main till")}
        </button>
      </div>
      {alone && <Leave shop={shop} onClose={() => setAlone(false)} />}
    </div>
  );
}

// going back to this PC's own data
function Leave({ shop, onClose }: { shop: string; onClose: () => void }) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const leave = async () => {
    setBusy(true);
    try {
      await call("lan.disconnect");
      location.reload();
    } catch (e) {
      toast.error(errorText(e));
      setBusy(false);
    }
  };
  return (
    <Modal
      title={tr(
        "Dejar de trabajar con la caja principal",
        "Stop working with the main till",
      )}
      onClose={onClose}
      actions={
        <>
          <button className="btn btn-ghost" onClick={onClose}>
            {tr("Cancelar", "Cancel")}
          </button>
          <button className="btn btn-danger" onClick={leave} disabled={busy}>
            <Unplug size={16} />
            {tr("Dejar de trabajar con ella", "Stop working with it")}
          </button>
        </>
      }
    >
      <p>
        {tr(
          `Este PC volverá a sus propios datos. Lo que se venda aquí desde ahora no estará en «${shop}», ni en su Hoy ni en su cierre de caja.`,
          `This PC goes back to its own data. What is sold here from now on will not be in "${shop}", nor in its Today or its till close.`,
        )}
      </p>
      <p className="muted">
        {tr(
          "Se puede volver a conectar cuando se quiera, en Ajustes › Cajas.",
          "It can connect again at any time, in Settings › Tills.",
        )}
      </p>
    </Modal>
  );
}

// Ajustes › Cajas
export function LanTab() {
  const toast = useToast();
  const [lan, setLan] = useState<LanRole | null>(null);
  const [busy, setBusy] = useState(false);
  const [joining, setJoining] = useState(false);
  const [leaving, setLeaving] = useState(false);

  const load = useCallback(() => {
    call<LanRole>("lan.status")
      .then(setLan)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  useEffect(() => {
    load();
    const t = window.setInterval(load, 5000);
    return () => window.clearInterval(t);
  }, [load]);

  const share = async (on: boolean) => {
    setBusy(true);
    try {
      setLan(await call<LanRole>("lan.share", { on }));
      toast.ok(
        on
          ? tr(
              "Los otros PC de la tienda ya pueden conectarse",
              "The shop’s other PCs can connect now",
            )
          : tr("Esta caja ya no se comparte", "This till is no longer shared"),
      );
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const kick = async (c: LanClient) => {
    try {
      setLan(await call<LanRole>("lan.kick", { id: c.id }));
      toast.ok(
        tr(
          `${c.pc || "Ese PC"} tendrá que volver a entrar con su contraseña`,
          `${c.pc || "That PC"} will have to sign in again`,
        ),
      );
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  if (!lan) return null;

  if (lan.role === "secondary") {
    return (
      <section className="form-section">
        <h2>
          {tr(
            "Esta caja trabaja con la caja principal",
            "This till works with the main till",
          )}
        </h2>
        <div className="lan-status">
          <Wifi size={20} />
          <span>
            <b>{lan.shop}</b>
            <small className="mono">{bare(lan.addr)}</small>
          </span>
        </div>
        <p className="muted">
          {tr(
            "Las ventas, los productos, los usuarios, el cajón, Hoy y los informes son los de la caja principal. La impresora, el papel y el idioma son de este PC.",
            "Sales, products, users, the drawer, Today and the reports are the main till’s. The printer, the paper and the language belong to this PC.",
          )}
        </p>
        <button className="btn" onClick={() => setLeaving(true)}>
          <Unplug size={16} />
          {tr(
            "Dejar de trabajar con la caja principal",
            "Stop working with the main till",
          )}
        </button>
        {leaving && <Leave shop={lan.shop} onClose={() => setLeaving(false)} />}
      </section>
    );
  }

  const main = lan.role === "main" ? lan : null;
  return (
    <div className="lan-tab">
      <section className="form-section">
        <h2>{tr("Varios PC en la tienda", "Several PCs in the shop")}</h2>
        <p className="lede">
          {tr(
            "Los otros PC de la tienda, en la misma WiFi, venden con los datos de esta caja: los mismos productos, la misma numeración, el mismo cajón y el mismo Hoy. Cada uno entra con su usuario y contraseña e imprime en su propia impresora.",
            "The shop’s other PCs, on the same WiFi, sell with this till’s data: the same products, the same numbering, the same drawer and the same Today. Each signs in with its own user and password and prints on its own printer.",
          )}
        </p>
        <Toggle
          checked={!!main}
          onChange={(v) => !busy && share(v)}
          label={tr(
            "Compartir esta caja con los otros PC de la tienda",
            "Share this till with the shop’s other PCs",
          )}
          hint={tr(
            "Esta será la caja principal: guarda los datos de todas, así que tiene que estar encendida y con Tiquet abierto mientras las otras venden.",
            "This becomes the main till: it keeps everyone’s data, so it has to be on with Tiquet open while the others sell.",
          )}
        />
        {main && (
          <>
            <div className="lan-status">
              <Wifi size={20} />
              <span>
                <b>{tr("Dirección de esta caja", "This till’s address")}</b>
                <small className="mono">
                  {main.addresses.length
                    ? main.addresses.join(" · ")
                    : tr("sin red", "no network")}
                </small>
              </span>
            </div>
            <ol className="lan-steps">
              <li>
                {tr(
                  "En el otro PC, instala Tiquet con el mismo enlace de descarga.",
                  "On the other PC, install Tiquet from the same download link.",
                )}
              </li>
              <li>
                {tr(
                  "En su primera pantalla pulsa «Conectar a la caja de la tienda» y elige esta.",
                  'On its first screen press "Connect to the shop’s till" and pick this one.',
                )}
              </li>
              <li>
                {tr(
                  "Entra con un usuario y contraseña de esta caja. Ya está.",
                  "Sign in with a user and password of this till. Done.",
                )}
              </li>
            </ol>
            <p className="muted">
              {tr(
                "Si Windows pregunta si deja a Tiquet usar la red, pulsa Permitir. Si el otro PC no la encuentra, la WiFi de este PC tiene que estar como red privada (Configuración › Red e Internet › Wi-Fi › Propiedades).",
                "If Windows asks whether Tiquet may use the network, press Allow. If the other PC cannot find it, this PC’s WiFi has to be a private network (Settings › Network & internet › Wi-Fi › Properties).",
              )}
            </p>
          </>
        )}
      </section>

      {main && (
        <section className="form-section">
          <h2>
            {tr("PC conectados", "Connected PCs")} ({main.clients.length})
          </h2>
          {main.clients.length === 0 ? (
            <p className="muted">{tr("Ninguno todavía.", "None yet.")}</p>
          ) : (
            <div className="backup-list">
              {main.clients.map((c) => (
                <div className="backup-row" key={c.id}>
                  <div>
                    <b>
                      {c.pc || "PC"} · {c.user ?? "—"}
                    </b>
                    <span>
                      {c.ip} · {tr("último uso", "last used")}{" "}
                      {dateTime(c.seen)}
                    </span>
                  </div>
                  <button className="btn btn-ghost sm" onClick={() => kick(c)}>
                    <Unplug size={15} />
                    {tr("Desconectar", "Disconnect")}
                  </button>
                </div>
              ))}
            </div>
          )}
        </section>
      )}

      {!main && (
        <section className="form-section">
          <h2>
            {tr(
              "¿La tienda ya tiene una caja principal?",
              "Does the shop already have a main till?",
            )}
          </h2>
          <p className="muted">
            {tr(
              "Este PC puede vender con los datos de otra caja de la tienda en vez de los suyos. Sus propias ventas se quedan guardadas aquí y vuelven si deja de trabajar con ella.",
              "This PC can sell with another till’s data instead of its own. Its own sales stay saved here and come back if it stops working with it.",
            )}
          </p>
          {joining ? (
            <JoinShop onJoined={() => location.reload()} />
          ) : (
            <button className="btn" onClick={() => setJoining(true)}>
              <Link2 size={16} />
              {tr("Conectar a otra caja", "Connect to another till")}
            </button>
          )}
        </section>
      )}
    </div>
  );
}
