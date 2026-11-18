import { useMemo } from 'react';

// An A4 page as the core lays it out (src-tauri/src/invoice.rs), drawn in
// millimetres: the same marks go to the PDF and to the printer, so this is
// exactly what comes out.

type Align = 'Left' | 'Right' | 'Center';

export type Mark =
  | { Text: { x: number; y: number; pt: number; bold: boolean; align: Align; text: string } }
  | { Line: { x1: number; y1: number; x2: number; y2: number; width: number } }
  | { Rect: { x: number; y: number; w: number; h: number; fill: number | null; stroke: boolean } }
  | { Image: { x: number; y: number; w: number; h: number; bits: number[]; px_w: number; px_h: number } };

export interface Sheet {
  items: Mark[];
}

const PT = 25.4 / 72;
const ANCHOR: Record<Align, 'start' | 'end' | 'middle'> = { Left: 'start', Right: 'end', Center: 'middle' };

const gray = (g: number) => {
  const v = Math.round(Math.max(0, Math.min(1, g)) * 255);
  return `rgb(${v},${v},${v})`;
};

// 1-bit rows, most significant bit first, 1 black: as a picture the browser draws
function bitmap(bits: number[], w: number, h: number): string {
  try {
    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext('2d');
    if (!ctx) return '';
    const img = ctx.createImageData(w, h);
    const stride = Math.ceil(w / 8);
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const on = (bits[y * stride + (x >> 3)] >> (7 - (x & 7))) & 1;
        const i = (y * w + x) * 4;
        const v = on ? 0 : 255;
        img.data[i] = v;
        img.data[i + 1] = v;
        img.data[i + 2] = v;
        img.data[i + 3] = 255;
      }
    }
    ctx.putImageData(img, 0, 0);
    return canvas.toDataURL('image/png');
  } catch {
    return '';
  }
}

function Picture({ m }: { m: Extract<Mark, { Image: unknown }>['Image'] }) {
  const href = useMemo(() => bitmap(m.bits, m.px_w, m.px_h), [m.bits, m.px_w, m.px_h]);
  if (!href) return null;
  return <image href={href} x={m.x} y={m.y} width={m.w} height={m.h} preserveAspectRatio="none" style={{ imageRendering: 'pixelated' }} />;
}

function Draw({ m }: { m: Mark }) {
  if ('Text' in m) {
    const t = m.Text;
    return (
      <text x={t.x} y={t.y} fontSize={t.pt * PT} fontWeight={t.bold ? 700 : 400} textAnchor={ANCHOR[t.align]} fill="#000">
        {t.text}
      </text>
    );
  }
  if ('Line' in m) {
    const l = m.Line;
    return <line x1={l.x1} y1={l.y1} x2={l.x2} y2={l.y2} stroke="#000" strokeWidth={l.width} />;
  }
  if ('Rect' in m) {
    const r = m.Rect;
    return <rect x={r.x} y={r.y} width={r.w} height={r.h} fill={r.fill === null ? 'none' : gray(r.fill)} stroke={r.stroke ? '#000' : 'none'} strokeWidth={0.2} />;
  }
  return <Picture m={m.Image} />;
}

export function SheetView({ sheets, busy }: { sheets: Sheet[]; busy?: boolean }) {
  return (
    <div className={`sheets${busy ? ' busy' : ''}`}>
      {sheets.map((s, i) => (
        <svg key={i} className="sheet" viewBox="0 0 210 297" role="img" aria-label={`Página ${i + 1}`}>
          <rect x={0} y={0} width={210} height={297} fill="#fff" />
          <g fontFamily="Helvetica, Arial, sans-serif">
            {s.items.map((m, j) => (
              <Draw key={j} m={m} />
            ))}
          </g>
        </svg>
      ))}
    </div>
  );
}
