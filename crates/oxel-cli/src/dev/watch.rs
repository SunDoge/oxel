//! Detects edits to manifest sources, including editors replacing a file by rename.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use notify::{
    Event, RecommendedWatcher, RecursiveMode, Watcher,
    event::{AccessKind, AccessMode, EventKind},
};

const CHILD_CHECK: Duration = Duration::from_millis(40);
const DEBOUNCE: Duration = Duration::from_millis(6);
const RESCAN: Duration = Duration::from_secs(1);

pub(super) struct SourceWatch {
    native: Option<Native>,
    len: usize,
}

struct Native {
    _watcher: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    sources: HashMap<PathBuf, Vec<usize>>,
    last_scan: Instant,
}

impl SourceWatch {
    pub(super) fn new(paths: &[PathBuf]) -> Self {
        Self {
            native: Native::new(paths),
            len: paths.len(),
        }
    }

    pub(super) fn changes(&mut self) -> Vec<usize> {
        if let Some(native) = &mut self.native {
            match native.changes() {
                Ok(Some(files)) => return files,
                Ok(None) => {
                    native.last_scan = Instant::now();
                    return (0..self.len).collect();
                }
                Err(_) => self.native = None,
            }
        } else {
            std::thread::sleep(CHILD_CHECK);
        }
        (0..self.len).collect()
    }
}

impl Native {
    fn new(paths: &[PathBuf]) -> Option<Self> {
        let (sender, receiver) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(sender).ok()?;
        let mut sources: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        let mut directories = HashSet::new();
        for (index, path) in paths.iter().enumerate() {
            let parent = canonical_parent(path)?;
            let target = parent.join(path.file_name()?);
            sources.entry(target.clone()).or_default().push(index);
            directories.insert(parent);
            // Watching the link's parent tracks replacement of the link itself.
            // Watching the resolved parent tracks changes to its current target.
            if let Ok(resolved) = fs::canonicalize(path)
                && resolved != target
            {
                directories.insert(resolved.parent()?.to_owned());
                sources.entry(resolved).or_default().push(index);
            }
        }
        for directory in directories {
            watcher
                .watch(&directory, RecursiveMode::NonRecursive)
                .ok()?;
        }
        Some(Self {
            _watcher: watcher,
            receiver,
            sources,
            last_scan: Instant::now(),
        })
    }

    fn changes(&mut self) -> Result<Option<Vec<usize>>, RecvTimeoutError> {
        if self.last_scan.elapsed() >= RESCAN {
            // Reconcile periodically in case the OS misses a change, for example
            // when a source directory is replaced or a symlink is retargeted.
            return Ok(None);
        }
        let timeout = Instant::now() + CHILD_CHECK;
        let mut batch_end = None;
        let mut batch_limit = None;
        let mut changed = Vec::new();
        loop {
            let remaining = batch_end
                .unwrap_or(timeout)
                .saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let event = match self.receiver.recv_timeout(remaining) {
                Ok(Ok(event)) => event,
                Ok(Err(_)) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(RecvTimeoutError::Disconnected);
                }
                Err(RecvTimeoutError::Timeout) => break,
            };
            if event.need_rescan() {
                return Ok(None);
            }
            if let EventKind::Access(kind) = event.kind
                && kind != AccessKind::Close(AccessMode::Write)
            {
                continue;
            }
            for path in event.paths {
                if let Some(indices) = self.sources.get(&path) {
                    changed.extend_from_slice(indices);
                    let now = Instant::now();
                    let limit = *batch_limit.get_or_insert(now + CHILD_CHECK);
                    batch_end = Some((now + DEBOUNCE).min(limit));
                }
            }
        }
        changed.sort_unstable();
        changed.dedup();
        Ok(Some(changed))
    }
}

fn canonical_parent(path: &Path) -> Option<PathBuf> {
    let parent = path.parent()?;
    fs::canonicalize(if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    })
    .ok()
}
