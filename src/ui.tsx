import { useEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { X } from 'lucide-react';
import { tr, type Lang } from './i18n';

export function Mark({ size = 28 }: { size?: number }) {
  return (
    <svg viewBox="64 64 896 896" width={size} height={size} aria-hidden="true" className="mark">
      <defs>
        <linearGradient id="mark-bg" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#4170FF" />
          <stop offset="1" stopColor="#1D3CD4" />
        </linearGradient>
      </defs>
      <rect x="64" y="64" width="896" height="896" rx="208" fill="url(#mark-bg)" />
      <path
        d="M332 236H692A40 40 0 0 1 732 276V780L677 740L622 780L567 740L512 780L457 740L402 780L347 740L292 780V276A40 40 0 0 1 332 236Z"
        fill="#fff"
      />
      <rect x="392" y="316" width="240" height="40" rx="20" fill="#2E5BFF" />
      <rect x="352" y="404" width="320" height="26" rx="13" fill="#CBD6FF" />
      <rect x="352" y="456" width="250" height="26" rx="13" fill="#CBD6FF" />
      <rect x="352" y="584" width="320" height="52" rx="14" fill="#1D3CD4" />
    </svg>
  );
}

// Keeps Tab inside an open dialog. It only steps in while focus is in this
// dialog or lost on the page, so a dialog opened on top of another keeps its
// own focus.
export function useTrap(ref: RefObject<HTMLElement>, on = true) {
  useEffect(() => {
    if (!on) return;
    const k = (e: KeyboardEvent) => {
      const box = ref.current;
      if (e.key !== 'Tab' || !box) return;
      const active = document.activeElement;
      const inside = box.contains(active);
      if (!inside && active !== document.body) return;
      const els = [...box.querySelectorAll<HTMLElement>('button, input, select, textarea, a[href], [tabindex]:not([tabindex="-1"])')].filter(
        (el) => !el.hasAttribute('disabled') && el.offsetParent !== null,
      );
      if (!els.length) return;
      const [first, last] = [els[0], els[els.length - 1]];
      if (e.shiftKey ? !inside || active === first : !inside || active === last) {
        e.preventDefault();
        (e.shiftKey ? last : first).focus();
      }
    };
    window.addEventListener('keydown', k, true);
    return () => window.removeEventListener('keydown', k, true);
  }, [ref, on]);
}

function useEscape(onClose: () => void) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener('keydown', k, true);
    return () => window.removeEventListener('keydown', k, true);
  }, [onClose]);
}

const stay = () => {};

// `locked` is for something that must be acknowledged: no X, Esc or backdrop
export function Modal(props: { title: string; onClose: () => void; children: ReactNode; actions?: ReactNode; wide?: boolean; locked?: boolean }) {
  useEscape(props.locked ? stay : props.onClose);
  const box = useRef<HTMLDivElement>(null);
  useTrap(box);
  return createPortal(
    <div className="modal-back" onMouseDown={props.locked ? undefined : props.onClose}>
      <div
        ref={box}
        className={`modal${props.wide ? ' wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h3>{props.title}</h3>
          {!props.locked && (
            <button className="icon-btn" onClick={props.onClose} aria-label={tr('Cerrar', 'Close')}>
              <X size={18} />
            </button>
          )}
        </div>
        <div className="modal-body">{props.children}</div>
        {props.actions && <div className="modal-foot">{props.actions}</div>}
      </div>
    </div>,
    document.body,
  );
}

// With `dirty`, closing it (X, Esc, a click outside) asks before throwing
// the edits away. `wide` leaves room for a preview beside the form.
export function Drawer(props: { title: string; onClose: () => void; children: ReactNode; footer?: ReactNode; dirty?: boolean; wide?: boolean }) {
  const [asking, setAsking] = useState(false);
  const close = () => (props.dirty ? setAsking(true) : props.onClose());
  useEscape(close);
  const box = useRef<HTMLElement>(null);
  useTrap(box);
  return createPortal(
    <div className="modal-back drawer-back" onMouseDown={close}>
      <aside
        ref={box}
        className={`drawer${props.wide ? ' wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={props.title}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h3>{props.title}</h3>
          <button className="icon-btn" onClick={close} aria-label={tr('Cerrar', 'Close')}>
            <X size={18} />
          </button>
        </div>
        <div className="drawer-body">{props.children}</div>
        {asking ? (
          <div className="drawer-foot discard" role="alertdialog" aria-label={tr('¿Descartar los cambios?', 'Discard the changes?')}>
            <span>{tr('¿Descartar los cambios?', 'Discard the changes?')}</span>
            <div className="row-gap">
              <button className="btn btn-ghost" autoFocus onClick={() => setAsking(false)}>
                {tr('Seguir editando', 'Keep editing')}
              </button>
              <button className="btn btn-danger" onClick={props.onClose}>
                {tr('Descartar', 'Discard')}
              </button>
            </div>
          </div>
        ) : (
          props.footer && <div className="drawer-foot">{props.footer}</div>
        )}
      </aside>
    </div>,
    document.body,
  );
}

export function Field(props: { label: string; hint?: string; children: ReactNode; error?: string }) {
  return (
    <label className="field">
      <span className="field-label">{props.label}</span>
      {props.children}
      {props.error ? <span className="field-error">{props.error}</span> : props.hint && <span className="field-hint">{props.hint}</span>}
    </label>
  );
}

export function Toggle(props: { checked: boolean; onChange: (v: boolean) => void; label: string; hint?: string }) {
  return (
    <label className="toggle">
      <input type="checkbox" checked={props.checked} onChange={(e) => props.onChange(e.target.checked)} />
      <span className="toggle-track" aria-hidden="true">
        <span className="toggle-thumb" />
      </span>
      <span className="toggle-text">
        <span>{props.label}</span>
        {props.hint && <small>{props.hint}</small>}
      </span>
    </label>
  );
}

export function Segmented<T extends string | number>(props: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
}) {
  return (
    <div className="segmented" role="radiogroup">
      {props.options.map((o) => (
        <button
          type="button"
          key={String(o.value)}
          role="radio"
          aria-checked={props.value === o.value}
          className={props.value === o.value ? 'on' : ''}
          onClick={() => props.onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function Empty(props: { icon: ReactNode; title: string; text?: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <div className="empty-icon">{props.icon}</div>
      <h3>{props.title}</h3>
      {props.text && <p>{props.text}</p>}
      {props.children}
    </div>
  );
}

export function LangSwitch({ lang, onLang }: { lang: Lang; onLang: (l: Lang) => void }) {
  return (
    <div className="lang-switch" role="radiogroup" aria-label="Idioma / Language">
      {(['es', 'en'] as const).map((l) => (
        <button key={l} type="button" role="radio" aria-checked={lang === l} className={lang === l ? 'on' : ''} onClick={() => onLang(l)}>
          {l.toUpperCase()}
        </button>
      ))}
    </div>
  );
}
