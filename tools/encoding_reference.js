#!/usr/bin/env node
// Record Chromium's Encoding Standard behavior for comparison with
// rustic-encoding (docs/ENCODING.md, "Reference comparison").
//
//   node tools/encoding_reference.js /tmp/encoding-reference.txt
//   cargo run --release -p rustic-encoding --example compare_reference -- /tmp/encoding-reference.txt
//
// Needs Playwright with Chromium. Output lines (about 1.5 GB in total):
//   D <encoding> <input hex> <code points>      TextDecoder, replacement mode
//   E <encoding> <code point> <query>           <a href> query encoding of one scalar
//   S <encoding> <code points> <query>          the same for a random string
// The file is not committed; record the Chromium version with the results.
const { chromium } = require('playwright');
const fs = require('fs');

const NAMES = ["UTF-8", "IBM866", "ISO-8859-2", "ISO-8859-3", "ISO-8859-4", "ISO-8859-5",
  "ISO-8859-6", "ISO-8859-7", "ISO-8859-8", "ISO-8859-8-I", "ISO-8859-10", "ISO-8859-13",
  "ISO-8859-14", "ISO-8859-15", "ISO-8859-16", "KOI8-R", "KOI8-U", "macintosh", "windows-874",
  "windows-1250", "windows-1251", "windows-1252", "windows-1253", "windows-1254", "windows-1255",
  "windows-1256", "windows-1257", "windows-1258", "x-mac-cyrillic", "GBK", "gb18030", "Big5",
  "EUC-JP", "ISO-2022-JP", "Shift_JIS", "EUC-KR", "UTF-16BE", "UTF-16LE", "x-user-defined"];
const MULTI_BYTE = ["GBK", "gb18030", "Big5", "EUC-JP", "ISO-2022-JP", "Shift_JIS", "EUC-KR",
  "UTF-16BE", "UTF-16LE", "UTF-8"];

(async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const stream = fs.createWriteStream(process.argv[2]);
  console.log(`Chromium ${browser.version()}`);
  for (const name of NAMES) {
    const lines = await page.evaluate(([name, multi]) => {
      let s = 0x9E3779B9 >>> 0;
      const rnd = () => { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; return s; };
      const hex = (a) => Array.from(a, b => b.toString(16).padStart(2, '0')).join('');
      const cps = (t) => Array.from(t, c => c.codePointAt(0).toString(16)).join(',');
      const alphabets = [[0x1B, 0x24, 0x28, 0x42, 0x4A, 0x49, 0x40, 0x21, 0x7E, 0x0E, 0x30, 0x81, 0xA1, 0xFE, 0x5C, 0x41, 0x8E, 0x8F, 0xDF],
        null,
        [0x81, 0x82, 0x30, 0x39, 0x31, 0x84, 0xFE, 0xE3, 0x32, 0x9A, 0x35, 0x40, 0x7F, 0x80, 0xA0, 0xD8, 0xDC, 0x00, 0xED, 0xF0, 0xF4, 0x90, 0xBF, 0xC2]];
      const decoder = new TextDecoder(name, { ignoreBOM: true });
      const lines = [];
      const add = (bytes) => lines.push(`D ${name} ${hex(bytes)} ${cps(decoder.decode(bytes))}`);
      if (multi) {
        for (let a = 0; a < 256; a++) for (let b = 0; b < 256; b++) {
          add(new Uint8Array([a, b]));
          add(new Uint8Array([a, b, 0x30, 0x81, 0x30]));
          add(new Uint8Array([a, b, 0x41]));
        }
      } else {
        for (let a = 0; a < 256; a++) add(new Uint8Array([a]));
      }
      for (let i = 0; i < 30000; i++) {
        const length = rnd() % 24, alphabet = alphabets[i % 3];
        const bytes = new Uint8Array(length);
        for (let j = 0; j < length; j++) bytes[j] = alphabet ? alphabet[rnd() % alphabet.length] : rnd() & 0xFF;
        add(bytes);
      }
      return lines;
    }, [name, MULTI_BYTE.includes(name)]);
    stream.write(lines.join('\n') + '\n');
  }
  for (const name of NAMES) {
    if (name.startsWith('UTF-16')) continue;
    await page.goto(`data:text/html;charset=${encodeURIComponent(name)},<meta charset="${name}">`);
    for (let start = 0x80; start <= 0x10FFFF; start += 65536) {
      const lines = await page.evaluate(([name, start]) => {
        if (document.characterSet.toLowerCase() !== name.toLowerCase()) throw new Error(`${name}: ${document.characterSet}`);
        const a = document.createElement('a');
        const lines = [];
        for (let cp = start; cp < Math.min(start + 65536, 0x110000); cp++) {
          if (cp >= 0xD800 && cp <= 0xDFFF) continue;
          a.href = 'http://h/?' + String.fromCodePoint(cp);
          lines.push(`E ${name} ${cp.toString(16)} ${a.href.slice(10)}`);
        }
        return lines;
      }, [name, start]);
      stream.write(lines.join('\n') + '\n');
    }
    const lines = await page.evaluate((name) => {
      const a = document.createElement('a');
      let s = 12345;
      const rnd = () => { s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0; return s; };
      const pool = Array.from("a\\~¥‾−ｱﾞ€\u{E5E5}\u{E7C7}\u{E78D}漢字한국어中文\u{20000}\u{10FFFF}╞十卅Êb");
      const lines = [];
      for (let i = 0; i < 5000; i++) {
        const length = rnd() % 10;
        let text = '';
        for (let j = 0; j < length; j++) text += pool[rnd() % pool.length];
        a.href = 'http://h/?' + text;
        lines.push(`S ${name} ${Array.from(text, c => c.codePointAt(0).toString(16)).join(',')} ${a.href.slice(10)}`);
      }
      return lines;
    }, name);
    stream.write(lines.join('\n') + '\n');
  }
  await new Promise((resolve) => stream.end(resolve));
  await browser.close();
})();
