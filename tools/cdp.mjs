// Talks to a Tiquet page through the Chrome DevTools Protocol: the real app
// started with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>
// (hidden with --no-window), or any Chromium page with a debugging port.
// Starts no program. Used by the cdp-*.mjs tools.

export async function connect(port) {
  const pages = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
  const page = pages.find((p) => p.type === 'page');
  if (!page) throw new Error(`no page on port ${port}`);
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((ok, fail) => {
    ws.onopen = ok;
    ws.onerror = fail;
  });
  let next = 0;
  const waiting = new Map();
  ws.onmessage = (m) => {
    const d = JSON.parse(m.data);
    const w = waiting.get(d.id);
    if (!w) return;
    waiting.delete(d.id);
    if (d.error) w.fail(new Error(d.error.message));
    else w.ok(d.result);
  };
  const send = (method, params = {}, ms = 20000) =>
    new Promise((ok, fail) => {
      const id = ++next;
      waiting.set(id, { ok, fail });
      ws.send(JSON.stringify({ id, method, params }));
      setTimeout(() => waiting.has(id) && (waiting.delete(id), fail(new Error(`${method}: no answer in ${ms} ms`))), ms);
    });
  return { send, close: () => ws.close() };
}
