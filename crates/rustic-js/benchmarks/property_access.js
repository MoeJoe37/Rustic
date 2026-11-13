// Property reads and writes at one shape (monomorphic) and at four
// shapes in turn (polymorphic).
function Point(x, y) { this.x = x; this.y = y; }
function run(n) {
  var p = new Point(1, 2), sum = 0, i;
  for (i = 0; i < n; i++) { p.x = p.x + p.y; p.y = i & 7; sum += p.x; }
  var shapes = [
    { a: 1, v: 1 }, { b: 2, v: 2 }, { c: 3, d: 4, v: 3 }, { v: 4, e: 5 }
  ];
  for (i = 0; i < n; i++) { sum += shapes[i & 3].v; }
  return sum % 1000000007;
}
