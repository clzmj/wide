//! Read-only exchanges and a separate one-shot Set VCP primitive.
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, io::Write};
#[cfg(any(target_os = "macos", test))]
use std::{
    io::Read,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const READ_WORKER_MARKER: &str = "__wide-ddc-read";
#[cfg(target_os = "macos")]
const READ_WORKER_TIMEOUT: Duration = Duration::from_millis(1500);

/// An advisory native-I/O lease. Keep it alive through a native operation
/// or software reconnect; dropping it releases the lease. A read worker owns
/// its own lease, so an uninterruptible pending worker keeps the bus unavailable.
#[derive(Debug)]
pub struct BusGuard {
    #[cfg(unix)]
    _file: std::fs::File,
}

impl Drop for BusGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            extern "C" {
                fn flock(fd: i32, operation: i32) -> i32;
            }
            // Explicitly release this lease even if a concurrent fork briefly
            // inherited the open file description before its exec closes it.
            unsafe {
                flock(self._file.as_raw_fd(), 8);
            } // LOCK_UN
        }
    }
}

pub fn lock_bus(registry_id: u64) -> Result<BusGuard, String> {
    #[cfg(unix)]
    {
        use std::os::{
            fd::AsRawFd,
            unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        };
        extern "C" {
            fn flock(fd: i32, operation: i32) -> i32;
            fn getuid() -> u32;
        }
        let directory = std::env::temp_dir().join("wide-ddc-locks");
        if let Err(error) = std::fs::DirBuilder::new().mode(0o700).create(&directory) {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(format!(
                    "Could not create the private DDC bus-lock directory: {error}"
                ));
            }
        }
        let metadata = std::fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { getuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(
                "DDC bus-lock directory must be a private directory owned by this user".into(),
            );
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            // A proxy can be replaced while its old kernel call is pending.
            // ponytail: serialize all native interfaces until a verified stable
            // physical-port key is available; never bypass a pending request.
            .open(directory.join("native-io.lock"))
            .map_err(|error| error.to_string())?;
        // LOCK_EX | LOCK_NB: never wait behind a pending IOKit request.
        if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
            return Err(format!("DDC bus busy for registry {registry_id}; a native request or recovery may still be running ({})", std::io::Error::last_os_error()));
        }
        Ok(BusGuard { _file: file })
    }
    #[cfg(not(unix))]
    {
        let _ = registry_id;
        Err("DDC bus locking requires a Unix host".into())
    }
}

struct ReadRequest {
    registry_id: u64,
    payload: Vec<u8>,
    read_len: usize,
    delay_ms: u64,
}

fn parse_worker_request(arguments: &[OsString]) -> Result<ReadRequest, String> {
    if arguments.len() != 5 || arguments[0] != READ_WORKER_MARKER {
        return Err("Invalid internal read-worker arguments".into());
    }
    let argument = |index: usize| {
        arguments[index]
            .to_str()
            .ok_or("Non-UTF-8 internal read argument")
    };
    let registry_id = argument(1)?
        .parse()
        .map_err(|_| "Invalid read-worker registry ID")?;
    let payload = decode_hex(argument(2)?)?;
    let read_len = argument(3)?
        .parse()
        .map_err(|_| "Invalid read-worker response length")?;
    let delay_ms = argument(4)?
        .parse()
        .map_err(|_| "Invalid read-worker delay")?;
    validate_request(&payload, 0x51, read_len)?;
    if delay_ms > 1000 {
        return Err("Read-worker delay must not exceed 1000ms".into());
    }
    Ok(ReadRequest {
        registry_id,
        payload,
        read_len,
        delay_ms,
    })
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.is_ascii() || value.len() % 2 != 0 || value.len() > 512 {
        return Err("Invalid internal read payload hex".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| "Invalid internal read payload hex".into())
        })
        .collect()
}

/// Call before normal CLI parsing. This entry can only issue validated read requests;
/// it never calls the public NativeTransport exchange method or any Set primitive.
pub fn maybe_run_read_worker() -> Option<i32> {
    let mut arguments = std::env::args_os().skip(1);
    let marker = arguments.next()?;
    if marker != READ_WORKER_MARKER {
        return None;
    }
    let mut all = vec![marker];
    all.extend(arguments);
    let result = (|| {
        let request = parse_worker_request(&all)?;
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "warn".into()),
            )
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();
        tracing::debug!(
            phase = "worker_start",
            pid = std::process::id(),
            registry_id = request.registry_id,
            "DDC read worker"
        );
        let exchange = run_read_worker(request)?;
        tracing::debug!(phase = "stdout_serialize_begin", "DDC read worker");
        let mut stdout = std::io::stdout().lock();
        serde_json::to_writer(&mut stdout, &exchange).map_err(|error| error.to_string())?;
        writeln!(stdout).map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;
        tracing::debug!(phase = "stdout_serialize_end", "DDC read worker");
        Ok::<_, String>(())
    })();
    match result {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("DDC read worker: {error}");
            // Preserve a fail-fast bus-busy/setup reason for the supervising process.
            let mut stdout = std::io::stdout().lock();
            let _ = serde_json::to_writer(&mut stdout, &serde_json::json!({"error":error}));
            let _ = writeln!(stdout);
            let _ = stdout.flush();
            Some(2)
        }
    }
}

fn run_read_worker(request: ReadRequest) -> Result<Exchange, String> {
    #[cfg(target_os = "macos")]
    {
        tracing::debug!(
            phase = "bus_lock_begin",
            registry_id = request.registry_id,
            "DDC read worker"
        );
        let guard = lock_bus(request.registry_id)?;
        tracing::debug!(phase = "bus_lock_return", "DDC read worker");
        tracing::debug!(phase = "backend_open_begin", "DDC read worker");
        let mut backend = mac::Backend::open(request.registry_id)?;
        tracing::debug!(phase = "backend_open_return", "DDC read worker");
        let exchange = backend.exchange(&request.payload, request.read_len, request.delay_ms, 0x51);
        tracing::debug!(phase="exchange_complete",write_status=exchange.write_status,read_status=?exchange.read_status,"DDC read worker");
        tracing::debug!(phase = "backend_drop_begin", "DDC read worker");
        drop(backend);
        tracing::debug!(phase = "backend_drop_end", "DDC read worker");
        tracing::debug!(phase = "guard_release_begin", "DDC read worker");
        drop(guard);
        tracing::debug!(phase = "guard_release_end", "DDC read worker");
        Ok(exchange)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            request.registry_id,
            request.payload,
            request.read_len,
            request.delay_ms,
        );
        Err("Native DDC read workers require macOS".into())
    }
}

#[cfg(any(target_os = "macos", test))]
fn relay_completed_worker_stderr(child: &mut Child, limit: usize) {
    // Only call after try_wait confirms exit. A pending kernel call may hold its
    // private pipe open indefinitely; reading that pipe before exit would hang.
    if let Some(stderr) = child.stderr.take() {
        let mut bytes = Vec::new();
        if stderr.take(limit as u64).read_to_end(&mut bytes).is_ok() {
            let _ = std::io::stderr().lock().write_all(&bytes);
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn drain_worker_stderr_nonblocking(child: &mut Child, limit: usize) -> Result<Vec<u8>, String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        use std::os::fd::AsRawFd;
        extern "C" {
            fn fcntl(fd: i32, command: i32, ...) -> i32;
        }
        // Darwin SDK usr/include/sys/fcntl.h: F_GETFL=3, F_SETFL=4,
        // O_NONBLOCK=0x4. Linux uses the same commands and O_NONBLOCK=0x800.
        #[cfg(target_os = "macos")]
        const O_NONBLOCK: i32 = 0x4;
        #[cfg(target_os = "linux")]
        const O_NONBLOCK: i32 = 0x800;
        let Some(stderr) = child.stderr.as_mut() else {
            return Ok(Vec::new());
        };
        let fd = stderr.as_raw_fd();
        let flags = unsafe { fcntl(fd, 3) };
        if flags < 0 || unsafe { fcntl(fd, 4, flags | O_NONBLOCK) } < 0 {
            return Err(format!(
                "Could not make worker diagnostics nonblocking: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 1024];
        while bytes.len() < limit {
            let available = buffer.len().min(limit - bytes.len());
            match stderr.read(&mut buffer[..available]) {
                Ok(0) => break,
                Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    break
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        Ok(bytes)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (child, limit);
        Err("Nonblocking worker diagnostics are unavailable on this platform".into())
    }
}

#[cfg(any(target_os = "macos", test))]
fn stop_read_worker(child: &mut Child, grace: Duration) -> String {
    // Keep the pipe private, but retain available lifecycle evidence before
    // killing a worker that may remain inside an uncancellable driver call.
    let consumed = match drain_worker_stderr_nonblocking(child, 8192) {
        Ok(bytes) => {
            let _ = std::io::stderr().lock().write_all(&bytes);
            bytes.len()
        }
        Err(error) => {
            tracing::debug!(%error,"Could not collect pending worker diagnostics");
            0
        }
    };
    let kill_error = child.kill().err();
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                relay_completed_worker_stderr(child, 8192 - consumed);
                return if let Some(error) = kill_error {
                    format!("termination returned {error}; worker reaped ({status})")
                } else { "worker terminated and reaped".into() };
            }
            Err(error) => return format!("worker termination requested but its exit could not be checked: {error}"),
            Ok(None) if started.elapsed() >= grace => return "worker still stopping after SIGKILL; its bus lease remains held until it exits. Further setting/recovery calls must acquire the bus lease".into(),
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn wait_for_read_worker(child: &mut Child, timeout: Duration) -> Result<Vec<u8>, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                relay_completed_worker_stderr(child, 8192);
                let mut bytes = Vec::new();
                child
                    .stdout
                    .take()
                    .ok_or("DDC read worker stdout unavailable")?
                    .take(8193)
                    .read_to_end(&mut bytes)
                    .map_err(|error| error.to_string())?;
                if bytes.len() > 8192 {
                    return Err("DDC read worker response exceeded its limit".into());
                }
                if !status.success() {
                    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                        if let Some(error) = value["error"].as_str() {
                            return Err(error.into());
                        }
                    }
                    return Err(format!("DDC read worker exited with {status}"));
                }
                return Ok(bytes);
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10))
            }
            Ok(None) => {
                // SIGKILL cannot necessarily cancel an in-flight driver call. Poll
                // reaping briefly, but never block the CLI in Child::wait().
                let stopped = stop_read_worker(child, Duration::from_millis(100));
                return Err(format!(
                    "DDC read timed out after {}ms; {stopped}",
                    timeout.as_millis()
                ));
            }
            Err(error) => {
                let stopped = stop_read_worker(child, Duration::from_millis(100));
                return Err(format!(
                    "Could not wait for DDC read worker: {error}; {stopped}"
                ));
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn subprocess_read(request: ReadRequest) -> Result<Exchange, String> {
    let mut child = Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
        .arg(READ_WORKER_MARKER)
        .arg(request.registry_id.to_string())
        .arg(hex(&request.payload))
        .arg(request.read_len.to_string())
        .arg(request.delay_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // A pending child must not retain the outer CLI/capture stderr pipe.
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start DDC read worker: {error}"))?;
    let bytes = wait_for_read_worker(&mut child, READ_WORKER_TIMEOUT)?;
    let exchange: Exchange = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid DDC read worker response: {error}"))?;
    if exchange.request_hex != hex(&request.payload)
        || decode_hex(&exchange.response_hex)?.len() > request.read_len
    {
        return Err("DDC read worker returned a mismatched exchange".into());
    }
    Ok(exchange)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exchange {
    pub request_hex: String,
    pub response_hex: String,
    pub write_status: i32,
    pub read_status: Option<i32>,
    pub elapsed_ms: f64,
}

pub trait Transport {
    /// Payload includes length and checksum, excluding the source address.
    fn exchange(
        &mut self,
        payload: &[u8],
        read_len: usize,
        delay_ms: u64,
    ) -> Result<Exchange, String>;
    /// One vendor-neutral setting attempt; policy and user approval belong to the caller.
    fn write_once(&mut self, _payload: &[u8]) -> Result<Exchange, String> {
        Err("This transport does not support setting writes".into())
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn validate_request(payload: &[u8], source: u8, read_len: usize) -> Result<(), String> {
    let get = payload.len() == 4 && payload[0] == 0x82 && payload[1] == 1;
    let caps = payload.len() == 5 && payload[0] == 0x83 && payload[1] == 0xf3;
    if !get && !caps {
        return Err(
            "Read-only transport accepts only framed Get VCP (01) or capabilities (f3) requests"
                .into(),
        );
    }
    if payload.iter().fold(0x6e ^ source, |sum, byte| sum ^ byte) != 0 {
        return Err("Invalid DDC request checksum".into());
    }
    if !(1..=256).contains(&read_len) {
        return Err("Read length must be between 1 and 256 bytes".into());
    }
    Ok(())
}

pub struct NativeTransport {
    #[cfg(target_os = "macos")]
    registry_id: u64,
    source: u8,
    write_used: bool,
}
impl NativeTransport {
    pub fn open(registry_id: u64) -> Result<Self, String> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                registry_id,
                source: 0x51,
                write_used: false,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = registry_id;
            Err("Native DDC requires macOS on Apple Silicon".into())
        }
    }

    /// One experimental Set VCP attempt per handle, including failed attempts.
    /// The caller must enforce the exact user-authorized feature/value policy.
    #[cfg(feature = "experimental-writes")]
    #[allow(dead_code)] // Invoked only by the separately gated experimental example.
    pub fn experimental_write_once(&mut self, payload: &[u8]) -> Result<Exchange, String> {
        self.write_once(payload)
    }
}

pub(crate) fn validate_set_request(payload: &[u8]) -> Result<(), String> {
    if payload.len() != 6 || payload[0] != 0x84 || payload[1] != 0x03 {
        return Err("Setting write requires one framed Set VCP request".into());
    }
    if payload.iter().fold(0x6e ^ 0x51, |sum, byte| sum ^ byte) != 0 {
        return Err("Invalid Set VCP checksum".into());
    }
    Ok(())
}
impl Transport for NativeTransport {
    fn write_once(&mut self, payload: &[u8]) -> Result<Exchange, String> {
        // Consume even a failed attempt before validation or any kernel call.
        if std::mem::replace(&mut self.write_used, true) {
            return Err("Setting write attempt already consumed for this transport".into());
        }
        validate_set_request(payload)?;
        #[cfg(target_os = "macos")]
        {
            let _guard = lock_bus(self.registry_id)?;
            let mut backend = mac::Backend::open(self.registry_id)?;
            Ok(backend.write_once(payload))
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Native DDC requires macOS on Apple Silicon".into())
        }
    }
    fn exchange(
        &mut self,
        payload: &[u8],
        read_len: usize,
        delay_ms: u64,
    ) -> Result<Exchange, String> {
        validate_request(payload, self.source, read_len)?;
        #[cfg(target_os = "macos")]
        {
            if delay_ms > 1000 {
                return Err("Read-worker delay must not exceed 1000ms".into());
            }
            subprocess_read(ReadRequest {
                registry_id: self.registry_id,
                payload: payload.to_vec(),
                read_len,
                delay_ms,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = delay_ms;
            Err("Native DDC requires macOS on Apple Silicon".into())
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use crate::display::mac::{self as registry, Cf, Object, OwnedCf};
    use std::ffi::{c_char, c_void};
    type Create = unsafe extern "C" fn(Cf, u32) -> Cf;
    type Write = unsafe extern "C" fn(Cf, u32, u32, *const u8, u32) -> i32;
    type Read = unsafe extern "C" fn(Cf, u32, u32, *mut u8, u32) -> i32;
    extern "C" {
        fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        fn dlclose(handle: *mut c_void) -> i32;
    }
    struct Library(*mut c_void);
    impl Drop for Library {
        fn drop(&mut self) {
            unsafe {
                dlclose(self.0);
            }
        }
    }
    pub struct Backend {
        // Declaration order releases AV object before proxy and loaded library.
        service: OwnedCf,
        _proxy: Object,
        _library: Library,
        write: Write,
        read: Read,
    }
    impl Backend {
        pub fn open(registry_id: u64) -> Result<Self, String> {
            let mut matches = Vec::new();
            for proxy in registry::proxies()? {
                if registry::registry_id(proxy.0)? == registry_id {
                    matches.push(proxy);
                }
            }
            if matches.len() != 1 {
                return Err(format!("Expected exactly one external AV proxy with registry ID {registry_id}; found {}. Run list again.", matches.len()));
            }
            let proxy = matches.remove(0);
            let handle = unsafe {
                dlopen(
                    c"/System/Library/Frameworks/IOKit.framework/IOKit".as_ptr(),
                    2,
                )
            };
            if handle.is_null() {
                return Err("Cannot load IOKit framework".into());
            }
            let library = Library(handle);
            let create = unsafe { dlsym(handle, c"IOAVServiceCreateWithService".as_ptr()) };
            let write = unsafe { dlsym(handle, c"IOAVServiceWriteI2C".as_ptr()) };
            let read = unsafe { dlsym(handle, c"IOAVServiceReadI2C".as_ptr()) };
            if create.is_null() || write.is_null() || read.is_null() {
                return Err("IOAVService private symbols unavailable on this macOS version".into());
            }
            // Signatures mirror references/OpenDisplay/.../DDCControl.swift.
            let create: Create = unsafe { std::mem::transmute(create) };
            let service = unsafe { create(std::ptr::null(), proxy.0) };
            if service.is_null() {
                return Err(format!(
                    "IOAVServiceCreateWithService failed for registry ID {registry_id}"
                ));
            }
            Ok(Self {
                service: OwnedCf(service),
                _proxy: proxy,
                _library: library,
                write: unsafe { std::mem::transmute::<*mut c_void, Write>(write) },
                read: unsafe { std::mem::transmute::<*mut c_void, Read>(read) },
            })
        }
        pub fn exchange(
            &mut self,
            payload: &[u8],
            read_len: usize,
            delay_ms: u64,
            source: u8,
        ) -> Exchange {
            let start = std::time::Instant::now();
            tracing::debug!(phase="request_write_begin",request=%hex(payload),"DDC exchange");
            let write_status = unsafe {
                (self.write)(
                    self.service.0,
                    0x37,
                    source.into(),
                    payload.as_ptr(),
                    payload.len() as u32,
                )
            };
            tracing::debug!(
                phase = "request_write_return",
                write_status,
                elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                "DDC exchange"
            );
            let mut response = Vec::new();
            let read_status = if write_status == 0 {
                std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                response.resize(read_len, 0);
                tracing::debug!(phase = "reply_read_begin", read_len, "DDC exchange");
                let status = unsafe {
                    (self.read)(
                        self.service.0,
                        0x37,
                        source.into(),
                        response.as_mut_ptr(),
                        read_len as u32,
                    )
                };
                tracing::debug!(
                    phase = "reply_read_return",
                    read_status = status,
                    elapsed_ms = start.elapsed().as_secs_f64() * 1000.0,
                    "DDC exchange"
                );
                Some(status)
            } else {
                None
            };
            Exchange {
                request_hex: hex(payload),
                response_hex: hex(&response),
                write_status,
                read_status,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            }
        }

        pub(super) fn write_once(&mut self, payload: &[u8]) -> Exchange {
            let start = std::time::Instant::now();
            let write_status = unsafe {
                (self.write)(
                    self.service.0,
                    0x37,
                    0x51,
                    payload.as_ptr(),
                    payload.len() as u32,
                )
            };
            Exchange {
                request_hex: hex(payload),
                response_hex: String::new(),
                write_status,
                read_status: None,
                elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_settings_and_malformed_read_requests() {
        assert!(validate_request(&[0x82, 1, 0x10, 0xac], 0x51, 48).is_ok());
        assert!(validate_request(&[0x83, 0xf3, 0, 0, 0x4f], 0x51, 64).is_ok());
        assert!(validate_request(&[0x84, 3, 0x10, 0, 50, 0], 0x51, 48).is_err());
        assert!(validate_request(&[0x82, 1, 0x10, 0], 0x51, 48).is_err());
        assert!(validate_request(&[0x82, 1, 0x10, 0xac], 0x51, 257).is_err());
    }

    #[test]
    fn setting_boundary_requires_set_framing_and_checksum() {
        let set = [0x84, 0x03, 0xe2, 0, 0, 0x5a];
        assert!(validate_set_request(&set).is_ok());
        assert!(validate_request(&set, 0x51, 48).is_err());
        let mut bad = set;
        bad[5] ^= 1;
        assert!(validate_set_request(&bad).is_err());
        assert!(validate_set_request(&[0x82, 1, 0x10, 0xac]).is_err());
    }

    #[test]
    fn worker_rejects_sets_and_malformed_requests_before_native_open() {
        let request = |payload: &str, length: &str, delay: &str| {
            parse_worker_request(&[
                READ_WORKER_MARKER.into(),
                "1".into(),
                payload.into(),
                length.into(),
                delay.into(),
            ])
        };
        assert!(request("8201e25e", "48", "100").is_ok());
        assert!(request("83f300004f", "64", "150").is_ok());
        assert!(request("8403e8000151", "48", "100").is_err());
        assert!(request("8201e200", "48", "100").is_err());
        assert!(request("82", "48", "100").is_err());
        assert!(request("8201e25e", "257", "100").is_err());
        assert!(request("8201e25e", "48", "1001").is_err());
        assert!(request("é", "48", "100").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn stalled_read_worker_is_killed_and_reaped_before_timeout_error_returns() {
        // A plain owned sleep process exercises supervision without loading IOAV
        // or running any monitor request. exec avoids leaving a descendant behind.
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec /bin/sleep 5"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let result = wait_for_read_worker(&mut child, Duration::from_millis(50));
        assert!(result.unwrap_err().contains("timed out"));
        assert!(child.try_wait().unwrap().is_some());
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn completed_worker_stderr_is_private_and_consumed_only_after_exit() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "printf 'worker diagnostic\n' >&2; printf response"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert!(child.stderr.is_some());
        assert_eq!(
            wait_for_read_worker(&mut child, Duration::from_secs(1)).unwrap(),
            b"response"
        );
        assert!(child.stderr.is_none());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn pending_worker_diagnostics_are_bounded_and_do_not_wait_for_eof() {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "printf 'phase=request_write_begin\n' >&2; exec /bin/sleep 5",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut first = Vec::new();
        while first.is_empty() && Instant::now() < deadline {
            first = drain_worker_stderr_nonblocking(&mut child, 8).unwrap();
            if first.is_empty() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let mut captured = first.clone();
        captured.extend(drain_worker_stderr_nonblocking(&mut child, 8192).unwrap());
        let alive = child.try_wait().unwrap().is_none();
        let started = Instant::now();
        let empty = drain_worker_stderr_nonblocking(&mut child, 8192).unwrap();
        let drain_elapsed = started.elapsed();
        let _ = stop_read_worker(&mut child, Duration::from_millis(100));
        assert_eq!(first.len(), 8, "the caller's byte cap must be respected");
        assert_eq!(captured, b"phase=request_write_begin\n");
        assert!(
            alive,
            "the diagnostic producer must still have its pipe open"
        );
        assert!(empty.is_empty());
        assert!(drain_elapsed < Duration::from_secs(1));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn bus_lease_refuses_a_second_holder_and_releases_on_drop() {
        // A synthetic registry number exercises filesystem locking only.
        let registry_id = u64::MAX - u64::from(std::process::id());
        let first = lock_bus(registry_id).unwrap();
        assert!(lock_bus(registry_id).unwrap_err().contains("bus busy"));
        // A replacement proxy must not bypass the outstanding native call.
        assert!(lock_bus(registry_id - 1).unwrap_err().contains("bus busy"));
        drop(first);
        let next = lock_bus(registry_id).unwrap();
        drop(next);
    }
}
