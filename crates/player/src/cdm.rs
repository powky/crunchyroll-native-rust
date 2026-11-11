//! Widevine through the TV's own library CDM, the one its Chromium loads for EME: `libwvcdm_shared.so`, LG's build of
//! Chromium's CDM interface 10 (`media/cdm/api/content_decryption_module.h`). We play Chromium's part, the host:
//! the C++ classes the CDM calls back into are laid out here by hand (an object is a pointer to a table of
//! functions, in declaration order, then the two destructors).
//!
//! On this TV `Decrypt` doesn't hand back clear samples: it serializes the encrypted one with its key and IVs for the
//! secure video path (`libsvp_inband_serializer`), and the media pipeline decrypts it inside the TEE.

use std::collections::VecDeque;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};

use crate::Subsample;
use crate::dl::Library;

const LIBRARY: &str = "libwvcdm_shared.so";
const KEY_SYSTEM: &str = "com.widevine.alpha";
const INTERFACE: i32 = 10;
const TIMEOUT: Duration = Duration::from_secs(10);

// cdm::Status
const SUCCESS: u32 = 0;
const NO_KEY: u32 = 2;
// cdm::SessionType, cdm::InitDataType
const TEMPORARY: u32 = 0;
const INIT_DATA_CENC: u32 = 0;
// cdm::QueryResult, cdm::OutputLinkTypes
const QUERY_SUCCEEDED: u32 = 0;
const LINK_INTERNAL: u32 = 1 << 1;

/// How the samples are encrypted (`schm` in the init segment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Clear = 0,
    /// AES-CTR, whole subsamples
    Cenc = 1,
    /// AES-CBC, a pattern of encrypted and skipped blocks
    Cbcs = 2,
}

/// One sample for [`Widevine::decrypt`].
pub struct Encrypted<'a> {
    pub data: &'a [u8],
    pub scheme: Scheme,
    pub key_id: &'a [u8],
    /// 8 or 16 bytes (8 are padded with zeros, as the CDM wants 16)
    pub iv: &'a [u8],
    pub subsamples: &'a [Subsample],
    /// `cbcs` only: encrypted and skipped 16-byte blocks
    pub pattern: (u32, u32),
    pub timestamp_us: i64,
}

/// A key the license holds and whether it can be used (cdm::KeyStatus: 0 usable, 3 output restricted…).
#[derive(Clone, Debug)]
pub struct Key {
    pub id: Vec<u8>,
    pub status: u32,
}

/// One CDM instance with (at most) one session.
pub struct Widevine {
    shared: Arc<Shared>,
    host: *mut Host,
    session: Option<String>,
    next_promise: u32,
}

// every call into the CDM goes through `Shared::with_cdm`, under one lock
unsafe impl Send for Widevine {}

impl Widevine {
    pub fn new() -> Result<Widevine> {
        let module = module()?;
        let shared = Arc::new(Shared::default());
        let host = Box::into_raw(Box::new(Host { vtable: &HOST_VTABLE, shared: shared.clone() }));
        let key_system = KEY_SYSTEM.as_bytes();
        let cdm = unsafe {
            (module.create)(INTERFACE, key_system.as_ptr().cast(), key_system.len() as u32, host_for_version, host.cast())
        };
        if cdm.is_null() {
            unsafe { drop(Box::from_raw(host)) };
            bail!("CreateCdmInstance refused {KEY_SYSTEM}");
        }
        *shared.cdm.lock().unwrap() = Cdm(cdm.cast());
        let widevine = Widevine { shared, host, session: None, next_promise: 1 };
        // as the TV's Chromium does: no distinctive identifier, no persistent state, no hardware-secure codecs
        widevine.shared.with_cdm(|cdm| unsafe { ((*(*cdm).vtable).initialize)(cdm, false, false, false) });
        match widevine.shared.wait(|event| matches!(event, Event::Initialized(_)))? {
            Event::Initialized(true) => Ok(widevine),
            _ => bail!("the CDM failed to initialize"),
        }
    }

    /// The library's own version string.
    pub fn version() -> Result<String> {
        let module = module()?;
        Ok(unsafe { CStr::from_ptr((module.version)()) }.to_string_lossy().into_owned())
    }

    /// The license server's service certificate, for privacy mode (requests encrypted to it).
    pub fn set_server_certificate(&mut self, certificate: &[u8]) -> Result<()> {
        let promise = self.promise();
        self.shared.with_cdm(|cdm| unsafe {
            ((*(*cdm).vtable).set_server_certificate)(cdm, promise, certificate.as_ptr(), certificate.len() as u32)
        });
        match self.shared.wait(|event| event.settles(promise))? {
            Event::Resolved(_) => Ok(()),
            other => bail!("service certificate refused: {}", other.describe()),
        }
    }

    /// Opens a session for the content's `pssh` boxes (CENC init data) and returns the license request to post.
    pub fn license_request(&mut self, init_data: &[u8]) -> Result<Vec<u8>> {
        let promise = self.promise();
        self.shared.with_cdm(|cdm| unsafe {
            ((*(*cdm).vtable).create_session_and_generate_request)(
                cdm,
                promise,
                TEMPORARY,
                INIT_DATA_CENC,
                init_data.as_ptr(),
                init_data.len() as u32,
            )
        });
        let session = match self.shared.wait(|event| event.settles(promise))? {
            Event::NewSession { session, .. } => session,
            other => bail!("no session: {}", other.describe()),
        };
        self.session = Some(session);
        match self.shared.wait(|event| matches!(event, Event::Message { .. }))? {
            Event::Message { message, .. } => Ok(message),
            _ => unreachable!(),
        }
    }

    /// Hands the license server's answer to the session; returns the keys it holds.
    pub fn update(&mut self, license: &[u8]) -> Result<Vec<Key>> {
        let session = self.session.clone().context("no session")?;
        let promise = self.promise();
        self.shared.with_cdm(|cdm| unsafe {
            ((*(*cdm).vtable).update_session)(
                cdm,
                promise,
                session.as_ptr().cast(),
                session.len() as u32,
                license.as_ptr(),
                license.len() as u32,
            )
        });
        match self.shared.wait(|event| event.settles(promise))? {
            Event::Resolved(_) => {}
            other => bail!("license refused: {}", other.describe()),
        }
        // the key statuses come with the answer or right after it
        match self.shared.wait_for(|event| matches!(event, Event::Keys(_)), Duration::from_secs(1)) {
            Ok(Event::Keys(keys)) => Ok(keys),
            _ => Ok(Vec::new()),
        }
    }

    /// A license renewal the CDM asked for since, if any (post it and pass the answer to [`Widevine::update`]).
    pub fn pending_message(&self) -> Option<Vec<u8>> {
        match self.shared.take(|event| matches!(event, Event::Message { .. }))? {
            Event::Message { message, .. } => Some(message),
            _ => None,
        }
    }

    /// The sample as the pipeline takes it: for video, wrapped for the secure path.
    pub fn decrypt(&self, sample: &Encrypted) -> Result<Vec<u8>> {
        let mut iv = [0u8; 16];
        let iv_len = sample.iv.len().min(16);
        iv[..iv_len].copy_from_slice(&sample.iv[..iv_len]);
        let clear = sample.scheme == Scheme::Clear;
        let input = InputBuffer {
            data: sample.data.as_ptr(),
            data_size: sample.data.len() as u32,
            encryption_scheme: sample.scheme as u32,
            key_id: if clear { std::ptr::null() } else { sample.key_id.as_ptr() },
            key_id_size: if clear { 0 } else { sample.key_id.len() as u32 },
            _pad0: 0,
            iv: if clear { std::ptr::null() } else { iv.as_ptr() },
            iv_size: if clear { 0 } else { 16 },
            _pad1: 0,
            subsamples: if sample.subsamples.is_empty() { std::ptr::null() } else { sample.subsamples.as_ptr() },
            num_subsamples: sample.subsamples.len() as u32,
            _pad2: 0,
            crypt_byte_block: sample.pattern.0,
            skip_byte_block: sample.pattern.1,
            timestamp: sample.timestamp_us,
        };
        let mut block = Block { vtable: &BLOCK_VTABLE, buffer: std::ptr::null_mut(), timestamp: 0 };
        let status = self
            .shared
            .with_cdm(|cdm| unsafe { ((*(*cdm).vtable).decrypt)(cdm, &input, &mut block) })
            .context("the CDM is gone")?;
        let out = if block.buffer.is_null() {
            Vec::new()
        } else {
            let buffer = unsafe { Box::from_raw(block.buffer) };
            buffer.data[..buffer.size as usize].to_vec()
        };
        match status {
            SUCCESS => Ok(out),
            NO_KEY => bail!("no key for the sample"),
            status => bail!("Decrypt failed (cdm status {status})"),
        }
    }

    fn promise(&mut self) -> u32 {
        self.next_promise += 1;
        self.next_promise
    }
}

impl Drop for Widevine {
    fn drop(&mut self) {
        let session = self.session.take();
        let promise = self.promise();
        self.shared.with_cdm(|cdm| unsafe {
            if let Some(session) = &session {
                ((*(*cdm).vtable).close_session)(cdm, promise, session.as_ptr().cast(), session.len() as u32);
            }
            ((*(*cdm).vtable).destroy)(cdm);
        });
        *self.shared.cdm.lock().unwrap() = Cdm(std::ptr::null_mut());
        unsafe { drop(Box::from_raw(self.host)) };
    }
}

/// The library, opened and initialized once per process (and never closed, as Chromium does).
struct Module {
    create: unsafe extern "C" fn(i32, *const c_char, u32, GetHost, *mut c_void) -> *mut c_void,
    version: unsafe extern "C" fn() -> *const c_char,
    _library: Library,
}

type GetHost = unsafe extern "C" fn(i32, *mut c_void) -> *mut c_void;

fn module() -> Result<&'static Module> {
    static MODULE: OnceLock<Result<Module, String>> = OnceLock::new();
    MODULE
        .get_or_init(|| {
            let load = || -> Result<Module> {
                let library = Library::open(LIBRARY)?;
                unsafe {
                    let initialize: unsafe extern "C" fn() = library.get("InitializeCdmModule_10")?;
                    let module = Module {
                        create: library.get("CreateCdmInstance")?,
                        version: library.get("GetCdmVersion")?,
                        _library: library,
                    };
                    initialize();
                    Ok(module)
                }
            };
            load().map_err(|error| format!("{error:#}"))
        })
        .as_ref()
        .map_err(|error| anyhow!("{error}"))
}

unsafe extern "C" fn host_for_version(version: i32, host: *mut c_void) -> *mut c_void {
    if version == INTERFACE { host } else { std::ptr::null_mut() }
}

/// What the CDM tells the host, queued for whoever waits on it.
enum Event {
    Initialized(bool),
    NewSession { promise: u32, session: String },
    Resolved(u32),
    Rejected { promise: u32, message: String, system_code: u32 },
    Message { kind: u32, message: Vec<u8> },
    Keys(Vec<Key>),
}

impl Event {
    /// Whether this answers promise `id`.
    fn settles(&self, id: u32) -> bool {
        match self {
            Event::NewSession { promise, .. } | Event::Resolved(promise) | Event::Rejected { promise, .. } => *promise == id,
            _ => false,
        }
    }

    fn trace(&self) -> String {
        match self {
            Event::Initialized(success) => format!("initialized {success}"),
            Event::NewSession { promise, session } => format!("promise {promise}: session {session}"),
            Event::Resolved(promise) => format!("promise {promise} resolved"),
            Event::Rejected { promise, .. } => format!("promise {promise} rejected: {}", self.describe()),
            Event::Message { kind, message } => format!("message of kind {kind}, {} bytes", message.len()),
            Event::Keys(keys) => format!("{} key statuses", keys.len()),
        }
    }

    fn describe(&self) -> String {
        match self {
            Event::Rejected { message, system_code, .. } => format!("{message} (system code {system_code})"),
            Event::Message { kind, .. } => format!("message of kind {kind}"),
            _ => "unexpected answer".to_string(),
        }
    }
}

struct Cdm(*mut CdmObject);
unsafe impl Send for Cdm {}

#[derive(Default)]
struct Shared {
    events: Mutex<VecDeque<Event>>,
    arrived: Condvar,
    /// the instance, null once destroyed; held for every call into it (the CDM expects one caller at a time)
    cdm: Mutex<Cdm>,
}

impl Default for Cdm {
    fn default() -> Cdm {
        Cdm(std::ptr::null_mut())
    }
}

impl Shared {
    /// `None` once the instance is gone.
    fn with_cdm<T>(&self, call: impl FnOnce(*mut CdmObject) -> T) -> Option<T> {
        let cdm = self.cdm.lock().unwrap();
        if cdm.0.is_null() { None } else { Some(call(cdm.0)) }
    }

    fn push(&self, event: Event) {
        if trace() {
            eprintln!("cdm: {}", event.trace());
        }
        self.events.lock().unwrap().push_back(event);
        self.arrived.notify_all();
    }

    fn take(&self, wanted: impl Fn(&Event) -> bool) -> Option<Event> {
        let mut events = self.events.lock().unwrap();
        let index = events.iter().position(&wanted)?;
        events.remove(index)
    }

    fn wait(&self, wanted: impl Fn(&Event) -> bool) -> Result<Event> {
        self.wait_for(wanted, TIMEOUT)
    }

    fn wait_for(&self, wanted: impl Fn(&Event) -> bool, timeout: Duration) -> Result<Event> {
        let deadline = Instant::now() + timeout;
        let mut events = self.events.lock().unwrap();
        loop {
            if let Some(index) = events.iter().position(&wanted) {
                return Ok(events.remove(index).unwrap());
            }
            let now = Instant::now();
            if now >= deadline {
                bail!("the CDM didn't answer in {timeout:?}");
            }
            events = self.arrived.wait_timeout(events, deadline - now).unwrap().0;
        }
    }

    /// Runs `call` on the CDM later, from another thread (the CDM must not be re-entered from a callback).
    fn later(self: &Arc<Self>, delay: Duration, call: impl FnOnce(*mut CdmObject) + Send + 'static) {
        let shared = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            shared.with_cdm(call);
        });
    }
}

// ---- the CDM's classes, as laid out in memory ----

#[repr(C)]
struct InputBuffer {
    data: *const u8,
    data_size: u32,
    encryption_scheme: u32,
    key_id: *const u8,
    key_id_size: u32,
    _pad0: u32,
    iv: *const u8,
    iv_size: u32,
    _pad1: u32,
    subsamples: *const Subsample,
    num_subsamples: u32,
    _pad2: u32,
    crypt_byte_block: u32,
    skip_byte_block: u32,
    timestamp: i64,
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(size_of::<InputBuffer>() == 64);

#[repr(C)]
struct KeyInformation {
    key_id: *const u8,
    key_id_size: u32,
    status: u32,
    system_code: u32,
}

#[repr(C)]
struct CdmObject {
    vtable: *const CdmVtable,
}

/// cdm::ContentDecryptionModule_10; the slots we never call are kept as plain pointers.
#[repr(C)]
struct CdmVtable {
    initialize: unsafe extern "C" fn(*mut CdmObject, bool, bool, bool),
    _get_status_for_policy: usize,
    set_server_certificate: unsafe extern "C" fn(*mut CdmObject, u32, *const u8, u32),
    create_session_and_generate_request: unsafe extern "C" fn(*mut CdmObject, u32, u32, u32, *const u8, u32),
    _load_session: usize,
    update_session: unsafe extern "C" fn(*mut CdmObject, u32, *const c_char, u32, *const u8, u32),
    close_session: unsafe extern "C" fn(*mut CdmObject, u32, *const c_char, u32),
    _remove_session: usize,
    timer_expired: unsafe extern "C" fn(*mut CdmObject, *mut c_void),
    decrypt: unsafe extern "C" fn(*mut CdmObject, *const InputBuffer, *mut Block) -> u32,
    _initialize_audio_decoder: usize,
    _initialize_video_decoder: usize,
    _deinitialize_decoder: usize,
    _reset_decoder: usize,
    _decrypt_and_decode_frame: usize,
    _decrypt_and_decode_samples: usize,
    _on_platform_challenge_response: usize,
    on_query_output_protection_status: unsafe extern "C" fn(*mut CdmObject, u32, u32, u32),
    on_storage_id: unsafe extern "C" fn(*mut CdmObject, u32, *const u8, u32),
    destroy: unsafe extern "C" fn(*mut CdmObject),
}

/// cdm::Buffer, allocated by the host for the CDM's output.
#[repr(C)]
struct Buffer {
    vtable: &'static BufferVtable,
    data: Vec<u8>,
    size: u32,
}

#[repr(C)]
struct BufferVtable {
    destroy: unsafe extern "C" fn(*mut Buffer),
    capacity: unsafe extern "C" fn(*const Buffer) -> u32,
    data: unsafe extern "C" fn(*mut Buffer) -> *mut u8,
    set_size: unsafe extern "C" fn(*mut Buffer, u32),
    size: unsafe extern "C" fn(*const Buffer) -> u32,
    destructor: unsafe extern "C" fn(*mut Buffer),
    deleting_destructor: unsafe extern "C" fn(*mut Buffer),
}

static BUFFER_VTABLE: BufferVtable = BufferVtable {
    destroy: buffer_destroy,
    capacity: buffer_capacity,
    data: buffer_data,
    set_size: buffer_set_size,
    size: buffer_size,
    destructor: buffer_noop,
    deleting_destructor: buffer_destroy,
};

unsafe extern "C" fn buffer_destroy(buffer: *mut Buffer) {
    unsafe { drop(Box::from_raw(buffer)) }
}
unsafe extern "C" fn buffer_capacity(buffer: *const Buffer) -> u32 {
    unsafe { (*buffer).data.len() as u32 }
}
unsafe extern "C" fn buffer_data(buffer: *mut Buffer) -> *mut u8 {
    unsafe { (*buffer).data.as_mut_ptr() }
}
unsafe extern "C" fn buffer_set_size(buffer: *mut Buffer, size: u32) {
    unsafe { (*buffer).size = size.min((*buffer).data.len() as u32) }
}
unsafe extern "C" fn buffer_size(buffer: *const Buffer) -> u32 {
    unsafe { (*buffer).size }
}
unsafe extern "C" fn buffer_noop(_: *mut Buffer) {}

/// cdm::DecryptedBlock, what `Decrypt` writes its output into.
#[repr(C)]
struct Block {
    vtable: &'static BlockVtable,
    buffer: *mut Buffer,
    timestamp: i64,
}

#[repr(C)]
struct BlockVtable {
    set_decrypted_buffer: unsafe extern "C" fn(*mut Block, *mut Buffer),
    decrypted_buffer: unsafe extern "C" fn(*mut Block) -> *mut Buffer,
    set_timestamp: unsafe extern "C" fn(*mut Block, i64),
    timestamp: unsafe extern "C" fn(*const Block) -> i64,
    destructor: unsafe extern "C" fn(*mut Block),
    deleting_destructor: unsafe extern "C" fn(*mut Block),
}

static BLOCK_VTABLE: BlockVtable = BlockVtable {
    set_decrypted_buffer: block_set_buffer,
    decrypted_buffer: block_buffer,
    set_timestamp: block_set_timestamp,
    timestamp: block_timestamp,
    destructor: block_noop,
    deleting_destructor: block_noop,
};

unsafe extern "C" fn block_set_buffer(block: *mut Block, buffer: *mut Buffer) {
    unsafe {
        if !(*block).buffer.is_null() && (*block).buffer != buffer {
            buffer_destroy((*block).buffer);
        }
        (*block).buffer = buffer;
    }
}
unsafe extern "C" fn block_buffer(block: *mut Block) -> *mut Buffer {
    unsafe { (*block).buffer }
}
unsafe extern "C" fn block_set_timestamp(block: *mut Block, timestamp: i64) {
    unsafe { (*block).timestamp = timestamp }
}
unsafe extern "C" fn block_timestamp(block: *const Block) -> i64 {
    unsafe { (*block).timestamp }
}
unsafe extern "C" fn block_noop(_: *mut Block) {}

/// cdm::Host_10: us.
#[repr(C)]
struct Host {
    vtable: &'static HostVtable,
    shared: Arc<Shared>,
}

#[repr(C)]
struct HostVtable {
    allocate: unsafe extern "C" fn(*mut Host, u32) -> *mut Buffer,
    set_timer: unsafe extern "C" fn(*mut Host, i64, *mut c_void),
    get_current_wall_time: unsafe extern "C" fn(*mut Host) -> f64,
    on_initialized: unsafe extern "C" fn(*mut Host, bool),
    on_resolve_key_status_promise: unsafe extern "C" fn(*mut Host, u32, u32),
    on_resolve_new_session_promise: unsafe extern "C" fn(*mut Host, u32, *const c_char, u32),
    on_resolve_promise: unsafe extern "C" fn(*mut Host, u32),
    on_reject_promise: unsafe extern "C" fn(*mut Host, u32, u32, u32, *const c_char, u32),
    on_session_message: unsafe extern "C" fn(*mut Host, *const c_char, u32, u32, *const c_char, u32),
    on_session_keys_change: unsafe extern "C" fn(*mut Host, *const c_char, u32, bool, *const KeyInformation, u32),
    on_expiration_change: unsafe extern "C" fn(*mut Host, *const c_char, u32, f64),
    on_session_closed: unsafe extern "C" fn(*mut Host, *const c_char, u32),
    send_platform_challenge: unsafe extern "C" fn(*mut Host, *const c_char, u32, *const c_char, u32),
    enable_output_protection: unsafe extern "C" fn(*mut Host, u32),
    query_output_protection_status: unsafe extern "C" fn(*mut Host),
    on_deferred_initialization_done: unsafe extern "C" fn(*mut Host, u32, u32),
    create_file_io: unsafe extern "C" fn(*mut Host, *mut c_void) -> *mut c_void,
    request_storage_id: unsafe extern "C" fn(*mut Host, u32),
    destructor: unsafe extern "C" fn(*mut Host),
    deleting_destructor: unsafe extern "C" fn(*mut Host),
}

static HOST_VTABLE: HostVtable = HostVtable {
    allocate: host_allocate,
    set_timer: host_set_timer,
    get_current_wall_time: host_wall_time,
    on_initialized: host_on_initialized,
    on_resolve_key_status_promise: host_on_key_status,
    on_resolve_new_session_promise: host_on_new_session,
    on_resolve_promise: host_on_resolve,
    on_reject_promise: host_on_reject,
    on_session_message: host_on_message,
    on_session_keys_change: host_on_keys_change,
    on_expiration_change: host_on_expiration,
    on_session_closed: host_on_closed,
    send_platform_challenge: host_platform_challenge,
    enable_output_protection: host_enable_output_protection,
    query_output_protection_status: host_query_output_protection,
    on_deferred_initialization_done: host_on_deferred_initialization,
    create_file_io: host_create_file_io,
    request_storage_id: host_request_storage_id,
    destructor: host_noop,
    deleting_destructor: host_noop,
};

/// `CRUNCHY_CDM_TRACE` set: every callback is logged.
fn trace() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("CRUNCHY_CDM_TRACE").is_some())
}

unsafe fn shared<'a>(host: *mut Host) -> &'a Arc<Shared> {
    unsafe { &(*host).shared }
}

unsafe fn text(data: *const c_char, size: u32) -> String {
    if data.is_null() {
        return String::new();
    }
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size as usize) }).into_owned()
}

unsafe extern "C" fn host_allocate(_: *mut Host, capacity: u32) -> *mut Buffer {
    Box::into_raw(Box::new(Buffer { vtable: &BUFFER_VTABLE, data: vec![0; capacity as usize], size: 0 }))
}

struct TimerContext(*mut c_void);
unsafe impl Send for TimerContext {}

unsafe extern "C" fn host_set_timer(host: *mut Host, delay_ms: i64, context: *mut c_void) {
    if trace() {
        eprintln!("cdm: timer in {delay_ms} ms");
    }
    let context = TimerContext(context);
    unsafe { shared(host) }.later(Duration::from_millis(delay_ms.max(0) as u64), move |cdm| {
        let context = context;
        unsafe { ((*(*cdm).vtable).timer_expired)(cdm, context.0) }
    });
}

unsafe extern "C" fn host_wall_time(_: *mut Host) -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |now| now.as_secs_f64())
}

unsafe extern "C" fn host_on_initialized(host: *mut Host, success: bool) {
    unsafe { shared(host) }.push(Event::Initialized(success));
}

unsafe extern "C" fn host_on_key_status(host: *mut Host, promise: u32, _status: u32) {
    unsafe { shared(host) }.push(Event::Resolved(promise));
}

unsafe extern "C" fn host_on_new_session(host: *mut Host, promise: u32, session: *const c_char, size: u32) {
    let session = unsafe { text(session, size) };
    unsafe { shared(host) }.push(Event::NewSession { promise, session });
}

unsafe extern "C" fn host_on_resolve(host: *mut Host, promise: u32) {
    unsafe { shared(host) }.push(Event::Resolved(promise));
}

unsafe extern "C" fn host_on_reject(
    host: *mut Host,
    promise: u32,
    exception: u32,
    system_code: u32,
    message: *const c_char,
    size: u32,
) {
    let message = format!("exception {exception}: {}", unsafe { text(message, size) });
    unsafe { shared(host) }.push(Event::Rejected { promise, message, system_code });
}

unsafe extern "C" fn host_on_message(
    host: *mut Host,
    _session: *const c_char,
    _session_size: u32,
    kind: u32,
    message: *const c_char,
    size: u32,
) {
    let message = if message.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(message.cast::<u8>(), size as usize) }.to_vec()
    };
    unsafe { shared(host) }.push(Event::Message { kind, message });
}

unsafe extern "C" fn host_on_keys_change(
    host: *mut Host,
    _session: *const c_char,
    _session_size: u32,
    _has_new_usable_key: bool,
    keys: *const KeyInformation,
    count: u32,
) {
    let keys = if keys.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(keys, count as usize) } };
    let keys = keys
        .iter()
        .map(|key| Key {
            id: if key.key_id.is_null() {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(key.key_id, key.key_id_size as usize) }.to_vec()
            },
            status: key.status,
        })
        .collect();
    unsafe { shared(host) }.push(Event::Keys(keys));
}

unsafe extern "C" fn host_on_expiration(_: *mut Host, _: *const c_char, _: u32, _: f64) {}

unsafe extern "C" fn host_on_closed(_: *mut Host, _: *const c_char, _: u32) {}

unsafe extern "C" fn host_platform_challenge(_: *mut Host, _: *const c_char, _: u32, _: *const c_char, _: u32) {
    eprintln!("cdm: platform challenge asked for (unsupported)");
}

unsafe extern "C" fn host_enable_output_protection(_: *mut Host, _: u32) {}

/// The panel is built in: an internal link needs no protection.
unsafe extern "C" fn host_query_output_protection(host: *mut Host) {
    if trace() {
        eprintln!("cdm: output protection queried");
    }
    unsafe { shared(host) }.later(Duration::ZERO, |cdm| unsafe {
        ((*(*cdm).vtable).on_query_output_protection_status)(cdm, QUERY_SUCCEEDED, LINK_INTERNAL, 0)
    });
}

unsafe extern "C" fn host_on_deferred_initialization(_: *mut Host, _: u32, _: u32) {}

unsafe extern "C" fn host_create_file_io(_: *mut Host, _: *mut c_void) -> *mut c_void {
    eprintln!("cdm: file storage asked for (none given)");
    std::ptr::null_mut()
}

unsafe extern "C" fn host_request_storage_id(host: *mut Host, version: u32) {
    if trace() {
        eprintln!("cdm: storage id {version} asked for");
    }
    unsafe { shared(host) }.later(Duration::ZERO, move |cdm| unsafe {
        ((*(*cdm).vtable).on_storage_id)(cdm, version, std::ptr::null(), 0)
    });
}

unsafe extern "C" fn host_noop(_: *mut Host) {}
