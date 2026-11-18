#!/usr/bin/env node
// Records how Chromium's HTML parser places nodes past its depth cap, for
// crates/rustic-html/tests/fixtures/depth-cap-chromium.txt (ADR 0046).
//
//   node tools/html_depth_reference.js OUTPUT
//
// Each case is written as "#case", the HTML on one line, "#chromium", then
// one "depth:label" line per node in tree order (the document's children at
// depth 1; elements by local name, text as a JSON string).
const fs = require('fs');
const { chromium } = require('playwright');

const cases = [
  '<!doctype html>' + '<div>'.repeat(600) + 'x' + '</div>'.repeat(600) + '<p>after',
  '<!doctype html>' + '<span>a'.repeat(520) + '<b>bold<i>it</b>tail',
  '<!doctype html>' + '<div>'.repeat(515) + '<table><tr><td>c</td></tr>foster</table>z',
  '<!doctype html>' + '<b>'.repeat(530) + '<p>para</p>end',
  '<!doctype html>' + '<ul><li>'.repeat(300) + 'deep<!--c-->' + '<em>e</em>',
];

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  let out = `# Chromium ${browser.version()}\n`;
  for (const html of cases) {
    await page.setContent(html);
    const lines = await page.evaluate(() => {
      const lines = [];
      const label = (n) =>
        n.nodeType === 1 ? n.localName
          : n.nodeType === 3 ? JSON.stringify(n.data)
          : n.nodeType === 8 ? 'comment ' + JSON.stringify(n.data)
          : n.nodeType === 10 ? 'doctype' : '#' + n.nodeType;
      const stack = [...document.childNodes].reverse().map((n) => [n, 1]);
      while (stack.length) {
        const [n, d] = stack.pop();
        lines.push(`${d}:${label(n)}`);
        for (const c of [...n.childNodes].reverse()) stack.push([c, d + 1]);
      }
      return lines.join('\n');
    });
    out += `#case\n${html}\n#chromium\n${lines}\n`;
  }
  fs.writeFileSync(process.argv[2], out);
  await browser.close();
})();
