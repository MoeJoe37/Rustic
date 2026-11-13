// A DOM-free "framework diff" loop: render a virtual tree from state,
// diff it against the previous tree and count the patches.
function h(tag, props, children) {
  return { tag: tag, props: props, children: children };
}
function render(state) {
  var rows = [];
  for (var i = 0; i < state.rows.length; i++) {
    var row = state.rows[i];
    rows.push(h("li", { key: row.id, cls: row.done ? "done" : "open" },
      [h("span", { text: row.label }, []), h("b", { text: "" + row.count }, [])]));
  }
  return h("ul", { cls: "list" }, rows);
}
function diffProps(a, b, patches) {
  var k;
  for (k in a) { if (a[k] !== b[k]) { patches.count++; } }
  for (k in b) { if (!(k in a)) { patches.count++; } }
}
function diff(a, b, patches) {
  if (a.tag !== b.tag) { patches.count++; return; }
  diffProps(a.props, b.props, patches);
  var n = Math.min(a.children.length, b.children.length);
  for (var i = 0; i < n; i++) { diff(a.children[i], b.children[i], patches); }
  patches.count += Math.abs(a.children.length - b.children.length);
}
function run(n) {
  var state = { rows: [] }, i;
  for (i = 0; i < 100; i++) {
    state.rows.push({ id: i, label: "row " + i, count: 0, done: false });
  }
  var patches = { count: 0 }, previous = render(state);
  for (i = 0; i < n / 100; i++) {
    var row = state.rows[(i * 31) % 100];
    row.count++;
    row.done = !row.done;
    var next = render(state);
    diff(previous, next, patches);
    previous = next;
  }
  return patches.count;
}
