//! `rustic automate` (ADR 0072): inspection, console, evaluation, input,
//! the debugger, profiling and network diagnostics through the automation
//! protocol, and the acceptance checks for its authority and privacy
//! model: grants enforced by the broker, no remote endpoint, redacted
//! credentials, bounded sessions and a renderer that cannot forge output.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use rustic_devtools::Value;
use rustic_net::fixture::{FixtureServer, Reply};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "rustic-devtools-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Session {
    lines: Vec<Value>,
    stderr: String,
    success: bool,
}

impl Session {
    /// The response to command `id`.
    fn response(&self, id: u64) -> &Value {
        self.lines
            .iter()
            .find(|line| line.get("id").and_then(Value::as_u64) == Some(id))
            .unwrap_or_else(|| panic!("no response to {id}: {:?}\n{}", self.lines, self.stderr))
    }

    fn result(&self, id: u64) -> &Value {
        self.response(id)
            .get("result")
            .unwrap_or_else(|| panic!("command {id} failed: {:?}", self.response(id)))
    }

    fn error_code(&self, id: u64) -> &str {
        self.response(id)
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("command {id} did not fail: {:?}", self.response(id)))
    }

    fn events(&self, name: &str) -> Vec<&Value> {
        self.lines
            .iter()
            .filter(|line| line.get("event").and_then(Value::as_str) == Some(name))
            .filter_map(|line| line.get("params"))
            .collect()
    }
}

/// Run `rustic automate ARGS` with `commands` on standard input (then
/// closed) and parse every output line.
fn automate(args: &[&str], commands: &[&str]) -> Session {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .arg("automate")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rustic");
    let mut stdin = child.stdin.take().unwrap();
    for command in commands {
        writeln!(stdin, "{command}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(output.stdout).expect("protocol output is UTF-8");
    let lines = stdout
        .lines()
        .map(|line| Value::parse(line).unwrap_or_else(|e| panic!("{e}: {line}")))
        .collect();
    Session {
        lines,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        success: output.status.success(),
    }
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

const PAGE: &str = "<!doctype html>
<title> Inspect  me </title>
<style>#box { width: 100px; height: 50px; padding: 5px; color: rgb(255, 0, 0); --accent: blue }</style>
<div id=box class=\"a b\">Hello</div>
<div id=target style=\"width: 80px; height: 30px\">Go</div>
<script>
function add(a, b) {
  let sum = a + b;
  return sum;
}
var total = 0;
for (let i = 0; i < 2; i++) {
  total = add(total, i + 1);
}
console.log('total', total);
document.getElementById('target').addEventListener('click', () => {
  console.info('clicked', navigator.userActivation ? 'active' : '');
  document.title = 'Clicked';
});
</script>";

/// Both process models: split (with the broker's gate) and single.
fn modes() -> [&'static [&'static str]; 2] {
    [&["--no-sandbox"], &["--single-process"]]
}

#[test]
fn inspection_reads_dom_style_layout_and_screenshots() {
    let dir = TempDir::new();
    let page = dir.write("page.html", PAGE);
    for mode in modes() {
        let mut args = mode.to_vec();
        args.push(arg(&page));
        let session = automate(
            &args,
            &[
                r#"{"id":1,"method":"page.info"}"#,
                r##"{"id":2,"method":"dom.query","params":{"selector":"#box"}}"##,
                r#"{"id":3,"method":"dom.attributes","params":{"node":1}}"#,
                r#"{"id":4,"method":"dom.text","params":{"node":1}}"#,
                r#"{"id":5,"method":"css.computed","params":{"node":1,"properties":["width","color","--accent"]}}"#,
                r#"{"id":6,"method":"layout.box","params":{"node":1}}"#,
                r#"{"id":7,"method":"layout.hitTest","params":{"x":20,"y":20}}"#,
                r#"{"id":8,"method":"dom.document","params":{"maxNodes":5}}"#,
                r#"{"id":9,"method":"page.screenshot"}"#,
                r#"{"id":10,"method":"session.close"}"#,
            ],
        );
        assert!(session.success, "{mode:?}: {}", session.stderr);
        let info = session.result(1);
        assert_eq!(
            info.get("title").and_then(Value::as_str),
            Some("Inspect me")
        );
        assert!(
            info.get("url")
                .and_then(Value::as_str)
                .unwrap()
                .ends_with("/page.html")
        );
        assert_eq!(session.result(2).to_json(), r#"{"nodes":[1]}"#);
        assert_eq!(
            session.result(3).to_json(),
            r#"{"attributes":[["id","box"],["class","a b"]]}"#
        );
        assert_eq!(
            session.result(4).get("text").and_then(Value::as_str),
            Some("Hello")
        );
        assert_eq!(
            session.result(5).to_json(),
            r#"{"properties":{"color":"rgb(255, 0, 0)","width":"100px"},"custom":{"--accent":"blue"}}"#
        );
        assert_eq!(
            session.result(6).to_json(),
            r#"{"rect":{"x":8,"y":8,"width":110,"height":60},"content":{"width":100,"height":50},"border":{"width":110,"height":60},"padding":{"left":5,"top":5}}"#
        );
        assert_eq!(session.result(7).to_json(), r#"{"node":1}"#);
        let document = session.result(8);
        assert_eq!(
            document
                .get("nodes")
                .and_then(Value::as_array)
                .unwrap()
                .len(),
            5
        );
        assert_eq!(document.get("truncated"), Some(&Value::Bool(true)));
        let shot = session.result(9);
        assert_eq!(shot.get("width").and_then(Value::as_u64), Some(1280));
        assert_eq!(shot.get("height").and_then(Value::as_u64), Some(720));
        assert!(
            shot.get("png")
                .and_then(Value::as_str)
                .unwrap()
                .starts_with("iVBORw0KGgo"),
            "a PNG"
        );
        assert_eq!(session.result(10), &Value::Null);
    }
}

#[test]
fn console_evaluate_and_input_need_their_grants() {
    let dir = TempDir::new();
    let page = dir.write("page.html", PAGE);
    let commands = [
        r#"{"id":1,"method":"console.messages"}"#,
        r#"{"id":2,"method":"runtime.evaluate","params":{"expression":"total * 10"}}"#,
        r#"{"id":3,"method":"input.click","params":{"x":20,"y":90}}"#,
        r#"{"id":4,"method":"console.messages"}"#,
        r#"{"id":5,"method":"runtime.evaluate","params":{"expression":"document.title"}}"#,
        r#"{"id":6,"method":"runtime.evaluate","params":{"expression":"null.x"}}"#,
        r#"{"id":7,"method":"runtime.evaluate","params":{"expression":"("}}"#,
    ];
    for mode in modes() {
        // The read-only default: console yes, evaluate and input no.
        let mut args = mode.to_vec();
        args.push(arg(&page));
        let session = automate(&args, &commands);
        assert!(session.success, "{}", session.stderr);
        assert_eq!(
            session.result(1).to_json(),
            r#"{"messages":[{"level":"log","text":"total 3","truncated":false}],"dropped":0}"#
        );
        for id in [2, 3, 5, 6, 7] {
            assert_eq!(session.error_code(id), "not-granted", "{mode:?} {id}");
        }
        assert_eq!(
            session.result(4).to_json(),
            r#"{"messages":[],"dropped":0}"#
        );

        let mut args = mode.to_vec();
        args.extend([
            "--devtools-allow",
            "console,evaluate,input,inspect",
            arg(&page),
        ]);
        let session = automate(&args, &commands);
        assert!(session.success, "{}", session.stderr);
        assert_eq!(session.result(2).to_json(), r#"{"value":"30"}"#);
        assert_eq!(session.result(3).get("delivered"), Some(&Value::Bool(true)));
        assert_eq!(
            session.result(4).get("messages").unwrap().to_json(),
            r#"[{"level":"info","text":"clicked active","truncated":false}]"#
        );
        assert_eq!(session.result(5).to_json(), r#"{"value":"Clicked"}"#);
        let thrown = session
            .result(6)
            .get("exception")
            .and_then(Value::as_str)
            .unwrap();
        assert!(thrown.starts_with("TypeError"), "{thrown}");
        let syntax = session
            .result(7)
            .get("exception")
            .and_then(Value::as_str)
            .unwrap();
        assert!(syntax.starts_with("SyntaxError"), "{syntax}");
    }
}

#[test]
fn debugger_breakpoints_steps_scopes_and_document_lines() {
    let dir = TempDir::new();
    let page = dir.write("page.html", PAGE);
    for mode in modes() {
        let mut args = mode.to_vec();
        args.extend(["--devtools-allow", "debug,inspect", arg(&page)]);
        let session = automate(
            &args,
            &[
                // Line 8 of the document: `let sum = a + b;`.
                r#"{"id":1,"method":"debugger.setBreakpoint","params":{"url":"page.html","line":8}}"#,
                r#"{"id":2,"method":"page.load"}"#,
                r#"{"id":3,"method":"debugger.scopes","params":{"frame":0}}"#,
                r#"{"id":4,"method":"dom.document"}"#,
                r#"{"id":5,"method":"debugger.stepOver"}"#,
                r#"{"id":6,"method":"debugger.scopes","params":{"frame":0}}"#,
                r#"{"id":7,"method":"debugger.stepOut"}"#,
                r#"{"id":8,"method":"debugger.removeBreakpoint","params":{"breakpoint":1}}"#,
                r#"{"id":9,"method":"debugger.resume"}"#,
                r#"{"id":10,"method":"debugger.scripts"}"#,
                r#"{"id":11,"method":"debugger.resume"}"#,
            ],
        );
        assert!(session.success, "{mode:?}: {}", session.stderr);
        assert_eq!(
            session.result(1).to_json(),
            r#"{"breakpoint":1,"resolved":[]}"#
        );
        let parsed = session.events("debugger.scriptParsed");
        assert_eq!(parsed[0].get("lineOffset").and_then(Value::as_u64), Some(5));
        assert_eq!(parsed[0].get("breakpoints").unwrap().to_json(), "[1]");
        let paused = session.events("debugger.paused");
        let lines: Vec<(&str, u64, usize)> = paused
            .iter()
            .map(|event| {
                (
                    event.get("reason").and_then(Value::as_str).unwrap(),
                    event.get("line").and_then(Value::as_u64).unwrap(),
                    event.get("stack").and_then(Value::as_array).unwrap().len(),
                )
            })
            .collect();
        // The breakpoint, the step over to `return`, the step out to the
        // loop body's next run; the breakpoint was removed before resuming.
        assert_eq!(
            lines,
            vec![("breakpoint", 8, 2), ("step", 9, 2), ("step", 13, 1)],
            "{mode:?}"
        );
        let top = &paused[0].get("stack").and_then(Value::as_array).unwrap()[0];
        assert_eq!(top.get("function").and_then(Value::as_str), Some("add"));
        assert_eq!(
            session.result(3).to_json(),
            r#"[{"scope":"local","name":"a","value":"0"},{"scope":"local","name":"b","value":"1"},{"scope":"local","name":"arguments","value":"undefined"},{"scope":"local","name":"sum","value":"<uninitialized>"}]"#
        );
        // Inspection waits until the page is resumed.
        assert_eq!(session.error_code(4), "wrong-state");
        let sum = session
            .result(6)
            .as_array()
            .unwrap()
            .iter()
            .find(|binding| binding.get("name").and_then(Value::as_str) == Some("sum"))
            .unwrap();
        assert_eq!(sum.get("value").and_then(Value::as_str), Some("1"));
        assert_eq!(session.events("debugger.resumed").len(), 3);
        // The session continued after the load: the last resume has no
        // pause to end.
        assert_eq!(session.error_code(11), "wrong-state");
        let scripts = session.result(10).as_array().unwrap();
        assert_eq!(scripts[0].get("inline"), Some(&Value::Bool(true)));
    }
}

#[test]
fn debugger_statement_and_pause_on_start() {
    let dir = TempDir::new();
    let page = dir.write(
        "page.html",
        "<!doctype html><script src=app.js></script><script>\nvar seen = 1;\n</script>",
    );
    dir.write("app.js", "var x = 1;\ndebugger;\nx++;\n");
    let mut args = vec!["--no-sandbox", "--devtools-allow", "debug,evaluate"];
    args.push(arg(&page));
    let session = automate(
        &args,
        &[
            r#"{"id":1,"method":"debugger.pause"}"#,
            r#"{"id":2,"method":"page.load"}"#,
            r#"{"id":3,"method":"debugger.resume"}"#,
            r#"{"id":4,"method":"debugger.stack"}"#,
            r#"{"id":5,"method":"debugger.resume"}"#,
            r#"{"id":6,"method":"runtime.evaluate","params":{"expression":"x + seen"}}"#,
        ],
    );
    assert!(session.success, "{}", session.stderr);
    let paused = session.events("debugger.paused");
    let summary: Vec<(&str, u64, &str)> = paused
        .iter()
        .map(|event| {
            (
                event.get("reason").and_then(Value::as_str).unwrap(),
                event.get("line").and_then(Value::as_u64).unwrap(),
                event.get("url").and_then(Value::as_str).unwrap(),
            )
        })
        .collect();
    assert_eq!(summary.len(), 2, "{summary:?}");
    assert_eq!((summary[0].0, summary[0].1), ("step", 1));
    assert!(
        summary[0].2.ends_with("/app.js"),
        "external scripts keep their URL"
    );
    assert_eq!((summary[1].0, summary[1].1), ("debugger-statement", 2));
    let stack = session.result(4).as_array().unwrap();
    assert_eq!(stack[0].get("line").and_then(Value::as_u64), Some(2));
    assert_eq!(session.result(6).to_json(), r#"{"value":"3"}"#);
}

#[test]
fn profile_counts_statements_and_reports_metrics() {
    let dir = TempDir::new();
    let page = dir.write("page.html", PAGE);
    let session = automate(
        &["--single-process", arg(&page)],
        &[
            r#"{"id":1,"method":"profile.statements"}"#,
            r#"{"id":2,"method":"profile.metrics"}"#,
            r#"{"id":3,"method":"page.settle"}"#,
            r#"{"id":4,"method":"page.advance","params":{"ms":100}}"#,
            r#"{"id":5,"method":"page.advance","params":{"ms":100000}}"#,
        ],
    );
    assert!(session.success, "{}", session.stderr);
    let lines = session
        .result(1)
        .get("lines")
        .and_then(Value::as_array)
        .unwrap();
    let count = |line: u64| {
        lines
            .iter()
            .find(|entry| entry.get("line").and_then(Value::as_u64) == Some(line))
            .and_then(|entry| entry.get("count").and_then(Value::as_u64))
    };
    // `let sum` and `return sum` run once per call; the loop body twice.
    assert_eq!((count(8), count(9), count(13)), (Some(2), Some(2), Some(2)));
    let metrics = session.result(2);
    assert!(metrics.get("loadMs").and_then(Value::as_f64).unwrap() > 0.0);
    assert_eq!(metrics.get("scripts").and_then(Value::as_u64), Some(1));
    assert_eq!(session.result(3).to_json(), r#"{"turns":0}"#);
    assert_eq!(session.result(4).to_json(), r#"{"turns":0}"#);
    assert_eq!(session.error_code(5), "invalid-params");
}

#[test]
fn network_diagnostics_list_requests_without_credentials() {
    let server = FixtureServer::start([
        (
            "/",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/html"),
                    ("Set-Cookie", "session=s3cret; Path=/"),
                ],
                b"<!doctype html><link rel=stylesheet href=/style.css><script>\
                  fetch('/api', { headers: { 'Authorization': 'Bearer t0ken', 'X-Trace': 'on' } })\
                    .then(r => r.text()).then(t => console.log('api', t));\
                  </script>",
            ),
        ),
        (
            "/style.css",
            Reply::response(200, &[("Content-Type", "text/css")], b"p { color: red }"),
        ),
        (
            "/api",
            Reply::response(
                200,
                &[
                    ("Content-Type", "text/plain"),
                    ("Set-Cookie", "more=s3cret"),
                ],
                b"ok",
            ),
        ),
    ])
    .unwrap();
    let grant = format!("http://a.test={}", server.address());
    let commands = [
        r#"{"id":1,"method":"network.requests"}"#,
        r#"{"id":2,"method":"console.messages"}"#,
    ];
    for mode in modes() {
        let mut args = mode.to_vec();
        args.extend(["--allow-origin", &grant, "http://a.test/"]);
        let session = automate(&args, &commands);
        assert!(session.success, "{}", session.stderr);
        assert_eq!(session.error_code(1), "not-granted", "network is opt-in");

        let mut args = mode.to_vec();
        args.extend([
            "--devtools-allow",
            "network,console",
            "--allow-origin",
            &grant,
            "http://a.test/",
        ]);
        let session = automate(&args, &commands);
        assert!(session.success, "{}", session.stderr);
        let output: String = session.lines.iter().map(Value::to_json).collect();
        assert!(!output.contains("s3cret"), "cookies never appear: {output}");
        assert!(
            !output.contains("t0ken"),
            "credentials are redacted: {output}"
        );
        let requests = session
            .result(1)
            .get("requests")
            .and_then(Value::as_array)
            .unwrap();
        let mut kinds: Vec<&str> = requests
            .iter()
            .map(|r| r.get("kind").and_then(Value::as_str).unwrap())
            .collect();
        assert_eq!(kinds[0], "document");
        kinds.sort_unstable();
        assert_eq!(kinds, ["document", "fetch", "stylesheet"], "{output}");
        let fetch = requests
            .iter()
            .find(|r| r.get("kind").and_then(Value::as_str) == Some("fetch"))
            .unwrap();
        assert_eq!(fetch.get("status").and_then(Value::as_u64), Some(200));
        assert_eq!(
            fetch.get("requestHeaders").unwrap().to_json(),
            r#"[["Authorization","<redacted>"],["X-Trace","on"]]"#
        );
        assert!(
            session.result(2).to_json().contains(r#""text":"api ok""#),
            "{output}"
        );
        // The server did receive the page's own credential: redaction is
        // about the diagnostics, not the page's behavior.
        assert!(
            server
                .requests()
                .iter()
                .any(|r| r.target == "/api" && r.header("authorization") == Some("Bearer t0ken"))
        );
    }
}

#[test]
fn the_broker_refuses_ungranted_commands_before_the_renderer_sees_them() {
    let dir = TempDir::new();
    // The page counts every script it runs; a refused evaluation must
    // never reach the renderer, so the count stays at one.
    let page = dir.write(
        "page.html",
        "<!doctype html><script>var runs = (window.runs || 0) + 1;</script>",
    );
    let session = automate(
        &["--no-sandbox", "--devtools-allow", "inspect", arg(&page)],
        &[
            r#"{"id":1,"method":"runtime.evaluate","params":{"expression":"runs = 99"}}"#,
            r#"{"id":2,"method":"debugger.pause"}"#,
            r#"not json"#,
            r#"{"id":3,"method":"browser.exec"}"#,
            r#"{"id":4,"method":"session.capabilities"}"#,
        ],
    );
    assert!(session.success, "{}", session.stderr);
    assert_eq!(session.error_code(1), "not-granted");
    assert_eq!(session.error_code(2), "not-granted");
    assert_eq!(session.error_code(3), "unknown-method");
    assert!(session.lines.iter().any(|line| {
        line.get("error")
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            == Some("parse-error")
    }));
    let capabilities = session.result(4);
    assert_eq!(
        capabilities.get("grants").unwrap().to_json(),
        r#"["inspect"]"#
    );
    assert_eq!(capabilities.get("remote"), Some(&Value::Bool(false)));
    assert!(
        !capabilities
            .get("methods")
            .unwrap()
            .to_json()
            .contains("runtime")
    );
}

#[test]
fn there_is_no_remote_endpoint_and_bad_grants_fail() {
    let dir = TempDir::new();
    let page = dir.write("page.html", "<!doctype html><p>x");
    for option in ["--remote-debugging-port", "--devtools-port", "--listen"] {
        let session = automate(&[option, "9222", arg(&page)], &[]);
        assert!(!session.success);
        assert!(
            session.stderr.contains("unknown option"),
            "{}",
            session.stderr
        );
    }
    for list in ["root", "inspect,inspect", ""] {
        let session = automate(&["--devtools-allow", list, arg(&page)], &[]);
        assert!(!session.success, "{list:?}");
        assert!(
            session.stderr.contains("DevTools grant"),
            "{}",
            session.stderr
        );
    }
}

#[test]
fn a_renderer_cannot_pull_commands_or_forge_client_output() {
    let dir = TempDir::new();
    let page = dir.write("page.html", "<!doctype html><p>x");
    // Outside a session the broker refuses the request outright.
    let output = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(["dump-dom", "--no-sandbox", arg(&page)])
        .env("RUSTIC_TEST_COMPROMISED_RENDERER", "automation")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("broke the IPC protocol"));
    // In a session it may pull a command, but a forged multi-line answer
    // stops it and nothing it wrote reaches the client.
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(["automate", "--no-sandbox", arg(&page)])
        .env("RUSTIC_TEST_COMPROMISED_RENDERER", "automation")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(
        child.stdin.take().unwrap(),
        r#"{{"id":1,"method":"page.info"}}"#
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("broke the IPC protocol"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("forged"));
}

#[test]
fn a_paused_session_is_bounded_by_the_renderer_timeout() {
    let dir = TempDir::new();
    let page = dir.write("page.html", "<!doctype html><script>\ndebugger;\n</script>");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args([
            "automate",
            "--no-sandbox",
            "--renderer-timeout",
            "2",
            "--devtools-allow",
            "debug",
            arg(&page),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Start the load, then keep the input open and never resume.
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"id":1,"method":"page.load"}}"#).unwrap();
    let started = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the session never ended"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("debugger.paused"), "{stdout}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("did not finish within 2 s"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
