use std::sync::Mutex;

#[repr(C)]
pub struct Command {
    pub value: i32,
    pub key: i32,
}

type Update = extern "C" fn(i32) -> i32;
type Notify = unsafe extern "C" fn(*mut core::ffi::c_void, i32) -> i32;

static NOTIFY: Mutex<Option<Notify>> = Mutex::new(None);

/// # Safety
/// The buffer is writable for count elements. Rust uses it only during this call.
#[doc = "oxel: buffer_mut commands count"]
#[doc = "oxel: callback update"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn update_commands(
    commands: *mut Command,
    count: u32,
    update: Update,
) -> i32 {
    let mut sum: i32 = 0;
    for index in 0..count {
        let command = unsafe { &mut *commands.add(index as usize) };
        command.value = update(command.value);
        sum = sum.wrapping_add(command.value);
    }
    sum
}

/// # Safety
/// Bytes beyond the first count elements are never read.
/// A nonempty buffer must point to count readable bytes.
#[doc = "oxel: buffer text count"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn first_byte(text: *const u8, count: usize) -> i32 {
    if count == 0 {
        -1
    } else {
        i32::from(unsafe { text.read() })
    }
}

#[doc = "oxel: callback_retained notify"]
#[unsafe(no_mangle)]
pub extern "C" fn set_notify(notify: Option<Notify>) {
    *NOTIFY.lock().unwrap_or_else(|error| error.into_inner()) = notify;
}

/// # Safety
/// Context must satisfy the contract of the registered callback.
#[doc = "oxel: address context"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn notify(context: *mut core::ffi::c_void, event: i32) -> i32 {
    let handler = *NOTIFY.lock().unwrap_or_else(|error| error.into_inner());
    handler.map_or(-1, |handler| unsafe { handler(context, event) })
}
