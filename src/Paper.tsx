import type { CSSProperties } from 'react';
import type { Logo, PLine } from './types';

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
export function Paper({ lines, logo, cols, stamp }: { lines: PLine[]; logo: Logo | null; cols: number; stamp?: string }) {
  return (
    <div className="paper-wrap">
      <div className="paper-shadow">
        <div className="paper" style={{ '--cols': cols } as CSSProperties}>
          {lines.map((l, i) =>
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
                {l.text || ' '}
              </div>
            ),
          )}
          {stamp && <div className="paper-stamp">{stamp}</div>}
        </div>
      </div>
    </div>
  );
}
