// A JSON-like tree: build it, serialize it, parse it back and walk it.
function build(depth, state) {
  var node = { id: state.next++, name: "node" + state.next, kids: [] };
  if (depth > 0) {
    for (var i = 0; i < 3 && state.next < state.limit; i++) {
      node.kids.push(build(depth - 1, state));
    }
  }
  return node;
}
function walk(node) {
  var total = node.id + node.name.length;
  for (var i = 0; i < node.kids.length; i++) { total += walk(node.kids[i]); }
  return total;
}
function run(n) {
  var tree = build(12, { next: 0, limit: n });
  var text = JSON.stringify(tree);
  var copy = JSON.parse(text);
  return walk(copy) + text.length;
}
