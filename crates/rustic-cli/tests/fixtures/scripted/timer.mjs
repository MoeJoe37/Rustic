await new Promise(resolve => setTimeout(resolve, 10));
if (order === 'first:inline:async') {
  requestAnimationFrame(() => document.getElementById('result').setAttribute('class', 'pass'));
}
