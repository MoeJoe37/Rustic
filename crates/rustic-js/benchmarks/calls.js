// Calls and recursion: a tight leaf call, a method call and naive
// recursive Fibonacci.
function add(a, b) { return a + b; }
function fib(k) { return k < 2 ? k : fib(k - 1) + fib(k - 2); }
var counter = { n: 0, bump: function (d) { this.n += d; return this.n; } };
function run(n) {
  var sum = 0, i;
  for (i = 0; i < n; i++) { sum = add(sum, i) % 65521; }
  counter.n = 0;
  for (i = 0; i < n; i++) { counter.bump(i & 3); }
  // fib's work doubles about every 1.44 steps of depth, so the depth
  // grows with the logarithm of n to keep the workload linear.
  var depth = 0, m = n;
  while (m > 1) { depth++; m = Math.floor(m / 1.618); }
  return sum + counter.n + fib(Math.min(depth, 30));
}
