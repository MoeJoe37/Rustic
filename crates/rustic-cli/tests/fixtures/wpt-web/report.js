// Disable the upstream visual reporter and use the headless host's limits.
setup({ output: false, explicit_timeout: true });

add_completion_callback((tests, harness) => {
  const payload = JSON.stringify({
    status: harness.status,
    message: harness.message,
    tests: tests.map(test => ({
      name: test.name,
      status: test.status,
      message: test.message,
    })),
  });

  // Encode UTF-16 code units so DOM dump escaping cannot alter a test name.
  let encoded = '';
  for (let i = 0; i < payload.length; i++) {
    encoded += payload.charCodeAt(i).toString(16).padStart(4, '0');
  }
  document.documentElement.setAttribute('data-rustic-wpt-output', encoded);
});
