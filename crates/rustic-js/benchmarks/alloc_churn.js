// Allocation churn: short-lived objects and arrays with a small live
// window, which is what a collector sees from most page script.
function run(n) {
  var window = [], sum = 0, i;
  for (i = 0; i < 256; i++) { window.push(null); }
  for (i = 0; i < n; i++) {
    var o = { id: i, tag: "t" + (i & 31), pair: [i, i + 1], next: null };
    o.next = window[(i * 7) & 255];
    // Keep the chain one link long, or everything ever made stays live.
    if (o.next) { o.next.next = null; }
    window[i & 255] = o;
    sum += o.pair[1] - o.id;
    if (o.next) { sum += o.next.id & 1; }
  }
  return sum;
}
