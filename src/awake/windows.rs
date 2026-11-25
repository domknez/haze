//! Windows: power requests that keep the display (and so the system) on, or the
//! system alone.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Power::{
    POWER_REQUEST_TYPE, PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired, PowerSetRequest,
};
use windows_sys::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows_sys::Win32::System::Threading::{POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0};

/// The power request, if Windows granted one; cleared and closed when dropped.
pub struct Hold {
    _request: Option<Request>,
}

/// A power request and what it asks for.
struct Request(HANDLE, &'static [POWER_REQUEST_TYPE]);

// SAFETY: a power request is a kernel handle; any thread may clear and close it.
unsafe impl Send for Request {}

/// Windows doesn't throttle a player in the background: nothing to hold.
pub fn begin() -> Hold {
    Hold { _request: None }
}

/// The display on, and the system awake to show it.
pub fn screen() -> Hold {
    request(&[PowerRequestDisplayRequired, PowerRequestSystemRequired], "Showing video")
}

/// The system awake; the display may go off.
pub fn recording() -> Hold {
    request(&[PowerRequestSystemRequired], "Recording")
}

fn request(needs: &'static [POWER_REQUEST_TYPE], why: &str) -> Hold {
    let mut reason: Vec<u16> = why.encode_utf16().chain([0]).collect();
    let context = REASON_CONTEXT {
        Version: POWER_REQUEST_CONTEXT_VERSION,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 { SimpleReasonString: reason.as_mut_ptr() },
    };
    // SAFETY: the context and the string it points at outlive the call, which copies them.
    let handle = unsafe { PowerCreateRequest(&context) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        log_warn!("couldn't keep the computer awake: {}", std::io::Error::last_os_error());
        return Hold { _request: None };
    }
    let request = Request(handle, needs);
    for &need in needs {
        // SAFETY: the handle is the power request just created, open until dropped.
        if unsafe { PowerSetRequest(request.0, need) } == 0 {
            log_warn!("couldn't keep the computer awake: {}", std::io::Error::last_os_error());
        }
    }
    Hold { _request: Some(request) }
}

impl Drop for Request {
    fn drop(&mut self) {
        // SAFETY: the handle is open (closed only here); clearing a request that wasn't
        // set just fails.
        unsafe {
            for &need in self.1 {
                PowerClearRequest(self.0, need);
            }
            CloseHandle(self.0);
        }
    }
}

pub fn high_priority_thread() {}
