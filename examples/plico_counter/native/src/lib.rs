mod batch;

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    slice,
};

use batch::Batch;
use plico::{Theme, UiContext, View};
use plico_desktop::WindowOptions;

const MAX_COMMANDS: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Command {
    pub kind: i32,
    pub key: i32,
    pub parent: i32,
    pub title_start: i32,
    pub title_len: i32,
    pub flags: i32,
}

type Build = extern "C" fn(*const i32, i32) -> i32;

enum Reply {
    Idle,
    Awaiting,
    Ready(Batch),
}

thread_local! {
    static REPLY: RefCell<Reply> = const { RefCell::new(Reply::Idle) };
    static RUNNING: Cell<bool> = const { Cell::new(false) };
}

/// Run the winit window on the calling desktop UI thread. The callback
/// remains live until this call returns.
#[doc = "oxel: callback build"]
#[unsafe(no_mangle)]
pub extern "C" fn run_counter(build: Build) -> i32 {
    if RUNNING.replace(true) {
        return 1;
    }
    let status = std::panic::catch_unwind(|| run(build)).unwrap_or(1);
    REPLY.replace(Reply::Idle);
    RUNNING.set(false);
    status
}

fn run(build: Build) -> i32 {
    let Some(batch) = request_batch(build, &[]) else {
        eprintln!("Plico counter received no valid initial view");
        return 1;
    };
    let failed = Rc::new(Cell::new(false));
    let view = Counter {
        build,
        batch,
        failed: Rc::clone(&failed),
    };
    let options = WindowOptions::new()
        .title("Oxel Plico Counter")
        .initial_inner_size(360, 180)
        .min_inner_size(320, 160);
    match plico_desktop::run_view_with_options(view, options) {
        Ok(()) => i32::from(failed.get()),
        Err(error) => {
            eprintln!("Plico window failed: {error}");
            1
        }
    }
}

/// Submit one UI declaration during the build callback. Rust copies accepted
/// data before returning.
///
/// # Safety
/// Nonempty inputs must point to aligned, readable buffers covering their
/// respective element counts for the duration of the call.
#[doc = "oxel: buffer commands count"]
#[doc = "oxel: buffer titles title_bytes"]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn submit(
    commands: *const Command,
    count: usize,
    titles: *const u8,
    title_bytes: usize,
) -> i32 {
    if count == 0
        || count > MAX_COMMANDS
        || commands.is_null()
        || title_bytes > MAX_TEXT_BYTES
        || (title_bytes != 0 && titles.is_null())
    {
        return 1;
    }
    REPLY.with_borrow_mut(|reply| {
        if !matches!(reply, Reply::Awaiting) {
            return 1;
        }
        let commands = unsafe { slice::from_raw_parts(commands, count) };
        let titles = if title_bytes == 0 {
            &[]
        } else {
            unsafe { slice::from_raw_parts(titles, title_bytes) }
        };
        let Some(batch) = Batch::decode(commands, titles) else {
            return 1;
        };
        *reply = Reply::Ready(batch);
        0
    })
}

fn request_batch(build: Build, events: &[i32]) -> Option<Batch> {
    REPLY.replace(Reply::Awaiting);
    let status = build(events.as_ptr(), events.len() as i32);
    match REPLY.replace(Reply::Idle) {
        Reply::Ready(batch) if status == 0 => Some(batch),
        _ => None,
    }
}

struct Counter {
    build: Build,
    batch: Batch,
    failed: Rc<Cell<bool>>,
}

impl View for Counter {
    fn theme(&self) -> Theme {
        Theme::default()
    }

    fn show(&mut self, ui: &mut UiContext<'_>) {
        let events = self.batch.show(ui);
        if !events.is_empty() && !self.failed.get() {
            if let Some(batch) = request_batch(self.build, &events) {
                self.batch = batch;
                ui.request_rebuild();
            } else {
                eprintln!("Plico counter rejected a view update");
                self.failed.set(true);
            }
        }
    }
}
