import type { CSSProperties } from 'react';
import type { Logo, PLine } from './types';

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
