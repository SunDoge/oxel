#[repr(C)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[doc = "oxel: rename translate"]
#[unsafe(export_name = "oxel_rust_translate")]
pub extern "C" fn move_point(point: Point, dx: i32, dy: i32) -> Point {
    Point {
        x: point.x.wrapping_add(dx),
        y: point.y.wrapping_add(dy),
    }
}

/// # Safety
/// Both pointers must be valid for one aligned i32 write and must not alias.
/// `value` must also be initialized.
#[doc = "oxel: ref value"]
#[doc = "oxel: out previous"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn increment(value: *mut i32, previous: *mut i32) {
    unsafe {
        let before = value.read();
        previous.write(before);
        value.write(before.wrapping_add(1));
    }
}

/// # Safety
/// `text` must be null or point to a readable, NUL-terminated string.
#[doc = "oxel: utf8_nullable text"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn text_bytes(text: *const std::ffi::c_char) -> i32 {
    if text.is_null() {
        return -1;
    }
    unsafe { std::ffi::CStr::from_ptr(text) }.to_bytes().len() as i32
}
