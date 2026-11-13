// Closure creation: one environment and one function object per round,
// each called once, and a chain of composed closures.
function adder(a) { return function (b) { return a + b; }; }
function compose(f, g) { return function (x) { return f(g(x)); }; }
function run(n) {
  var sum = 0, i;
  for (i = 0; i < n; i++) { sum = (sum + adder(i)(1)) % 1000003; }
  var f = adder(0);
  for (i = 0; i < 64; i++) { f = compose(f, adder(i)); }
  for (i = 0; i < n / 64; i++) { sum = (sum + f(i)) % 1000003; }
  return sum;
}
