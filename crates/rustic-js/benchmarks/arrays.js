// Arrays: push, indexed iteration, higher-order iteration and sort.
function run(n) {
  var a = [], i, seed = 12345;
  for (i = 0; i < n; i++) {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    a.push(seed % 100000);
  }
  var sum = 0;
  for (i = 0; i < a.length; i++) { sum += a[i]; }
  sum += a.map(function (x) { return x & 15; })
    .filter(function (x) { return x > 7; })
    .reduce(function (acc, x) { return acc + x; }, 0);
  a.sort(function (x, y) { return x - y; });
  for (i = 1; i < a.length; i++) { if (a[i - 1] > a[i]) { return -1; } }
  return sum + a[0] + a[a.length >> 1] + a[a.length - 1];
}
