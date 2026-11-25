import type { CSSProperties } from 'react';
import type { Logo, PLine } from './types';

type Visual =
  | { k: 'blank' }
  | { k: 'logo' }
  | { k: 'qr'; rows: string[]; module: number }
  | { k: 'rule'; heavy: boolean }
  | { k: 'badge'; text: string }
  | { k: 'pill'; text: string }
  | { k: 'total'; label: string; amount: string }
  | { k: 'heading'; text: string; big: boolean }
  | { k: 'tableHead'; text: string }
  | { k: 'tableRow'; text: string; bold: boolean }
  | { k: 'pair'; left: string; right: string; bold: boolean }
  | { k: 'center'; text: string; bold: boolean }
  | { k: 'text'; text: string; bold: boolean };

const BADGES = new Set([
  'FACTURA',
  'FACTURA SIMPLIFICADA',
  'FACTURA RECTIFICATIVA',
  'FACTURA SIMPLIFICADA RECTIFICATIVA',
  'TICKET',
  'TICKET DE DEVOLUCIÓN',
  'RESGUARDO DE REPARACIÓN',
  'JUSTIFICANTE DE ENTREGA',
  'JUSTIFICANTE DE RECARGA',
  'RESUMEN DE VENTAS',
  'CIERRE DE CAJA',
  'CÓDIGO DE RECUPERACIÓN',
  'PRUEBA DE IMPRESIÓN',
]);

function isRule(s: string, ch: string) {
  if (s.length < 8) return false;
  for (let i = 0; i < s.length; i++) if (s[i] !== ch) return false;
  return true;
}

function splitPair(text: string): [string, string] | null {
  const body = text.trimEnd();
  const trimmed = body.trimStart();
  const lead = body.length - trimmed.length;
  if (lead > 2) return null;
  const gap = trimmed.indexOf('  ');
  if (gap < 0) return null;
  const left = trimmed.slice(0, gap).trimEnd();
  const rest = trimmed.slice(gap).trimStart();
  if (!left || !rest || rest.includes('  ')) return null;
  return [body.slice(0, lead + left.length), rest];
}

function isTotalLine(l?: PLine) {
  return !!l && l.t === 'text' && l.size === 'big' && l.text.trim().startsWith('TOTAL');
}

function classify(lines: PLine[], cols: number): Visual[] {
  const out: Visual[] = [];
  let inTable = false;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.t === 'logo') {
      inTable = false;
      out.push({ k: 'logo' });
      continue;
    }
    if (line.t === 'qr') {
      inTable = false;
      out.push({ k: 'qr', rows: line.rows, module: line.module });
      continue;
    }
    const trimmed = line.text.trim();
    if (!trimmed) {
      out.push({ k: 'blank' });
      continue;
    }
    if (isRule(trimmed, '-') || isRule(trimmed, '_')) {
      inTable = false;
      out.push({ k: 'rule', heavy: false });
      continue;
    }
    if (isRule(trimmed, '=')) {
      inTable = false;
      if (!isTotalLine(lines[i + 1]) && !isTotalLine(lines[i - 1])) {
        out.push({ k: 'rule', heavy: true });
      }
      continue;
    }
    if (line.size === 'big' && trimmed.startsWith('TOTAL')) {
      inTable = false;
      const p = splitPair(trimmed);
      if (p) out.push({ k: 'total', label: p[0].trim(), amount: p[1].trim() });
      else out.push({ k: 'total', label: 'TOTAL', amount: trimmed.replace(/^TOTAL\s*/, '') });
      continue;
    }
    if (line.size !== 'normal') {
      inTable = false;
      out.push({ k: 'heading', text: trimmed, big: line.size === 'big' });
      continue;
    }
    if (line.bold && BADGES.has(trimmed)) {
      inTable = false;
      out.push({ k: 'badge', text: trimmed });
      continue;
    }
    if (line.bold && ((trimmed.startsWith('*** ') && trimmed.endsWith(' ***')) || trimmed.startsWith('PENDIENTE:') || trimmed.startsWith('COBRAR '))) {
      inTable = false;
      const inner = trimmed.startsWith('*** ') && trimmed.endsWith(' ***') ? trimmed.slice(4, -4) : trimmed;
      out.push({ k: 'pill', text: inner });
      continue;
    }
    if ((trimmed.startsWith('UDS ') && trimmed.includes('DESCRIPCI')) || (trimmed.startsWith('IVA ') && trimmed.includes('BASE'))) {
      inTable = true;
      out.push({ k: 'tableHead', text: trimmed });
      continue;
    }
    if (inTable) {
      out.push({ k: 'tableRow', text: line.text.trimEnd(), bold: line.bold });
      continue;
    }
    const pair = splitPair(line.text);
    if (pair) {
      out.push({ k: 'pair', left: pair[0], right: pair[1], bold: line.bold });
      continue;
    }
    const lead = line.text.length - line.text.trimStart().length;
    const centerPad = Math.floor(Math.max(0, cols - [...trimmed].length) / 2);
    if (lead >= 2 && lead === centerPad) {
      out.push({ k: 'center', text: trimmed, bold: line.bold });
    } else {
      out.push({ k: 'text', text: trimmed, bold: line.bold });
    }
  }
  return out;
}

// The AEAT's QR from the same modules the printer gets, four of white all
// round. A module is `module` dots and a column 12, so it is printed size.
function Qr({ rows, module }: { rows: string[]; module: number }) {
  const n = rows.length;
  const side = n + 8;
  let d = '';
  rows.forEach((r, y) => {
    for (let x = 0; x < n; x++) {
      if (r[x] !== '1') continue;
      let end = x;
      while (end < n && r[end] === '1') end++;
      d += `M${x + 4} ${y + 4}h${end - x}v1h${x - end}z`;
      x = end;
    }
  });
  return (
    <div className="pl-qr">
      <svg viewBox={`0 0 ${side} ${side}`} style={{ width: `${(side * module) / 12}ch` }} shapeRendering="crispEdges" role="img" aria-label="QR">
        <path d={d} fill="currentColor" />
      </svg>
    </div>
  );
}

// Renders the exact lines the printer receives. Font A on these printers is
// 12 dots wide on a 576-dot head, so one monospace column is one character
// and the logo width converts straight from dots to `ch`.
export function Paper({ lines, logo, cols, stamp, modern = true }: { lines: PLine[]; logo: Logo | null; cols: number; stamp?: string; modern?: boolean }) {
  return (
    <div className="paper-wrap">
      <div className="paper-shadow">
        <div className={`paper${modern ? ' paper-modern' : ''}`} style={{ '--cols': cols } as CSSProperties}>
          {modern
            ? classify(lines, cols).map((v, i) => {
                if (v.k === 'logo') {
                  return (
                    logo && (
                      <div className="pl-logo" key={i}>
                        <img src={logo.png} alt="" style={{ width: `${Math.min(logo.w, cols * 12) / 12}ch` }} />
                      </div>
                    )
                  );
                }
                if (v.k === 'qr') return <Qr key={i} rows={v.rows} module={v.module} />;
                if (v.k === 'blank') return <div key={i} className="pl-m-blank" />;
                if (v.k === 'rule') return <hr key={i} className={`pl-m-rule${v.heavy ? ' heavy' : ''}`} />;
                if (v.k === 'heading') return <div key={i} className={`pl-m-head${v.big ? ' big' : ''}`}>{v.text}</div>;
                if (v.k === 'badge') return <div key={i} className="pl-m-badge"><span>{v.text}</span></div>;
                if (v.k === 'pill') return <div key={i} className="pl-m-pill"><span>{v.text}</span></div>;
                if (v.k === 'total') return <div key={i} className="pl-m-total"><span>{v.label}</span><strong>{v.amount}</strong></div>;
                if (v.k === 'tableHead') return <div key={i} className="pl-m-th">{v.text}</div>;
                if (v.k === 'tableRow') return <div key={i} className={`pl-m-tr${v.bold ? ' b' : ''}`}>{v.text || '\u00a0'}</div>;
                if (v.k === 'pair') return <div key={i} className={`pl-m-pair${v.bold ? ' b' : ''}`}><span>{v.left}</span><span>{v.right}</span></div>;
                if (v.k === 'center') return <div key={i} className={`pl-m-center${v.bold ? ' b' : ''}`}>{v.text}</div>;
                return <div key={i} className={`pl-m-text${v.bold ? ' b' : ''}`}>{v.text}</div>;
              })
            : lines.map((l, i) =>
                l.t === 'logo' ? (
                  logo && (
                    <div className="pl-logo" key={i}>
                      <img src={logo.png} alt="" style={{ width: `${Math.min(logo.w, cols * 12) / 12}ch` }} />
                    </div>
                  )
                ) : l.t === 'qr' ? (
                  <Qr key={i} rows={l.rows} module={l.module} />
                ) : (
                  <div key={i} className={`pl ${l.size}${l.bold ? ' b' : ''}`}>
                    {l.text || '\u00a0'}
                  </div>
                ),
              )}
          {stamp && <div className="paper-stamp">{stamp}</div>}
        </div>
      </div>
    </div>
  );
}
