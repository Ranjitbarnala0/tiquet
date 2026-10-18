import { invoke } from '@tauri-apps/api/core';

// inside the desktop app, as opposed to a browser on the devserver
export const native = '__TAURI_INTERNALS__' in window;

export class ApiError extends Error {
  constructor(public code: string, public detail: string) {
    super(detail ? `${code}: ${detail}` : code);
  }
}

export function toError(e: unknown): ApiError {
  const raw = typeof e === 'string' ? e : e instanceof Error ? e.message : JSON.stringify(e);
  if (raw.startsWith('E_')) {
    const i = raw.indexOf(':');
    return i < 0 ? new ApiError(raw, '') : new ApiError(raw.slice(0, i), raw.slice(i + 1));
  }
  return new ApiError('E_INTERNAL', raw);
}

// called when the core says nobody is signed in any more (locked after
// being left unused, or signed out elsewhere)
let lost: ((code: string) => void) | null = null;

export function onSessionLost(f: ((code: string) => void) | null) {
  lost = f;
}

// In the app this goes over Tauri IPC; in a plain browser it hits the
// devserver, which runs the same Rust handlers.
export async function call<T = unknown>(cmd: string, args: object = {}): Promise<T> {
  try {
    if (native) return await invoke<T>('call', { cmd, args });
    const res = await fetch(`/api/${cmd}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(args),
    });
    const body = await res.json();
    if (!res.ok) throw body.error;
    return body as T;
  } catch (e) {
    const err = e instanceof ApiError ? e : toError(e);
    if (lost && (err.code === 'E_SESSION' || err.code === 'E_SESSION_IDLE')) lost(err.code);
    throw err;
  }
}
