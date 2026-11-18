// node tools/cdp-eval.mjs <port> "<expression>"
// Evaluates an expression in the page (promises are awaited) and prints the
// result as JSON. Example, the version shown at the bottom left:
//   node tools/cdp-eval.mjs 9333 "document.body.innerText.match(/\d+\.\d+\.\d+/)?.[0]"
import { connect } from './cdp.mjs';

const [port, expression] = process.argv.slice(2);
if (!port || !expression) {
  console.error('usage: node tools/cdp-eval.mjs <port> "<expression>"');
  process.exit(2);
}
const page = await connect(port);
const r = await page.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
page.close();
if (r.exceptionDetails) {
  console.error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  process.exit(1);
}
console.log(JSON.stringify(r.result?.value ?? null));
