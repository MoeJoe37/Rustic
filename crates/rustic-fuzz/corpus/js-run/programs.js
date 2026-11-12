var a = 1, b = 2; a + b * 3
let s = ''; for (let i = 0; i < 5; i++) s += i; s
function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2) } fib(12)
var o = {x: 1, y: 2}; o.z = o.x + o.y; delete o.x; o.z + '' + o.x
function P(a) { this.a = a } P.prototype.get = function () { return this.a }; new P(4).get() + new P(5).get()
var t = 0; for (var i = 0; i < 100; i++) { var q = {v: i}; t += q.v } t
var arr = [1, 2, 3], sum = 0; for (var v of arr) sum += v; sum
var keys = ''; for (var k in {a: 1, b: 2, 3: 0}) keys += k; keys
try { null.x } catch (e) { e.name + ': ' + e.message }
try { throw 1 } catch (e) { e } finally { 2 }
function f() { try { return 1 } finally { g = 2 } } var g; f() + g
var x = 0; do { x += 2 } while (x < 9); x
outer: for (var i = 0; i < 4; i++) { for (var j = 0; j < 4; j++) { if (j == 2) continue outer; if (i == 3) break outer } } i + j
switch (3) { case 1: 'a'; break; case 3: 'c'; default: 'd' }
var c = (function () { var n = 0; return function () { return ++n } })(); c(); c(); c()
var {a, b: [c, d = 5], ...rest} = {a: 1, b: [2], e: 3, f: 4}; a + c + d + rest.e + rest.f
var [p, , q = 9, ...r] = 'abcdef'; p + q + r.length
function args() { return arguments.length + arguments[1] } args(1, 2, 3)
var o = {get x() { return this.y * 2 }, set x(v) { this.y = v }, y: 1}; o.x = 4; o.x
var proto = {m() { return 'p' }}, o = {__proto__: proto}; var r = o.m(); proto.m = () => 'q'; r + o.m()
var top = {x: 1}, mid = {__proto__: top}, o = {__proto__: mid}; var r = o.x; mid.x = 2; r + o.x
function get(o) { return o.x } get({x: 1}) + get({y: 0, x: 2}) + get({__proto__: {x: 3}})
function put(o) { o.x = 1; return o } var p = {}; put({__proto__: p}); p = {set x(v) { hit = v }}; var hit; put({__proto__: p}).x + '' + hit
'use strict'; var o = {}; try { undefinedName = 1 } catch (e) { e.name }
typeof nothing + typeof null + typeof 1 + typeof '' + typeof {} + typeof function () {}
1 / 0 + -1 / 0 + 0 / 0 + (5 % 3) + (-5 % 3) + 2 ** 10
(1 << 31) + (1 >>> 0) + (-1 >>> 28) + (7 & 3 | 8 ^ 1) + ~5
'a' < 'b' && 1 <= 1 && '10' > 9 && null == undefined && null !== undefined
var u; u ?? 'd'
var n = null; n?.x?.y + '' + n?.[0]
var i = 5; i++ + ++i + i-- + --i
var s = 'x'; s += 1; s += null; s += {}; s
`a${1 + 1}b${'c'}`
var o = {['k' + 1]: 1, 2: 2, k3: 3}; o.k1 + o[2] + o['k3']
with ({w: 1}) { var w2 = w + 1 } w2
var f = function g(n) { return n ? n * g(n - 1) : 1 }; f(6)
var add = (a, b = 2, ...c) => a + b + c.length; add(1) + add(1, 1, 1, 1)
function C() { if (!new.target) return 'call'; this.k = 1 } C() + new C().k
var o = {a: {b: {c: 1}}}; o.a.b.c += 2; o.a.b.c *= 3; o.a.b.c
var a = []; a[0] = 1; a[5] = 2; a.length + '' + a[3]
var a = [1, 2, 3]; a.length = 1; a[1] + '' + a.length
var it = [1, 2][Symbol.iterator](); it.next().value + it.next().value + it.next().done
var r = ''; for (var ch of 'héy') r = ch + r; r
for (var x of [1, 2, 3]) { if (x == 2) break } x
var n = 0; while (true) { if (++n > 50) break } n
var o = {}; for (var i = 0; i < 40; i++) o['p' + i] = i; o.p39 + o.p0
var o = {}; for (var i = 0; i < 40; i++) o['p' + i] = i; for (var i = 0; i < 40; i += 2) delete o['p' + i]; var c = 0; for (var k in o) c++; c
function deep(n) { return n ? deep(n - 1) + 1 : 0 } deep(150)
function loop() { return loop() } try { loop() } catch (e) { e.name }
var list = null; for (var i = 0; i < 300; i++) list = {next: list, v: i}; var t = 0; while (list) { t += list.v; list = list.next } t
var s = ''; for (var i = 0; i < 200; i++) s += 'ab'; s.length
var a = []; for (var i = 0; i < 300; i++) a[i] = [i]; a[299][0] + a.length
var o = {valueOf() { return 4 }, toString() { return 's' }}; o + 1 + `${o}`
var o = {toString() { throw 'boom' }}; try { '' + o } catch (e) { e }
void 0 === undefined ? (1, 2, 3) : 4
!'' + !!'a' + -'3' + +true
var x = 1; { let x = 2; { const x = 3 } } x
try { tdz; let tdz = 1 } catch (e) { e.name }
const k = 1; try { k = 2 } catch (e) { e.name }
var fns = []; for (let i = 0; i < 3; i++) fns[i] = () => i; fns[0]() + fns[1]() + fns[2]()
var fns = []; for (var i = 0; i < 3; i++) fns[i] = () => i; fns[0]() + fns[1]() + fns[2]()
function counter() { var c = 0; return {inc() { return ++c }, get() { return c }} } var k = counter(); k.inc(); k.inc(); k.get()
var o = {f() { return this === o }}; var f = o.f; o.f() + '' + (0, o.f)()
'x' in {x: 1} && !('y' in {x: 1}) && [] instanceof Array === undefined
function F() {} var f = new F(); (f instanceof F) + '' + (({}) instanceof F)
var a = 1; a ||= 2; var b = 0; b ||= 3; var c = null; c ??= 4; var d = 1; d &&= 5; a + b + c + d
var g = globalThis; g.z1 = 1; z1 + g.z1
function r() { return read } try { r() } catch (e) { e.name }
var x = 10; function sh() { var x = 20; return function () { return x } } sh()()
var o = {a: 1}; var k = 'a'; o[k]++ ; o[k] += 2; o.a
[1, [2, [3]]][1][1][0]
var q = [3, 1, 2], m = 0; for (var i = 0; i < q.length; i++) if (q[i] > m) m = q[i]; m
label: { break label }
var i = 0; for (;;) { if (i++ > 3) break } i
for (var i = 0, j = 10; i < j; i++, j--); i + j
var t = 0; for (var [a, b] of [[1, 2], [3, 4]]) t += a * b; t
var t = ''; for (var {n} of [{n: 'a'}, {n: 'b'}]) t += n; t
function d({a = 1, b} = {}, [c] = [7]) { return a + (b | 0) + c } d() + d({b: 2}, [1])
var o = {}; o.self = o; o.self.self === o
(function () { 'use strict'; return this })() === undefined
(function () { return typeof this })()
var e; try { throw {code: 7} } catch ({code}) { e = code } e
function fin() { for (var x of [1, 2]) { try { continue } finally { n2++ } } } var n2 = 0; fin(); n2
function early() { for (var x of [1, 2, 3]) { if (x == 2) return x } } early()
var it = {[Symbol.iterator]() { var i = 0; return {next() { return {done: i > 2, value: i++} }, return() { closed = 1; return {} }} }}; var closed = 0; for (var v of it) { if (v) break } closed
var big = 9007199254740991; big + 2
0.1 + 0.2
1e21 + '' + 1e-7 + 123456789012345680000
'5' * '2' + ('5' - 2) + '5' + 2
NaN == NaN || Infinity > 1e308
var a = 5, b = 3; a = a ^ b; b = a ^ b; a = a ^ b; a * 10 + b
var s = 0; for (var i = 0; i < 400; i++) { if (i % 3 == 0) continue; s += i & 7 } s
var h = 0; for (var i = 0; i < 500; i++) h = (h * 31 + i) | 0; h
function mk(i) { return {a: i, b: i + 1, c: i + 2} } var t = 0; for (var i = 0; i < 200; i++) { var o = mk(i); t += o.a + o.b + o.c } t
function A() { this.x = 1 } function B() { this.x = 2; this.y = 3 } var t = 0; for (var i = 0; i < 50; i++) { t += (i % 2 ? new A() : new B()).x } t
var o = {x: 1}; function rd() { return o.x } rd(); o = {get x() { return 9 }}; rd()
function Q() {} Q.prototype.v = 1; var q = new Q(); var r = q.v; Q.prototype = {v: 2}; r + q.v + new Q().v
var p = {}; var o = {__proto__: p}; function g2(o) { return o.late } g2(o); p.late = 3; g2(o)
var a = {__proto__: null}; a.x = 1; typeof a.toString + a.x
var z; z = z + 1; z !== z
var x = 'outer'; function h2() { return x; var x } h2()
if (true) function decl() {} typeof decl
var f3 = () => { return }; f3()
var arr = [,1,,2]; var c = 0; for (var k in arr) c += +k; c
var o = {a: 1, b: 2}; for (var k in o) { delete o.b } k
var str = 'abc'; str[1] + str.length + str[5]
var x = 3; x **= 2; x <<= 1; x >>= 2; x >>>= 0; x %= 3; x
var t = 0; for (var i = 0; i < 30; i++) { try { if (i % 5 == 0) throw i; t += 1 } catch (e) { t += e } finally { t -= 1 } } t
function F() {} var p = F.prototype; p === F.prototype && p.constructor === F
function F() {} F.x = 1; var o = new F(); F.prototype.y = 2; o.y + F.x + (o instanceof F)
function F() {} function g(f) { return f.prototype } g(F); F.prototype = {z: 3}; g(F).z + new F().z
var fs = []; for (var i = 0; i < 50; i++) fs[i] = function () {}; var n = 0; for (var f of fs) { if (f.prototype.constructor === f) n++ } n
class A { constructor(x) { this.x = x } get d() { return this.x * 2 } m() { return this.x + 1 } static s() { return 's' } } var a = new A(3); a.m() + a.d + A.s()
class A { constructor(x) { this.x = x } m() { return 'A' + this.x } } class B extends A { constructor(x) { super(x * 2); this.y = 1 } m() { return 'B' + super.m() + this.y } } new B(2).m()
class A { constructor(...a) { this.n = a.length } } class B extends A {} new B(1, 2, 3).n + (new B() instanceof A)
class A {} class B extends A { constructor() { var init = () => super(); var get = () => this; init(); this.ok = get() === this } } new B().ok
class A {} class B extends A { constructor() { this.x = 1; super() } } try { new B() } catch (e) { e.name }
class A {} try { A() } catch (e) { e.name }
class A { x = 1; y = this.x + 1; static n = 5; static { this.m = this.n + 1 } } var a = new A(); a.x + a.y + A.n + A.m
class A { a = 1 } class B extends A { b = this.a + 1; get v() { return super.constructor === A } } var b = new B(); b.a + b.b + b.v
var k = 'dyn'; class A { [k] = 1; static [k + 's'] = 2; [k + 'm']() { return 3 } } var a = new A(); a.dyn + A.dyns + a.dynm()
class A { static s() { return 1 } } class B extends A { static s() { return super.s() + 1 } } class C extends B {} C.s() + (C.prototype instanceof A)
try { class X extends X {} } catch (e) { e.name }
var t = 0; class P { constructor(i) { this.i = i } get v() { return this.i } } for (var i = 0; i < 100; i++) t += new P(i).v; t
class A { #x = 1; #y = this.#x + 1; #m() { return this.#x * 2 } get #p() { return this.#y } set #p(v) { this.#y = v } run() { this.#p += this.#m(); this.#x++; return this.#p + this.#x } } var a = new A(); a.run() + a.run()
class A { #x; static #n = 0; static #s() { return ++A.#n } static has(o) { return (#x in o) + A.#s() } } var r = 0; for (var i = 0; i < 20; i++) r += A.has(i % 2 ? new A() : {}); r
function make() { return class { #v = 1; static read(o) { try { return o.#v } catch (e) { return -1 } } } } var A = make(), B = make(), t = 0; for (var i = 0; i < 30; i++) t += A.read(i % 3 ? new A() : new B()); t
class A { #x = 1; a() { return this.#x } } class B extends A { #x = 2; #b() { return this.#x + this.a() } go() { var f = () => this.#b(); return f() } } var s = 0; for (var i = 0; i < 25; i++) s += new B().go(); s
function tag(s, a) { return s.raw[0] + s[1] + a + s.length } var r = ''; for (var i = 0; i < 12; i++) r += tag`a${i}b`; r
var seen = []; function id(s) { seen.push(s); return s } for (var i = 0; i < 9; i++) id`x${i}y`; var same = 0; for (var s of seen) same += s === seen[0]; same
var p = { m(n) { return n + this.k } }; var o = { __proto__: p, k: 1, m(n) { return super.m(n) * 2 }, a(n) { return (() => super.m(n))() } }; var t = 0; for (var i = 0; i < 20; i++) t += o.m(i) + o.a(i); t
var o = { f(n) { return n } }, u, t = 0; for (var i = 0; i < 20; i++) t += (i % 2 ? o : u)?.f?.(i) ?? -1; t
var t = 0; function f(a = 1, b = () => a) { var a = 5; for (var i = 0; i < 9; i++) a += i; return b() + a } for (var j = 0; j < 9; j++) t += f(j); t
var s = { x: 1 }, x = 0; with (s) { for (var i = 0; i < 9; i++) { x += i; if (i == 4) delete s.x } } s.x + ',' + x
