// node tools/cdp-shot.mjs <port> <out.png>
// A screenshot of the page as it is drawn, also when its window is hidden.
import { writeFileSync } from 'node:fs';
import { connect } from './cdp.mjs';

const [port, out] = process.argv.slice(2);
if (!port || !out) {
  console.error('usage: node tools/cdp-shot.mjs <port> <out.png>');
  process.exit(2);
}
const page = await connect(port);
const r = await page.send('Page.captureScreenshot', { format: 'png' });
page.close();
writeFileSync(out, Buffer.from(r.data, 'base64'));
console.log('saved', out);
