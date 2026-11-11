//! LG's in-process media pipeline, `StarfishMediaAPIs` in libplayerAPIs.so.1 (a C++ class, reached through its
//! mangled symbols): loaded in buffer-feed mode (`BUFFERSTREAM`) and fed one access unit at a time, the way the
//! TV's Chromium plays MSE. Its events come back on the library's own thread.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};

use anyhow::{Result, anyhow, bail};

use crate::dl::Library;

const LIBRARY: &str = "libplayerAPIs.so.1";
/// The class's size isn't published: the object is built in a generously larger block, which is never freed (a late
/// callback may still reach it).
const OBJECT_SIZE: usize = 64 * 1024;

/// Event types seen from the pipeline. 0 comes ~5 times a second while playing, with the position in `number`.
pub const EVENT_POSITION: i32 = 0;

#[derive(Debug)]
pub struct Event {
    pub kind: i32,
    pub number: i64,
    pub text: String,
}

/// What `Feed` answered.
#[derive(Debug, PartialEq, Eq)]
pub enum Fed {
    Ok,
    /// the pipeline holds enough: feed the same buffer again later
    Full,
    Other(String),
}

pub struct Pipeline {
    object: *mut c_void,
    loaded: bool,
}

// the library serializes calls on the object itself
unsafe impl Send for Pipeline {}

impl Pipeline {
    pub fn new() -> Result<Pipeline> {
        let api = api()?;
        let layout = std::alloc::Layout::from_size_align(OBJECT_SIZE, 16)?;
        let object = unsafe { std::alloc::alloc_zeroed(layout) }.cast::<c_void>();
        // the argument is the media server connection's id: none, so the library registers a fresh one (the app
        // id goes in the load payload)
        unsafe { (api.construct)(object, std::ptr::null()) };
        Ok(Pipeline { object, loaded: false })
    }

    /// Loads with `payload` (`{"args":[{"mediaTransportType":"BUFFERSTREAM",…}]}`); events go to the receiver.
    pub fn load(&mut self, payload: &str) -> Result<Receiver<Event>> {
        let api = api()?;
        let (sender, receiver) = channel();
        // leaked: the library keeps calling back with it until it's unloaded, and maybe after
        let context = Box::into_raw(Box::new(Mutex::new(sender))).cast::<c_void>();
        let payload = CString::new(payload)?;
        if !unsafe { (api.load)(self.object, payload.as_ptr(), on_event, context) } {
            bail!("StarfishMediaAPIs::Load refused the payload");
        }
        self.loaded = true;
        unsafe { (api.notify_foreground)(self.object) };
        Ok(receiver)
    }

    /// Feeds one access unit (`video` or audio), presentation time in nanoseconds. The data is copied.
    pub fn feed(&self, data: &[u8], pts_ns: u64, video: bool) -> Fed {
        let Ok(api) = api() else { return Fed::Other("no library".into()) };
        let payload = format!(
            "{{\"bufferAddr\":\"{:p}\",\"bufferSize\":{},\"pts\":{},\"esData\":{}}}",
            data.as_ptr(),
            data.len(),
            pts_ns,
            if video { 1 } else { 2 }
        );
        let payload = CString::new(payload).unwrap();
        let mut reply = CxxString { data: std::ptr::null_mut(), length: 0, local: [0; 16] };
        unsafe { (api.feed)(&mut reply, self.object, payload.as_ptr()) };
        let text = if reply.data.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(reply.data) }.to_string_lossy().into_owned()
        };
        // a reply too long for the string's inline buffer lives on the heap
        if !reply.data.is_null() && reply.data != reply.local.as_mut_ptr() {
            unsafe { (api.delete)(reply.data.cast()) };
        }
        // "Ok", or on newer firmware {"returnValue":"Ok"}
        let value = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|json| json.get("returnValue")?.as_str().map(String::from))
            .unwrap_or_else(|| text.clone());
        match value.as_str() {
            "Ok" => Fed::Ok,
            "BufferFull" => Fed::Full,
            _ => Fed::Other(text),
        }
    }

    pub fn play(&self) -> bool {
        api().is_ok_and(|api| unsafe { (api.play)(self.object) })
    }

    pub fn pause(&self) -> bool {
        api().is_ok_and(|api| unsafe { (api.pause)(self.object) })
    }

    /// Tells the pipeline no more data comes, so it plays out what it holds.
    pub fn end_of_stream(&self) -> bool {
        api().is_ok_and(|api| unsafe { (api.push_eos)(self.object) })
    }

    /// Drops what was fed (before feeding from a new position).
    pub fn flush(&self) -> bool {
        api().is_ok_and(|api| unsafe { (api.flush)(self.object) })
    }

    pub fn seek(&self, millis: u64) -> bool {
        let millis = CString::new(millis.to_string()).unwrap();
        api().is_ok_and(|api| unsafe { (api.seek)(self.object, millis.as_ptr()) })
    }

    /// The pipeline's own volume, 0 to 100.
    pub fn set_volume(&self, volume: u8) -> bool {
        let json = CString::new(format!("{{\"volume\":{volume}}}")).unwrap();
        api().is_ok_and(|api| unsafe { (api.set_volume)(self.object, json.as_ptr()) })
    }

    /// Whether loading finished (`play` before that is ignored).
    pub fn load_completed(&self) -> bool {
        api().is_ok_and(|api| unsafe { (api.is_load_completed)(self.object) })
    }

    /// Where playback is, in nanoseconds (what Chromium polls, since `queryPosition` loads send no position events).
    pub fn position(&self) -> i64 {
        api().map_or(-1, |api| unsafe { (api.current_playtime)(self.object) })
    }

    /// The id the pipeline registered with the media server (for display and audio routing).
    pub fn media_id(&self) -> Option<String> {
        let api = api().ok()?;
        let id = unsafe { (api.media_id)(self.object) };
        (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_string_lossy().into_owned())
    }

    pub fn unload(&mut self) {
        if std::mem::take(&mut self.loaded)
            && let Ok(api) = api()
        {
            unsafe { (api.unload)(self.object) };
        }
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        self.unload();
    }
}

unsafe extern "C" fn on_event(kind: i32, number: i64, text: *const c_char, context: *mut c_void) {
    let text = if text.is_null() { String::new() } else { unsafe { CStr::from_ptr(text) }.to_string_lossy().into_owned() };
    let sender = unsafe { &*context.cast::<Mutex<Sender<Event>>>() };
    if let Ok(sender) = sender.lock() {
        let _ = sender.send(Event { kind, number, text });
    }
}

/// libstdc++'s `std::string` (C++11 ABI), as `Feed` returns it.
#[repr(C)]
struct CxxString {
    data: *mut c_char,
    length: usize,
    local: [c_char; 16],
}

type Callback = unsafe extern "C" fn(i32, i64, *const c_char, *mut c_void);

struct Api {
    construct: unsafe extern "C" fn(*mut c_void, *const c_char),
    load: unsafe extern "C" fn(*mut c_void, *const c_char, Callback, *mut c_void) -> bool,
    /// returns `std::string`: the caller passes where to build it first, then `this`
    feed: unsafe extern "C" fn(*mut CxxString, *mut c_void, *const c_char),
    play: unsafe extern "C" fn(*mut c_void) -> bool,
    pause: unsafe extern "C" fn(*mut c_void) -> bool,
    push_eos: unsafe extern "C" fn(*mut c_void) -> bool,
    flush: unsafe extern "C" fn(*mut c_void) -> bool,
    seek: unsafe extern "C" fn(*mut c_void, *const c_char) -> bool,
    set_volume: unsafe extern "C" fn(*mut c_void, *const c_char) -> bool,
    unload: unsafe extern "C" fn(*mut c_void) -> bool,
    notify_foreground: unsafe extern "C" fn(*mut c_void) -> bool,
    media_id: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    current_playtime: unsafe extern "C" fn(*mut c_void) -> i64,
    is_load_completed: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `operator delete`, for heap strings the library hands back
    delete: unsafe extern "C" fn(*mut c_void),
    _library: Library,
}

fn api() -> Result<&'static Api> {
    static API: OnceLock<Result<Api, String>> = OnceLock::new();
    API.get_or_init(|| {
        let load = || -> Result<Api> {
            let library = Library::open(LIBRARY)?;
            unsafe {
                Ok(Api {
                    construct: library.get("_ZN17StarfishMediaAPIsC1EPKc")?,
                    load: library.get("_ZN17StarfishMediaAPIs4LoadEPKcPFvixS1_PvES2_")?,
                    feed: library.get("_ZN17StarfishMediaAPIs4FeedB5cxx11EPKc")?,
                    play: library.get("_ZN17StarfishMediaAPIs4PlayEv")?,
                    pause: library.get("_ZN17StarfishMediaAPIs5PauseEv")?,
                    push_eos: library.get("_ZN17StarfishMediaAPIs7pushEOSEv")?,
                    flush: library.get("_ZN17StarfishMediaAPIs5flushEv")?,
                    seek: library.get("_ZN17StarfishMediaAPIs4SeekEPKc")?,
                    set_volume: library.get("_ZN17StarfishMediaAPIs9setVolumeEPKc")?,
                    unload: library.get("_ZN17StarfishMediaAPIs6UnloadEv")?,
                    notify_foreground: library.get("_ZN17StarfishMediaAPIs16notifyForegroundEv")?,
                    media_id: library.get("_ZN17StarfishMediaAPIs10getMediaIDEv")?,
                    current_playtime: library.get("_ZN17StarfishMediaAPIs18getCurrentPlaytimeEv")?,
                    is_load_completed: library.get("_ZN17StarfishMediaAPIs15isLoadCompletedEv")?,
                    delete: library.get("_ZdlPv")?,
                    _library: library,
                })
            }
        };
        load().map_err(|error| format!("{error:#}"))
    })
    .as_ref()
    .map_err(|error| anyhow!("{error}"))
}
