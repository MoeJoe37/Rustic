//! A renderer started by a host rather than by the broker (ADR 0059: the
//! Android isolated service, which receives one end of a socket): the
//! broker drives it over that stream exactly like a child process, and a
//! renderer that disappears ends the command cleanly. Here the "service"
//! is a thread with one end of a Unix socket pair; on Android it is
//! `RendererService` in an isolated process (`android/`).
#![cfg(unix)]

use std::ffi::OsString;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use rustic_cli::{HostedRenderer, run_hosted};

fn hosted(stream: UnixStream) -> HostedRenderer {
    let stop = stream.try_clone().unwrap();
    HostedRenderer {
        reader: Box::new(stream.try_clone().unwrap()),
        writer: Box::new(stream),
        stop: Arc::new(move || {
            let _ = stop.shutdown(Shutdown::Both);
        }),
    }
}

#[test]
fn a_hosted_renderer_serves_a_command_and_a_vanished_one_ends_it() {
    let page = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linked/page.html");
    let args = [OsString::from(&page)];

    // The service: a renderer over its end of the socket.
    let (broker_end, service_end) = UnixStream::pair().unwrap();
    let service = std::thread::spawn(move || {
        let reader = service_end.try_clone().unwrap();
        rustic_cli::run_renderer_over(Box::new(reader), Box::new(service_end))
    });
    let (status, output) = run_hosted("dump-style", &args, hosted(broker_end));
    assert_eq!(status, 0);
    assert_eq!(service.join().unwrap(), 0);
    let single = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .args(["--single-process", "dump-style"])
        .arg(&page)
        .output()
        .unwrap();
    assert!(single.status.success());
    assert_eq!(output, single.stdout);

    // A service that dies before answering (Android killed or crashed
    // it): the command fails, nothing is written, nothing hangs.
    let (broker_end, service_end) = UnixStream::pair().unwrap();
    drop(service_end);
    let (status, output) = run_hosted("dump-style", &args, hosted(broker_end));
    assert_eq!(status, 1);
    assert!(output.is_empty());
}
