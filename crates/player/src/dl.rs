//! The TV's libraries are opened at run time: nothing links against them, so the app starts (and the simulator
//! builds) without them.

use std::ffi::{CStr, CString, c_void};

use anyhow::{Result, bail};

pub struct Library(*mut c_void);

// a dlopen handle is process-wide
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

impl Library {
    pub fn open(name: &str) -> Result<Library> {
        let cname = CString::new(name)?;
        let handle = unsafe { libc::dlopen(cname.as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if handle.is_null() {
            bail!("dlopen {name}: {}", last_error());
        }
        Ok(Library(handle))
    }

    /// A function pointer of type `T` (an `unsafe extern "C" fn`) for `symbol`.
    pub unsafe fn get<T: Copy>(&self, symbol: &str) -> Result<T> {
        assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
        let cname = CString::new(symbol)?;
        let pointer = unsafe { libc::dlsym(self.0, cname.as_ptr()) };
        if pointer.is_null() {
            bail!("dlsym {symbol}: {}", last_error());
        }
        Ok(unsafe { std::mem::transmute_copy(&pointer) })
    }
}

fn last_error() -> String {
    let error = unsafe { libc::dlerror() };
    if error.is_null() { "unknown error".to_string() } else { unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned() }
}
