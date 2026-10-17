import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { Archive, FileSpreadsheet, Package, Plus, Search, Star } from 'lucide-react';
import { call } from './api';
import { parseCsv, toProducts, type ProductIn } from './csv';
import { eur, fmt, norm, parseMoney, rate } from './format';
import { errorText, tr } from './i18n';
import { useToast } from './toast';
import type { Product, Settings } from './types';
import { Drawer, Empty, Field, Modal, Toggle } from './ui';

const RATES = [2100, 1000, 400, 0];

interface Draft {
  id: number;
  name: string;
  price: string;
  vatBp: number;
  barcode: string;
  favorite: boolean;
  active: boolean;
}

export function Products({ settings, onChanged }: { settings: Settings; onChanged: () => void }) {
  const toast = useToast();
  const [all, setAll] = useState<Product[] | null>(null);
  const [q, setQ] = useState('');
  const [archived, setArchived] = useState(false);
  const [edit, setEdit] = useState<Draft | null>(null);
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState<{ file: string; rows: ProductIn[]; skipped: number } | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  const load = useCallback(() => {
    call<Product[]>('products.list', { all: true })
      .then(setAll)
      .catch((e) => toast.error(errorText(e)));
  }, [toast]);

  useEffect(load, [load]);

  const shown = useMemo(() => {
    const tokens = norm(q).split(/\s+/).filter(Boolean);
    return (all ?? []).filter((p) => p.active !== archived && tokens.every((t) => norm(p.name).includes(t) || p.barcode.includes(t)));
  }, [all, q, archived]);

  const archivedCount = (all ?? []).filter((p) => !p.active).length;

  const open = (p?: Product) =>
    setEdit(
      p
        ? { id: p.id, name: p.name, price: fmt(p.priceCents), vatBp: p.vatBp, barcode: p.barcode, favorite: p.favorite, active: p.active }
        : { id: 0, name: '', price: '', vatBp: settings.defaultVatBp, barcode: '', favorite: false, active: true },
    );

  const price = edit ? parseMoney(edit.price || '0') : null;
  const valid = !!edit && edit.name.trim().length > 0 && price !== null && price >= 0;

  const save = async (d: Draft, patch: Partial<Draft> = {}) => {
    const next = { ...d, ...patch };
    const cents = parseMoney(next.price || '0');
    if (busy || !next.name.trim() || cents === null || cents < 0) return;
    setBusy(true);
    try {
      await call<Product>('products.save', {
        id: next.id,
        name: next.name,
        priceCents: cents,
        vatBp: next.vatBp,
        barcode: next.barcode,
        favorite: next.favorite,
        active: next.active,
      });
      toast.ok(
        patch.active === false
          ? tr('Producto archivado', 'Product archived')
          : patch.active === true
            ? tr('Producto restaurado', 'Product restored')
            : tr('Producto guardado', 'Product saved'),
      );
      setEdit(null);
      load();
      onChanged();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const toggleFav = async (p: Product) => {
    try {
      await call('products.save', { ...p, favorite: !p.favorite });
      load();
      onChanged();
    } catch (e) {
      toast.error(errorText(e));
    }
  };

  const pickFile = async (f: File | undefined) => {
    if (!f) return;
    const text = await f.text();
    const r = toProducts(parseCsv(text), settings.defaultVatBp);
    if (!r.rows.length) {
      toast.error(tr('No se encontraron productos en el archivo', 'No products found in the file'));
      return;
    }
    setImporting({ file: f.name, ...r });
  };

  const doImport = async () => {
    if (!importing || busy) return;
    setBusy(true);
    try {
      const r = await call<{ inserted: number; updated: number }>('products.import', { rows: importing.rows });
      toast.ok(tr(`${r.inserted} nuevos, ${r.updated} actualizados`, `${r.inserted} new, ${r.updated} updated`));
      setImporting(null);
      load();
      onChanged();
    } catch (e) {
      toast.error(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (edit && valid) save(edit);
  };

  return (
    <div className="page">
      <div className="page-head">
        <div>
          <h1>{tr('Productos', 'Products')}</h1>
          <p className="muted">
            {(all ?? []).length - archivedCount} {tr('activos', 'active')}
            {archivedCount > 0 && (
              <>
                {' · '}
                <button className="link" onClick={() => setArchived(!archived)}>
                  {archived ? tr('ver activos', 'show active') : `${archivedCount} ${tr('archivados', 'archived')}`}
                </button>
              </>
            )}
          </p>
        </div>
        <div className="page-actions">
          <div className="search-sm">
            <Search size={16} />
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={tr('Buscar por nombre o código', 'Search by name or code')} />
          </div>
          <input ref={fileRef} type="file" accept=".csv,.txt,text/csv" hidden onChange={(e) => {
            pickFile(e.target.files?.[0]);
            e.target.value = '';
          }} />
          <button className="btn" onClick={() => fileRef.current?.click()}>
            <FileSpreadsheet size={16} />
            {tr('Importar CSV', 'Import CSV')}
          </button>
          <button className="btn btn-primary" onClick={() => open()}>
            <Plus size={16} />
            {tr('Nuevo producto', 'New product')}
          </button>
        </div>
      </div>

      <div className="table-card">
        {all && shown.length === 0 ? (
          <Empty
            icon={<Package size={28} />}
            title={q ? tr('Nada coincide con la búsqueda', 'Nothing matches the search') : tr('Aún no hay productos', 'No products yet')}
            text={q ? undefined : tr('Crea el primero o importa una hoja de cálculo con nombre y precio.', 'Create the first one or import a spreadsheet with name and price.')}
          />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th className="w-star" />
                <th>{tr('Nombre', 'Name')}</th>
                <th>{tr('Código', 'Code')}</th>
                <th className="num">IVA</th>
                <th className="num">{tr('Precio', 'Price')}</th>
                <th className="num">{tr('Vendidos', 'Sold')}</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((p) => (
                <tr key={p.id} onClick={() => open(p)} className="clickable">
                  <td className="w-star">
                    <button
                      className={`star-btn${p.favorite ? ' on' : ''}`}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleFav(p);
                      }}
                      aria-label={tr('Favorito', 'Favorite')}
                      title={tr('Botón rápido en Venta', 'Quick button on Sale')}
                    >
                      <Star size={16} />
                    </button>
                  </td>
                  <td className="strong">{p.name}</td>
                  <td className="mono muted">{p.barcode || '-'}</td>
                  <td className="num muted">{rate(p.vatBp)}</td>
                  <td className="num strong">{p.priceCents ? eur(p.priceCents) : <span className="badge">{tr('Libre', 'Open')}</span>}</td>
                  <td className="num muted">{p.soldQty}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {edit && (
        <Drawer
          title={edit.id ? tr('Editar producto', 'Edit product') : tr('Nuevo producto', 'New product')}
          onClose={() => setEdit(null)}
          footer={
            <>
              {edit.id ? (
                <button className="btn btn-ghost" onClick={() => save(edit, { active: !edit.active })} disabled={busy}>
                  <Archive size={16} />
                  {edit.active ? tr('Archivar', 'Archive') : tr('Restaurar', 'Restore')}
                </button>
              ) : (
                <span />
              )}
              <button className="btn btn-primary" onClick={() => save(edit)} disabled={!valid || busy}>
                {tr('Guardar', 'Save')}
              </button>
            </>
          }
        >
          <form onSubmit={submit} className="stack">
            <Field label={tr('Nombre', 'Name')}>
              <input autoFocus value={edit.name} onChange={(e) => setEdit({ ...edit, name: e.target.value })} maxLength={120} />
            </Field>
            <div className="grid2">
              <Field
                label={tr('Precio con IVA', 'Price incl. VAT')}
                hint={tr('0 = se pide el precio al vender', '0 = asks for the price at the till')}
                error={price === null ? tr('Precio no válido', 'Invalid price') : undefined}
              >
                <div className="money-input">
                  <input value={edit.price} onChange={(e) => setEdit({ ...edit, price: e.target.value })} inputMode="decimal" placeholder="0,00" />
                  <span>€</span>
                </div>
              </Field>
              <Field label="IVA">
                <select value={edit.vatBp} onChange={(e) => setEdit({ ...edit, vatBp: Number(e.target.value) })}>
                  {[...new Set([...RATES, edit.vatBp])].map((r) => (
                    <option key={r} value={r}>
                      {rate(r)}
                    </option>
                  ))}
                </select>
              </Field>
            </div>
            <Field label={tr('Código de barras', 'Barcode')} hint={tr('Opcional. Haz clic aquí y escanéalo.', 'Optional. Click here and scan it.')}>
              <input
                value={edit.barcode}
                onChange={(e) => setEdit({ ...edit, barcode: e.target.value })}
                className="mono"
                onKeyDown={(e) => e.key === 'Enter' && e.preventDefault()}
              />
            </Field>
            <Toggle
              checked={edit.favorite}
              onChange={(v) => setEdit({ ...edit, favorite: v })}
              label={tr('Botón rápido', 'Quick button')}
              hint={tr('Aparece en la pantalla de venta para añadirlo con un clic', 'Shows on the sale screen to add it with one click')}
            />
            <button type="submit" hidden />
          </form>
        </Drawer>
      )}

      {importing && (
        <Modal
          wide
          title={tr('Importar productos', 'Import products')}
          onClose={() => setImporting(null)}
          actions={
            <>
              <button className="btn btn-ghost" onClick={() => setImporting(null)}>
                {tr('Cancelar', 'Cancel')}
              </button>
              <button className="btn btn-primary" onClick={doImport} disabled={busy}>
                {tr(`Importar ${importing.rows.length}`, `Import ${importing.rows.length}`)}
              </button>
            </>
          }
        >
          <p className="muted">
            {importing.file}: {importing.rows.length} {tr('productos', 'products')}
            {importing.skipped > 0 &&
              ` · ${tr(`se ignoran ${importing.skipped} fila(s) sin nombre o precio`, `${importing.skipped} row(s) without a name or price will be skipped`)}`}
            .{' '}
            {tr('Si un producto ya existe (mismo código o nombre) se actualiza su precio.', 'Existing products (same code or name) get their price updated.')}
          </p>
          <div className="table-card inset">
            <table className="table compact">
              <thead>
                <tr>
                  <th>{tr('Nombre', 'Name')}</th>
                  <th>{tr('Código', 'Code')}</th>
                  <th className="num">IVA</th>
                  <th className="num">{tr('Precio', 'Price')}</th>
                </tr>
              </thead>
              <tbody>
                {importing.rows.slice(0, 8).map((r, i) => (
                  <tr key={i}>
                    <td>{r.name}</td>
                    <td className="mono muted">{r.barcode || '-'}</td>
                    <td className="num">{rate(r.vatBp)}</td>
                    <td className="num">{eur(r.priceCents)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {importing.rows.length > 8 && <p className="muted small pad">{tr(`y ${importing.rows.length - 8} más`, `and ${importing.rows.length - 8} more`)}</p>}
          </div>
        </Modal>
      )}
    </div>
  );
}
