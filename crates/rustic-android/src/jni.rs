//! The JNI entry points. Java hands over file descriptors it detached
//! (`ParcelFileDescriptor.detachFd()`), so each is owned here exactly once.

#![allow(
    unsafe_code,
    reason = "ADR 0059: JNI needs unmangled exported symbols and adopting file descriptors Java detached"
)]

use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::net::Shutdown;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

/// Adopt a descriptor Java detached, or `None` for an invalid one.
fn adopt(fd: i32) -> Option<OwnedFd> {
    if fd < 0 {
        return None;
    }
    // SAFETY: Java called `detachFd()` and passes the descriptor exactly
    // once, so nothing else owns or closes it.
    Some(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `RendererService.nativeRun(int fd, boolean crash)`: serve the broker as
/// a renderer over the socket `fd` until it is done. With `crash`, end the
/// process at once instead (the broker must recover).
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_rustic_android_RendererService_nativeRun(
    _env: *mut c_void,
    _class: *mut c_void,
    fd: i32,
    crash: u8,
) -> i32 {
    let Some(fd) = adopt(fd) else {
        return 2;
    };
    if crash != 0 {
        std::process::abort();
    }
    let stream = UnixStream::from(fd);
    let Ok(reader) = stream.try_clone() else {
        return 1;
    };
    i32::from(rustic_cli::run_renderer_over(
        Box::new(reader),
        Box::new(stream),
    ))
}

/// `RendererService.nativeProbe(int port)` (and `ProbeService`'s): the
/// [`crate::probe`] bits.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_rustic_android_RendererService_nativeProbe(
    _env: *mut c_void,
    _class: *mut c_void,
    port: i32,
) -> i32 {
    crate::probe(u16::try_from(port).unwrap_or(0))
}

/// `Broker.nativeDumpDom(int rendererFd, int outFd)`: run `dump-dom` on
/// `files/page.html` against the renderer at the other end of
/// `rendererFd`, write the output to `outFd` and return the exit status.
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_rustic_android_Broker_nativeDumpDom(
    _env: *mut c_void,
    _class: *mut c_void,
    renderer_fd: i32,
    out_fd: i32,
) -> i32 {
    let (Some(renderer), Some(out)) = (adopt(renderer_fd), adopt(out_fd)) else {
        return 2;
    };
    let stream = UnixStream::from(renderer);
    let (Ok(reader), Ok(stopper)) = (stream.try_clone(), stream.try_clone()) else {
        return 1;
    };
    let hosted = rustic_cli::HostedRenderer {
        reader: Box::new(reader),
        writer: Box::new(stream),
        stop: Arc::new(move || {
            let _ = stopper.shutdown(Shutdown::Both);
        }),
    };
    let page = format!("{}/page.html", crate::files_dir());
    let (status, output) = rustic_cli::run_hosted("dump-dom", &[page.into()], hosted);
    let mut out = File::from(out);
    if out.write_all(&output).and_then(|()| out.flush()).is_err() {
        return 1;
    }
    i32::from(status)
}
