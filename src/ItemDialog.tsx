import { useMemo, useState, type FormEvent } from 'react';
import { Minus, PackagePlus, Plus, Trash2 } from 'lucide-react';
import { call } from './api';
import { eur, fmt, norm, parseMoney, rate, VAT_RATES } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { Product } from './types';
import { Field, Modal, Segmented, Toggle } from './ui';

export interface ItemDraft {
  name: string;
  unitCents: number;
  qty: number;
  vatBp: number;
  productId?: number;
}

interface Props {
  // a line already in the ticket, or nothing for a new item typed by hand
  line?: ItemDraft;
  products: Product[];
  defaultVat: number;
  admin: boolean;
  onDone: (item: ItemDraft) => void;
  onRemove?: () => void;
  onClose: () => void;
  onCatalogue: () => void;
}

// One form for both: adding an item that is not in the catalogue (it can be
// saved as a product on the way) and changing a line already on the ticket
// (name, price, quantity, VAT; the product's own price too if wanted).
export function ItemDialog({ line, products, defaultVat, admin, onDone, onRemove, onClose, onCatalogue }: Props) {
  const toast = useToast();
  const editing = !!line;
  const [name, setName] = useState(line?.name ?? '');
  const [price, setPrice] = useState(line ? fmt(line.unitCents) : '');
  const [qty, setQty] = useState(line?.qty ?? 1);
  const [vat, setVat] = useState(line?.vatBp ?? defaultVat);
  const [productId, setProductId] = useState(line?.productId);
  const [save, setSave] = useState(false);
  const [favorite, setFavorite] = useState(false);
  const [barcode, setBarcode] = useState('');
  const [updateProduct, setUpdateProduct] = useState(false);
  const [busy, setBusy] = useState(false);

  const cents = parseMoney(price);
  const product = products.find((p) => p.id === productId);
  const same = useMemo(() => products.find((p) => p.active && norm(p.name) === norm(name.trim())), [products, name]);
  // as you type a name, products that match are offered with their price
  const matches = useMemo(() => {
    const t = norm(name.trim());
    if (editing || t.length < 2) return [];
    return products.filter((p) => p.active && norm(p.name).includes(t) && norm(p.name) !== t).slice(0, 4);
  }, [products, name, editing]);

  const discount = cents !== null && cents < 0;
  const valid = name.trim().length > 0 && cents !== null && cents !== 0 && qty >= 1 && qty <= 999;
  const canSave = admin && !editing && !discount;
  const priceChanged = !!product && cents !== null && cents !== product.priceCents;

  const pick = (p: Product) => {
    setName(p.name);
    if (p.priceCents) setPrice(fmt(p.priceCents));
    setVat(p.vatBp);
    setProductId(p.id);
  };

  const submit = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!valid || busy || cents === null) return;
    setBusy(true);
    try {
      let id = productId;
      // a typed name that is a product sells as that product
      if (!editing && !id && same) id = same.id;
      if (canSave && save) {
        const p = await call<Product>('products.save', {
          id: same?.id ?? 0,
          name: name.trim(),
          priceCents: cents,
          vatBp: vat,
          barcode: same?.barcode || barcode.trim(),
          favorite: favorite || !!same?.favorite,
          active: true,
        });
        id = p.id;
        toast.ok(same ? tr('Producto actualizado', 'Product updated') : tr('Guardado como producto', 'Saved as a product'));
        onCatalogue();
      } else if (editing && admin && updateProduct && product && cents > 0) {
        await call<Product>('products.save', { ...product, priceCents: cents, vatBp: vat });
        toast.ok(tr('Precio del producto actualizado', 'Product price updated'));
        onCatalogue();
      }
      onDone({ name: name.trim().replace(/\s+/g, ' ').slice(0, 120), unitCents: cents, qty, vatBp: vat, productId: id });
    } catch (err) {
      toast.error(errorText(err));
      setBusy(false);
    }
  };

  const total = cents !== null ? cents * qty : null;

  return (
    <Modal
      title={editing ? tr('Cambiar línea', 'Change line') : tr('Añadir artículo', 'Add item')}
      onClose={onClose}
      actions={
        <>
          {onRemove && (
            <button type="button" className="btn btn-danger-ghost push-left" onClick={onRemove}>
              <Trash2 size={16} />
              {tr('Quitar del ticket', 'Remove from ticket')}
            </button>
          )}
          <button type="button" className="btn btn-ghost" onClick={onClose}>
            {tr('Cancelar', 'Cancel')}
          </button>
          <button type="button" className="btn btn-primary" onClick={() => submit()} disabled={!valid || busy}>
            {editing ? tr('Guardar', 'Save') : tr('Añadir al ticket', 'Add to ticket')}
            {total !== null && valid && <span className="btn-sum">{eur(total)}</span>}
          </button>
        </>
      }
    >
      <form className="item-form" onSubmit={submit}>
        <Field label={tr('Nombre o descripción', 'Name or description')} hint={editing ? tr('Así sale en el ticket', 'This is how it prints') : undefined}>
          <input
            autoFocus
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              if (!editing) setProductId(undefined);
            }}
            maxLength={120}
            placeholder={tr('Funda iPhone 15, reparación de pantalla…', 'iPhone 15 case, screen repair…')}
          />
        </Field>
        {matches.length > 0 && (
          <div className="item-matches" aria-label={tr('Productos parecidos', 'Similar products')}>
            {matches.map((p) => (
              <button type="button" key={p.id} className="chip" onClick={() => pick(p)}>
                {p.name}
                {p.priceCents > 0 && <b>{eur(p.priceCents)}</b>}
              </button>
            ))}
          </div>
        )}
        <div className="item-row">
          <Field
            label={tr('Precio con IVA', 'Price incl. VAT')}
            error={price.trim() && cents === null ? tr('Precio no válido', 'Invalid price') : undefined}
            hint={discount ? tr('Importe negativo: es un descuento', 'Negative amount: a discount') : undefined}
          >
            <div className="money-input big">
              <input value={price} onChange={(e) => setPrice(e.target.value)} inputMode="decimal" placeholder="0,00" autoFocus={editing} onFocus={(e) => e.target.select()} />
              <span>€</span>
            </div>
          </Field>
          <Field label={tr('Cantidad', 'Quantity')}>
            <div className="stepper big">
              <button type="button" onClick={() => setQty(Math.max(1, qty - 1))} disabled={qty <= 1} aria-label={tr('Una menos', 'One less')}>
                <Minus size={18} />
              </button>
              <input
                className="qty-input"
                inputMode="numeric"
                value={qty}
                onChange={(e) => setQty(Math.min(999, Math.max(1, Number(e.target.value.replace(/\D/g, '')) || 1)))}
                onFocus={(e) => e.target.select()}
                aria-label={tr('Cantidad', 'Quantity')}
              />
              <button type="button" onClick={() => setQty(Math.min(999, qty + 1))} disabled={qty >= 999} aria-label={tr('Una más', 'One more')}>
                <Plus size={18} />
              </button>
            </div>
          </Field>
        </div>
        <Field label="IVA">
          <Segmented value={vat} onChange={setVat} options={[...new Set([...VAT_RATES, vat])].map((r) => ({ value: r, label: rate(r) }))} />
        </Field>

        {canSave && (
          <div className="item-save">
            <Toggle
              checked={save}
              onChange={setSave}
              label={same ? tr('Actualizar el producto con este precio', 'Update the product with this price') : tr('Guardar también como producto', 'Also save as a product')}
              hint={
                same
                  ? tr('Ya hay un producto con este nombre en el catálogo.', 'There is already a product with this name in the catalogue.')
                  : tr('Saldrá al buscar y en «Todos» para venderlo otra vez.', 'It will show up in search and under "All" to sell it again.')
              }
            />
            {save && !same && (
              <div className="item-save-more">
                <Toggle checked={favorite} onChange={setFavorite} label={tr('Botón rápido', 'Quick button')} hint={tr('En «Favoritos», abajo en la venta', 'Under "Favourites" on the sale screen')} />
                <Field label={tr('Código de barras (opcional)', 'Barcode (optional)')}>
                  <input className="mono" value={barcode} onChange={(e) => setBarcode(e.target.value)} maxLength={40} onKeyDown={(e) => e.key === 'Enter' && e.preventDefault()} />
                </Field>
              </div>
            )}
          </div>
        )}
        {editing && admin && product && priceChanged && cents !== null && cents > 0 && (
          <Toggle
            checked={updateProduct}
            onChange={setUpdateProduct}
            label={tr('Cambiar también el precio del producto', 'Also change the product price')}
            hint={`${tr('En el catálogo cuesta', 'The catalogue price is')} ${eur(product.priceCents)}`}
          />
        )}
        {!canSave && !editing && same && (
          <p className="field-hint">
            <PackagePlus size={14} /> {tr('Es un producto del catálogo: se venderá como tal.', 'It is a catalogue product: it will sell as one.')}
          </p>
        )}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}
