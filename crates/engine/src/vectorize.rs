//! Vectorize image's tracing job (#2497). The layers are rendered on the caller's thread (quick),
//! then traced ([`figcraft_pathops::trace`]) on a thread of their own, so a large image neither
//! freezes the canvas nor holds up the app; the job reports how far it got and can be cancelled.
//! `image.vectorize` starts it, reports its progress when called again, and once it is done the
//! next call in the same file puts the traced shapes in as one undo step. The browser has no
//! threads: there, and with `wait`, the tracing runs on the caller's thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use figcraft_doc::NodeId;
use figcraft_geom::Rect;
use figcraft_pathops::trace::{TraceOptions, TraceStop, TracedLayer, trace_with};

/// One layer as it shows, ready to trace: its pixels (not premultiplied), where they lie on the
/// page and how many pixels a page unit is.
#[derive(Clone, Debug)]
pub struct Source {
    pub node: NodeId,
    pub bounds: Rect,
    pub scale: f64,
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub options: TraceOptions,
}

/// One layer traced: its shapes in its pixels' units, bottom to top.
#[derive(Clone, Debug)]
pub struct Traced {
    pub node: NodeId,
    pub bounds: Rect,
    pub scale: f64,
    pub layers: Vec<TracedLayer>,
}

/// The process's one tracing job, for the document `doc` (its uid).
#[derive(Clone, Debug)]
pub enum Job {
    /// `done`: how far it got, 0–1, over `images` layers.
    Running {
        doc: u64,
        done: f32,
        images: usize,
    },
    Done {
        doc: u64,
        traced: Vec<Traced>,
    },
    Cancelled {
        doc: u64,
    },
    Failed {
        doc: u64,
        error: String,
    },
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static CANCEL: AtomicBool = AtomicBool::new(false);

fn job() -> std::sync::MutexGuard<'static, Option<Job>> {
    JOB.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn set(state: Job) {
    *job() = Some(state);
}

/// The job as it is now (for the app's progress window), without changing it.
pub fn status() -> Option<Job> {
    job().clone()
}

/// Take a job of document `doc` that ended (done, cancelled or failed), leaving none.
pub fn take_finished(doc: u64) -> Option<Job> {
    let mut slot = job();
    match slot.as_ref() {
        Some(Job::Done { doc: d, .. } | Job::Cancelled { doc: d } | Job::Failed { doc: d, .. }) if *d == doc => slot.take(),
        _ => None,
    }
}

/// Ask the running job to stop; it ends as [`Job::Cancelled`]. False when none is running.
pub fn cancel() -> bool {
    let running = matches!(*job(), Some(Job::Running { .. }));
    if running {
        CANCEL.store(true, Ordering::Relaxed);
    }
    running
}

/// Trace `sources` one after the other, telling `progress` how far it got over all of them (0–1);
/// when it returns false the tracing stops.
pub fn run(sources: &[Source], progress: &mut dyn FnMut(f32) -> bool) -> Result<Vec<Traced>, TraceStop> {
    let n = sources.len().max(1) as f32;
    let mut out = Vec::with_capacity(sources.len());
    for (i, s) in sources.iter().enumerate() {
        let layers = trace_with(&s.rgba, s.width, s.height, &s.options, &mut |f| progress((i as f32 + f) / n))?;
        out.push(Traced { node: s.node, bounds: s.bounds, scale: s.scale, layers });
    }
    Ok(out)
}

/// Start tracing `sources` for document `doc` on a thread of its own. The caller checked there is
/// no job running.
#[cfg(not(target_arch = "wasm32"))]
pub fn start(doc: u64, sources: Vec<Source>) {
    CANCEL.store(false, Ordering::Relaxed);
    let images = sources.len();
    set(Job::Running { doc, done: 0.0, images });
    std::thread::spawn(move || {
        let work = std::panic::AssertUnwindSafe(|| {
            run(&sources, &mut |done| {
                set(Job::Running { doc, done, images });
                !CANCEL.load(Ordering::Relaxed)
            })
        });
        let state = match std::panic::catch_unwind(work) {
            Ok(Ok(traced)) => Job::Done { doc, traced },
            Ok(Err(TraceStop::Cancelled)) => Job::Cancelled { doc },
            Ok(Err(TraceStop::BadInput)) => Job::Failed { doc, error: "the image couldn't be traced".into() },
            Err(_) => Job::Failed { doc, error: "the tracing stopped unexpectedly".into() },
        };
        set(state);
    });
}
