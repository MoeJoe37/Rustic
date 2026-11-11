var x = 1, y = 2;
let a = [1, , 2, ...b];
const {p, q: [r = 1, ...s], ...t} = o;
function f(a, b = 1, ...c) { return a + b; }
function* g() { yield 1; yield* h(); }
async function af() { await p; for await (const x of y) {} }
async function* ag() { yield await x; }
class A extends B { #x = 1; static #y; constructor() { super(); } get v() { return this.#x; } static { this.z = 1; } }
class C { static async *m() {} 'constructor'() {} [k] = 2; #m() { return #m in this; } }
label: for (;;) { break label; }
outer: while (true) { inner: do { continue outer; } while (false); }
for (var i = 0; i < 10; i++) {}
for (let [k, v] of map) {}
for (const k in obj) {}
for (x.y of z) {}
for (let in obj) {}
switch (x) { case 1: break; default: }
try { throw e; } catch ({message}) {} finally {}
try {} catch {}
if (a) b; else if (c) d; else e;
with (obj) prop;
debugger;
x = a ? b : c;
x ??= y || z && w;
a ?? (b || c);
a?.b?.[c]?.(d);
new a.b.c(d)?.e;
new new X()();
new.target;
import.meta.url;
import('x');
a ** -b;
(-a) ** b;
typeof x === 'undefined';
delete obj[key];
void 0;
x++ + ++y - --z;
a = b = c;
[a, b] = [b, a];
({a, b: {c}} = d);
({ a = 1 } = {});
(a, b) => a + b;
async (x) => await x;
async x => x;
x => ({ y: x });
() => {};
(a = 1, {b}, [c], ...d) => 0;
`template ${a} and ${`nested ${b}`}`;
tag`raw \u{zz} ok`;
/re[/]gex/gimsuy;
/(?<name>x)\k<name>/v;
a / b / c;
x = y / 2 / z;
0x1F + 0o17 + 0b101 + 1_000_000 + .5e-3 + 10n + 0xFFn;
017 + 08 + 09.5;
'\x41\u0041\u{1F600}\0';
'legacy \101 octal';
"line\
continuation";
<!-- html comment
--> html close comment
#!/usr/bin/env node
obj = { get a() {}, set a(v) {}, async *b() {}, [c]: 1, 'd': 2, 3: 4, __proto__: null };
obj = { get, set, async, await, yield, static: 1 };
let \u0061bc = 1;
var ℮ = 2, ゛= 3, $_ = 4;
a
++b
return
x
let
y = 1
var await = 1, yield = 2;
async function f() { var await; }
function f() { "use strict"; with (x) {} }
function f(a, a) { "use strict"; }
"use strict"; var eval = 1;
"use strict"; 010;
"use strict"; delete x;
let let = 1;
let x; let x;
const y;
for (let x of a, b) {}
class A { constructor() {} constructor() {} }
class B { #a; #a; }
this.#undeclared;
super();
break;
continue;
new.target;
return;
x = { a = 1 };
(a, b) => {}; (a, a) => {};
async (await) => {};
function* g() { (yield) => {}; }
a ?? b || c;
-a ** b;
a?.b = 1;
a?.`t`;
import x from 'y';
export default function () {}
export { a as b };
export * as ns from 'mod';
import { default as d, "string name" as s } from "m" with { type: "json" };
for (async of x) {}
async () => { for await (x of y) {} };
yield
let [a] = [1], {b} = {b: 2};
({ __proto__: a, __proto__: b });
({ __proto__: a, __proto__: b } = c);
/(/;
/a/gg;
'\u{110000}';
1__0;
0_1;
1n.toString();
08n;
`unterminated
"unterminated
/* unterminated
x = function f() {} / 1;
x = {} / 1;
{} / 1 /;
if (a) function f() {}
while (a) function f() {}
lbl: function f() {}
class A { static prototype() {} }
class A { get constructor() {} }
class A { constructor() { super.x; } }
({ m() { super.x; } });
function f() { super.x; }
async function f() { await => 1; }
function f(...a,) {}
(...a,) => 0;
[...a, b] = c;
