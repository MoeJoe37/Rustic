// String building: appending in a loop, joining pieces, and reading the
// result back by code unit.
function run(n) {
  var s = "", i;
  for (i = 0; i < n; i++) { s += "item" + (i % 10) + ";"; }
  var parts = [];
  for (i = 0; i < n; i++) { parts.push(String.fromCharCode(97 + (i % 26))); }
  var joined = parts.join("");
  var sum = s.length + joined.length;
  for (i = 0; i < joined.length; i += 7) { sum += joined.charCodeAt(i); }
  for (i = 0; i < s.length; i += 13) { sum += s.charCodeAt(i); }
  return sum;
}
