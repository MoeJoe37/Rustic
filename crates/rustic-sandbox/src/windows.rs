//! The Windows renderer launcher (ISO-04, ADR 0060).
//!
//! A renderer starts in the `Rustic.Renderer` AppContainer with no
//! capabilities: it cannot open the user's files, the registry outside its
//! container, the network or loopback, and cannot see other processes'
//! windows. It is created suspended, put in a job that kills it when the
//! broker's handle closes and admits no second process, and only then
//! resumed. Its standard streams are the only handles it inherits.
//!
//! Every failure is an error; nothing here falls back to an unconfined
//! process.

#![allow(
    unsafe_code,
    reason = "ADR 0060: the Win32 process, AppContainer and job APIs have no safe std equivalent"
)]

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::ptr::{null, null_mut};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    HANDLE, HANDLE_FLAG_INHERIT, LocalFree, SetHandleInformation, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::Isolation::{
    CreateAppContainerProfile, DeriveAppContainerSidFromAppContainerName,
};
use windows_sys::Win32::Security::{FreeSid, PSID, SECURITY_ATTRIBUTES, SECURITY_CAPABILITIES};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DETACHED_PROCESS,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, INFINITE,
    InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
    PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
    WaitForSingleObject,
};

/// The AppContainer every renderer shares. Renderers hold no authority
/// and keep nothing on disk, so one container profile is enough; the
/// broker separates documents by process (ADR 0057).
const CONTAINER: &str = "Rustic.Renderer";
/// `HRESULT_FROM_WIN32(ERROR_ALREADY_EXISTS)`.
const ALREADY_EXISTS: i32 = 0x8007_00B7_u32 as i32;
/// `E_UNEXPECTED`, which a racing profile creation can return.
const UNEXPECTED: i32 = 0x8000_FFFF_u32 as i32;
/// Retries of a profile creation that lost a race (50 ms to 800 ms).
const PROFILE_RETRIES: u32 = 5;
/// `PROCESS_CREATION_CHILD_PROCESS_RESTRICTED`: the renderer cannot create
/// processes even outside its job.
const CHILD_PROCESS_RESTRICTED: u32 = 1;
/// `PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT`: a less privileged
/// AppContainer (LPAC). Objects that admit every AppContainer through
/// `ALL APPLICATION PACKAGES` do not admit the renderer.
const ALL_APPLICATION_PACKAGES_OPT_OUT: u32 = 1;

/// The broker's ends of a confined renderer's standard streams.
#[derive(Debug)]
pub struct Pipes {
    /// The renderer's standard input (the broker writes).
    pub stdin: File,
    /// The renderer's standard output (the broker reads).
    pub stdout: File,
    /// The renderer's standard error (the broker reads).
    pub stderr: File,
}

/// A renderer running in the AppContainer. Dropping it closes the job,
/// which kills the renderer if it is still running.
#[derive(Debug)]
pub struct Confined {
    process: OwnedHandle,
    _job: OwnedHandle,
    id: u32,
}

impl Confined {
    /// The renderer's process id.
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Wait for the renderer to exit.
    pub fn wait(&self) -> io::Result<ExitStatus> {
        let handle = self.process.as_raw_handle() as HANDLE;
        // SAFETY: `handle` is the live process handle owned by `self`.
        if unsafe { WaitForSingleObject(handle, INFINITE) } != WAIT_OBJECT_0 {
            return Err(os_error("WaitForSingleObject"));
        }
        let mut code = 0u32;
        // SAFETY: as above; `code` is a valid out pointer.
        if unsafe { GetExitCodeProcess(handle, &mut code) } == 0 {
            return Err(os_error("GetExitCodeProcess"));
        }
        Ok(ExitStatus::from_raw(code))
    }

    /// Terminate the renderer.
    pub fn kill(&self) -> io::Result<()> {
        // SAFETY: the process handle is live and owned by `self`.
        if unsafe { TerminateProcess(self.process.as_raw_handle() as HANDLE, 1) } == 0 {
            return Err(os_error("TerminateProcess"));
        }
        Ok(())
    }
}

/// Start `program` with `args` in the renderer AppContainer, with exactly
/// the environment `env` (plus `SystemRoot` and `LOCALAPPDATA`, which
/// Windows needs) and the system directory as its working directory.
///
/// `program` gains an access-control entry that lets the container read
/// and execute it (and nothing else); the call fails if the broker may not
/// change that file's permissions.
pub fn spawn_app_container(
    program: &Path,
    args: &[&OsStr],
    env: &[(OsString, OsString)],
) -> io::Result<(Confined, Pipes)> {
    let sid = ContainerSid::open()?;
    grant_executable(program, &sid.to_string_sid()?)?;
    let system = system_directory()?;

    let (child_stdin, stdin) = pipe(Side::ChildReads)?;
    let (stdout, child_stdout) = pipe(Side::ChildWrites)?;
    let (stderr, child_stderr) = pipe(Side::ChildWrites)?;

    let mut capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid.0,
        Capabilities: null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let inherited: [HANDLE; 3] = [
        child_stdin.as_raw_handle() as HANDLE,
        child_stdout.as_raw_handle() as HANDLE,
        child_stderr.as_raw_handle() as HANDLE,
    ];
    let child_policy = CHILD_PROCESS_RESTRICTED;
    let packages_policy = ALL_APPLICATION_PACKAGES_OPT_OUT;
    let mut attributes = AttributeList::new(4)?;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        (&raw mut capabilities).cast(),
        size_of::<SECURITY_CAPABILITIES>(),
    )?;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        inherited.as_ptr().cast_mut().cast(),
        size_of_val(&inherited),
    )?;
    attributes.set(
        PROC_THREAD_ATTRIBUTE_CHILD_PROCESS_POLICY,
        (&raw const child_policy).cast_mut().cast(),
        size_of::<u32>(),
    )?;

    attributes.set(
        PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
        (&raw const packages_policy).cast_mut().cast(),
        size_of::<u32>(),
    )?;

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = inherited[0];
    startup.StartupInfo.hStdOutput = inherited[1];
    startup.StartupInfo.hStdError = inherited[2];
    startup.lpAttributeList = attributes.as_ptr();

    let application = wide(program.as_os_str());
    let mut command_line = command_line(program.as_os_str(), args)?;
    // CreateProcessW resolves the container's folder under the caller's
    // `LOCALAPPDATA` and fails with ERROR_ENVVAR_NOT_FOUND without it. The
    // path is not a secret the container could use: it cannot open it.
    let mut env = env.to_vec();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        env.retain(|(name, _)| !name.eq_ignore_ascii_case("LOCALAPPDATA"));
        env.push(("LOCALAPPDATA".into(), local));
    }
    let environment = environment_block(&env, &system)?;
    let directory = wide(system.as_os_str());
    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: every pointer refers to a live, NUL-terminated buffer or
    // initialized structure owned by this frame: the attribute list and
    // the values it points to (`capabilities`, `inherited`,
    // `child_policy`) outlive the call, `command_line` is mutable as the
    // API requires, and `environment` is a double-NUL-terminated UTF-16
    // block as `CREATE_UNICODE_ENVIRONMENT` declares.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_SUSPENDED
                | DETACHED_PROCESS,
            environment.as_ptr().cast(),
            directory.as_ptr(),
            &startup.StartupInfo,
            &mut information,
        )
    };
    if created == 0 {
        return Err(os_error("CreateProcessW"));
    }
    // SAFETY: CreateProcessW succeeded, so both handles are new, open and
    // owned by nobody else.
    let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess as RawHandle) };
    // SAFETY: as above.
    let thread = unsafe { OwnedHandle::from_raw_handle(information.hThread as RawHandle) };
    drop((child_stdin, child_stdout, child_stderr));

    let job = match confine(&process) {
        Ok(job) => job,
        Err(error) => {
            // Never let a suspended, unconfined renderer run.
            // SAFETY: `process` is live and owned here.
            unsafe { TerminateProcess(process.as_raw_handle() as HANDLE, 1) };
            return Err(error);
        }
    };
    // SAFETY: `thread` is the live primary thread handle of the suspended
    // process.
    if unsafe { ResumeThread(thread.as_raw_handle() as HANDLE) } == u32::MAX {
        let error = os_error("ResumeThread");
        // SAFETY: `process` is live and owned here.
        unsafe { TerminateProcess(process.as_raw_handle() as HANDLE, 1) };
        return Err(error);
    }
    let confined = Confined {
        process,
        _job: job,
        id: information.dwProcessId,
    };
    let pipes = Pipes {
        stdin: File::from(stdin),
        stdout: File::from(stdout),
        stderr: File::from(stderr),
    };
    Ok((confined, pipes))
}

/// Put `process` in a new job that kills it when the job handle closes
/// (the broker died or dropped it), admits no second process and ends it
/// on an unhandled exception instead of showing an error dialog.
fn confine(process: &OwnedHandle) -> io::Result<OwnedHandle> {
    // SAFETY: null attributes and name are allowed.
    let raw = unsafe { CreateJobObjectW(null(), null()) };
    if raw.is_null() {
        return Err(os_error("CreateJobObjectW"));
    }
    // SAFETY: CreateJobObjectW returned a new handle that nothing else owns.
    let job = unsafe { OwnedHandle::from_raw_handle(raw as RawHandle) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
        | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    // SAFETY: `limits` is an initialized structure of the declared class
    // and size.
    let set = unsafe {
        SetInformationJobObject(
            job.as_raw_handle() as HANDLE,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if set == 0 {
        return Err(os_error("SetInformationJobObject"));
    }
    // SAFETY: both handles are live and owned by the caller or this frame.
    let assigned = unsafe {
        AssignProcessToJobObject(
            job.as_raw_handle() as HANDLE,
            process.as_raw_handle() as HANDLE,
        )
    };
    if assigned == 0 {
        return Err(os_error("AssignProcessToJobObject"));
    }
    Ok(job)
}

/// The container's SID, freed with `FreeSid`.
struct ContainerSid(PSID);

impl ContainerSid {
    /// Create the container profile, or find the one an earlier run made.
    ///
    /// Two brokers starting their first renderers at once race to create
    /// the profile, and the loser can get `E_UNEXPECTED` ("Catastrophic
    /// failure") rather than `ERROR_ALREADY_EXISTS` (seen on native Windows
    /// ARM64 CI with parallel tests). Such a failure is retried a few times
    /// with a growing pause; by then the other broker's profile exists.
    fn open() -> io::Result<Self> {
        let name = wide(OsStr::new(CONTAINER));
        let description = wide(OsStr::new("Rustic renderer processes"));
        let mut attempt = 0u32;
        loop {
            let mut sid: PSID = null_mut();
            // SAFETY: the strings are NUL-terminated; no capabilities are
            // passed; `sid` is a valid out pointer.
            let mut result = unsafe {
                CreateAppContainerProfile(
                    name.as_ptr(),
                    name.as_ptr(),
                    description.as_ptr(),
                    null(),
                    0,
                    &mut sid,
                )
            };
            if result == ALREADY_EXISTS {
                // SAFETY: as above.
                result =
                    unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
            }
            if result >= 0 && !sid.is_null() {
                return Ok(Self(sid));
            }
            if result != UNEXPECTED || attempt >= PROFILE_RETRIES {
                return Err(with_step(
                    "CreateAppContainerProfile",
                    hresult_error(result),
                ));
            }
            attempt += 1;
            std::thread::sleep(std::time::Duration::from_millis(25 << attempt));
        }
    }

    fn to_string_sid(&self) -> io::Result<String> {
        let mut text: *mut u16 = null_mut();
        // SAFETY: `self.0` is a valid SID; `text` is a valid out pointer.
        if unsafe { ConvertSidToStringSidW(self.0, &mut text) } == 0 {
            return Err(os_error("ConvertSidToStringSidW"));
        }
        let mut length = 0;
        loop {
            // SAFETY: on success `text` is a NUL-terminated string allocated
            // by the system, valid until `LocalFree`; `length` never passes
            // its terminator.
            let unit = unsafe { text.add(length) };
            // SAFETY: as above, `unit` is inside that string.
            if unsafe { *unit } == 0 {
                break;
            }
            length += 1;
        }
        // SAFETY: `length` code units were just read from `text`.
        let units = unsafe { std::slice::from_raw_parts(text, length) };
        let string = String::from_utf16_lossy(units);
        // SAFETY: `text` came from ConvertSidToStringSidW and is freed once.
        unsafe { LocalFree(text.cast()) };
        Ok(string)
    }
}

impl Drop for ContainerSid {
    fn drop(&mut self) {
        // SAFETY: the SID came from the AppContainer APIs, whose results
        // are freed with FreeSid, and is freed once.
        unsafe { FreeSid(self.0) };
    }
}

/// A process/thread attribute list, deleted on drop.
struct AttributeList {
    buffer: Vec<usize>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0usize;
        // SAFETY: a null list with a size pointer asks for the needed size;
        // this call is documented to fail with ERROR_INSUFFICIENT_BUFFER.
        unsafe { InitializeProcThreadAttributeList(null_mut(), count, 0, &mut size) };
        if size == 0 {
            return Err(os_error("InitializeProcThreadAttributeList"));
        }
        // `usize` elements keep the list pointer-aligned.
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: `buffer` holds at least `size` writable, aligned bytes.
        if unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), count, 0, &mut size)
        } == 0
        {
            return Err(os_error("InitializeProcThreadAttributeList"));
        }
        Ok(Self { buffer })
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.buffer.as_mut_ptr().cast()
    }

    /// Add an attribute. `value` must stay valid until the process is
    /// created; the callers keep it in the same frame.
    fn set(&mut self, attribute: u32, value: *mut c_void, size: usize) -> io::Result<()> {
        // SAFETY: the list was initialized for enough attributes, and
        // `value` points to `size` readable bytes of the type the
        // attribute expects (see the callers).
        let updated = unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                value,
                size,
                null_mut(),
                null(),
            )
        };
        if updated == 0 {
            return Err(os_error("UpdateProcThreadAttribute"));
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: the list was initialized in `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

/// Which end of a pipe the renderer gets.
enum Side {
    ChildReads,
    ChildWrites,
}

/// An anonymous pipe as `(read, write)`. Only the renderer's end is
/// inheritable, and it is passed through the explicit handle list.
fn pipe(side: Side) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
    // SAFETY: valid out pointers and attributes; default buffer size.
    if unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } == 0 {
        return Err(os_error("CreatePipe"));
    }
    // SAFETY: CreatePipe succeeded; both handles are new and unowned.
    let read = unsafe { OwnedHandle::from_raw_handle(read as RawHandle) };
    // SAFETY: as above.
    let write = unsafe { OwnedHandle::from_raw_handle(write as RawHandle) };
    let broker_end = match side {
        Side::ChildReads => &write,
        Side::ChildWrites => &read,
    };
    // SAFETY: the handle is live and owned by this frame.
    if unsafe { SetHandleInformation(broker_end.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, 0) }
        == 0
    {
        return Err(os_error("SetHandleInformation"));
    }
    Ok((read, write))
}

/// `%SystemRoot%\System32`, from the system rather than the environment.
fn system_directory() -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 260];
    loop {
        let capacity = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
        // SAFETY: `buffer` has `capacity` writable code units.
        let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), capacity) } as usize;
        if length == 0 {
            return Err(os_error("GetSystemDirectoryW"));
        }
        if length < buffer.len() {
            buffer.truncate(length);
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        // Too small: `length` is the size needed, including the NUL.
        buffer.resize(length, 0);
    }
}

/// Let the container read and execute `program`. Done once per program
/// and broker; `icacls` is run from the system directory, never `PATH`.
fn grant_executable(program: &Path, sid: &str) -> io::Result<()> {
    static GRANTED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
    let mut granted = GRANTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if granted.iter().any(|path| path == program) {
        return Ok(());
    }
    let status = Command::new(system_directory()?.join("icacls.exe"))
        .arg(program)
        .arg("/grant")
        .arg(format!("*{sid}:(RX)"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "could not let the renderer AppContainer read the executable \
             (the broker needs permission to change its access control list)",
        ));
    }
    granted.push(program.to_owned());
    Ok(())
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(std::iter::once(0)).collect()
}

/// A command line `CommandLineToArgvW` splits back into `program` and
/// `args`. NUL characters cannot be passed and are refused.
fn command_line(program: &OsStr, args: &[&OsStr]) -> io::Result<Vec<u16>> {
    let mut line = Vec::new();
    for (index, argument) in std::iter::once(program)
        .chain(args.iter().copied())
        .enumerate()
    {
        if index > 0 {
            line.push(u16::from(b' '));
        }
        let units: Vec<u16> = argument.encode_wide().collect();
        if units.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an argument contains NUL",
            ));
        }
        quote(&units, &mut line);
    }
    line.push(0);
    Ok(line)
}

/// Quote one argument: backslashes are literal unless they precede a
/// quote, where they are doubled, and a quote is escaped.
fn quote(argument: &[u16], line: &mut Vec<u16>) {
    let (quote_mark, backslash) = (u16::from(b'"'), u16::from(b'\\'));
    line.push(quote_mark);
    let mut backslashes = 0;
    for &unit in argument {
        if unit == backslash {
            backslashes += 1;
            continue;
        }
        let repeat = if unit == quote_mark {
            backslashes * 2 + 1
        } else {
            backslashes
        };
        line.extend(std::iter::repeat_n(backslash, repeat));
        backslashes = 0;
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(backslash, backslashes * 2));
    line.push(quote_mark);
}

/// A sorted, double-NUL-terminated UTF-16 environment block holding `env`
/// and `SystemRoot`. Names with `=` or NUL, and values with NUL, are
/// refused.
fn environment_block(env: &[(OsString, OsString)], system: &Path) -> io::Result<Vec<u16>> {
    let root = system.parent().unwrap_or(system).as_os_str().to_owned();
    let mut pairs: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
    for (name, value) in env
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("SystemRoot"))
        .chain(std::iter::once(&(OsString::from("SystemRoot"), root)))
    {
        let name: Vec<u16> = name.encode_wide().collect();
        let value: Vec<u16> = value.encode_wide().collect();
        if name.is_empty()
            || name.contains(&0)
            || name.contains(&u16::from(b'='))
            || value.contains(&0)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "an environment variable cannot be passed to the renderer",
            ));
        }
        pairs.push((name, value));
    }
    let upper = |name: &[u16]| String::from_utf16_lossy(name).to_uppercase();
    pairs.sort_by_cached_key(|(name, _)| upper(name));
    let mut block = Vec::new();
    for (name, value) in pairs {
        block.extend(name);
        block.push(u16::from(b'='));
        block.extend(value);
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

/// The last Win32 error, naming the call that failed.
fn os_error(step: &str) -> io::Error {
    with_step(step, io::Error::last_os_error())
}

fn with_step(step: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{step}: {error}"))
}

/// An `HRESULT` as an I/O error (Win32 facility codes keep their code).
fn hresult_error(result: i32) -> io::Error {
    let bits = result as u32;
    if bits & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((bits & 0xFFFF) as i32)
    } else {
        io::Error::from_raw_os_error(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(line: &[u16]) -> String {
        String::from_utf16_lossy(&line[..line.len() - 1])
    }

    #[test]
    fn command_lines_quote_spaces_quotes_and_trailing_backslashes() {
        let line = command_line(
            OsStr::new(r"C:\Program Files\rustic.exe"),
            &[
                OsStr::new("__renderer"),
                OsStr::new(r#"a "b"\"#),
                OsStr::new(r"c\d"),
            ],
        )
        .unwrap();
        assert_eq!(
            split(&line),
            r#""C:\Program Files\rustic.exe" "__renderer" "a \"b\"\\" "c\d""#
        );
        assert!(command_line(OsStr::new("x"), &[OsStr::new("a\0b")]).is_err());
    }

    #[test]
    fn environment_blocks_are_sorted_terminated_and_validated() {
        let system = Path::new(r"C:\Windows\System32");
        let block = environment_block(
            &[
                ("b".into(), "2".into()),
                ("A".into(), "1".into()),
                ("SYSTEMROOT".into(), "D:\\evil".into()),
            ],
            system,
        )
        .unwrap();
        assert_eq!(split(&block), "A=1\0b=2\0SystemRoot=C:\\Windows\0");
        assert!(environment_block(&[("a=b".into(), "c".into())], system).is_err());
        assert!(environment_block(&[("".into(), "c".into())], system).is_err());
    }

    #[test]
    fn hresults_map_to_win32_errors() {
        assert_eq!(hresult_error(ALREADY_EXISTS).raw_os_error(), Some(183));
    }
}
