mod batch;

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    slice,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use batch::Batch;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use plico::UiContext;
use plico_desktop::{HostView, WakeCallback, WindowOptions};

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
pub extern "C" fn run_counter(poll_reload: i32, build: Build) -> i32 {
    if RUNNING.replace(true) {
        return 1;
    }
    let status = std::panic::catch_unwind(|| run(build, poll_reload != 0)).unwrap_or(1);
    REPLY.replace(Reply::Idle);
    RUNNING.set(false);
    status
}

fn run(build: Build, poll_reload: bool) -> i32 {
    let Ok(Some(batch)) = request_batch(build, &[]) else {
        eprintln!("Plico counter received no valid initial view");
        return 1;
    };
    let failed = Rc::new(Cell::new(false));
    let view = Counter {
        build,
        batch,
        failed: Rc::clone(&failed),
        reload: poll_reload.then(Reload::default),
    };
    let options = WindowOptions::new()
        .title("Oxel Plico Counter")
        .initial_inner_size(360, 180)
        .min_inner_size(320, 160);
    match plico_desktop::run_host_view_with_options(view, options) {
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

fn request_batch(build: Build, events: &[i32]) -> Result<Option<Batch>, ()> {
    REPLY.replace(Reply::Awaiting);
    let status = build(events.as_ptr(), events.len() as i32);
    match REPLY.replace(Reply::Idle) {
        Reply::Ready(batch) if status == 0 => Ok(Some(batch)),
        Reply::Awaiting if status == 0 && events.is_empty() => Ok(None),
        _ => Err(()),
    }
}

struct Counter {
    build: Build,
    batch: Batch,
    failed: Rc<Cell<bool>>,
    reload: Option<Reload>,
}

#[derive(Default)]
struct Reload {
    ready: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    watcher: Option<RecommendedWatcher>,
}

impl Reload {
    fn watch(&mut self, wake: WakeCallback) -> Option<RecommendedWatcher> {
        let directory = PathBuf::from(std::env::var_os("OXEL_RELOAD_DIRECTORY")?);
        let directory = std::fs::canonicalize(directory).ok()?;
        let marker = directory.join("current");
        let ready = Arc::clone(&self.ready);
        let failed = Arc::clone(&self.failed);
        let on_event = Arc::clone(&wake);
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let changed = match event {
                Ok(event) => {
                    event.need_rescan()
                        || (!matches!(event.kind, EventKind::Access(_))
                            && event.paths.contains(&marker))
                }
                Err(_) => {
                    failed.store(true, Ordering::Release);
                    true
                }
            };
            if changed {
                ready.store(true, Ordering::Release);
                on_event();
            }
        })
        .ok()?;
        watcher
            .watch(&directory, RecursiveMode::NonRecursive)
            .ok()?;
        // Reconcile a publication that arrived before the window attached its watcher.
        self.ready.store(true, Ordering::Release);
        wake();
        Some(watcher)
    }
}

impl Counter {
    fn update(&mut self, events: &[i32]) -> bool {
        if self.failed.get() {
            return false;
        }
        match request_batch(self.build, events) {
            Ok(Some(batch)) => {
                self.batch = batch;
                true
            }
            Ok(None) => false,
            Err(()) => {
                eprintln!("Plico counter rejected a view update");
                self.failed.set(true);
                false
            }
        }
    }
}

impl HostView for Counter {
    fn set_wake_callback(&mut self, wake: WakeCallback) {
        if let Some(reload) = &mut self.reload {
            reload.watcher = reload.watch(wake);
        }
    }

    fn poll_updates(&mut self) -> bool {
        let Some(reload) = &mut self.reload else {
            return false;
        };
        if reload.failed.swap(false, Ordering::AcqRel) {
            reload.watcher = None;
            return true; // Start the fallback timer on the next declaration.
        }
        reload.ready.swap(false, Ordering::AcqRel) && self.update(&[])
    }

    fn show(&mut self, ui: &mut UiContext<'_>) {
        let events = self.batch.show(ui);
        let polling = self
            .reload
            .as_ref()
            .is_some_and(|reload| reload.watcher.is_none());
        if (polling || !events.is_empty()) && self.update(&events) {
            ui.request_rebuild();
        }
        if polling && !self.failed.get() {
            ui.request_rebuild_at(ui.now() + Duration::from_millis(120));
        }
    }
}
