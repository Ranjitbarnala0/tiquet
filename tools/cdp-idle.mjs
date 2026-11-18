// node tools/cdp-idle.mjs <port> [seconds]
// How much work the page does while nobody touches it. An idle Tiquet page
// should run close to 0 ms of script; 1.10.0's empty Venta page ran 4,689 ms
// in 5 s because it re-rendered without end. Leave the page alone meanwhile.
import { connect } from './cdp.mjs';

const [port, secs = '5'] = process.argv.slice(2);
if (!port) {
  console.error('usage: node tools/cdp-idle.mjs <port> [seconds]');
  process.exit(2);
}
const page = await connect(port);
await page.send('Performance.enable');
const read = async () => Object.fromEntries((await page.send('Performance.getMetrics')).metrics.map((m) => [m.name, m.value]));
const a = await read();
await new Promise((r) => setTimeout(r, Number(secs) * 1000));
const b = await read();
page.close();
const ms = (k) => Math.round((b[k] - a[k]) * 1000);
console.log(JSON.stringify({ seconds: Number(secs), scriptMs: ms('ScriptDuration'), taskMs: ms('TaskDuration'), layouts: b.LayoutCount - a.LayoutCount, styleRecalcs: b.RecalcStyleCount - a.RecalcStyleCount }));
