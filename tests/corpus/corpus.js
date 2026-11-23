// Shared by the M8 desktop corpus pages (ADR 0074). Each page runs its
// own checks and calls `corpus.pass()` once they hold; the fixed square in
// the bottom-left corner then turns from red to green, which the window
// test reads from the presented frame. A failed check logs why.
var corpus = {
  failures: 0,
  check: function (ok, what) {
    if (!ok) {
      corpus.failures++;
      console.log('corpus check failed: ' + what);
    }
    return ok;
  },
  pass: function () {
    if (corpus.failures === 0) {
      document.getElementById('status').setAttribute('class', 'pass');
    }
  },
  box: function (id) {
    return document.getElementById(id).getBoundingClientRect();
  }
};
