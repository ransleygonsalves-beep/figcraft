//! Vectorize image's tracing job (#2497). The layers are rendered on the caller's thread (quick),
//! then traced ([`figcraft_pathops::trace`]) on a thread of their own, so a large image neither
//! freezes the canvas nor holds up the app; the job reports how far it got and can be cancelled.
//! `image.vectorize` starts it, reports its progress when called again, and once it is done the
//! next call in the same file puts the traced shapes in as one undo step. The browser has no
//! threads: there, and with `wait`, the tracing runs on the caller's thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use figcraft_doc::{Document, NodeId};
use figcraft_geom::Rect;
use figcraft_pathops::trace::{MAX_COLORS, TraceOptions, TraceStop, TracedLayer, trace_with};
use serde_json::Value;

use crate::params::{num_or, string};
use crate::{Result, bad};

/// What becomes of the traced layer once its shapes are in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Original {
    /// Hidden, so it can be compared or shown again.
    #[default]
    Hide,
    /// Deleted.
    Remove,
    /// Left as it is, under the shapes.
    Keep,
}

impl Original {
    pub const ALL: [Original; 3] = [Original::Hide, Original::Remove, Original::Keep];
    pub fn id(self) -> &'static str {
        match self {
            Original::Hide => "hide",
            Original::Remove => "remove",
            Original::Keep => "keep",
        }
    }
}

/// How Vectorize image traces: the command's options, shared by the run and its preview.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Colours to keep, 2–64 (ignored in one colour).
    pub colors: usize,
    /// 0–1: higher keeps smaller shapes.
    pub detail: f64,
    /// 0–1: higher lets outlines stray further from the pixels for smoother curves.
    pub smoothness: f64,
    /// One colour: the pixels darker than this share of white (0–1) make one shape.
    pub mono: Option<f32>,
    pub original: Original,
}

impl Default for Settings {
    fn default() -> Self {
        Self { colors: 16, detail: 0.5, smoothness: 0.5, mono: None, original: Original::Hide }
    }
}

impl Settings {
    /// From a command's `colors`, `detail`, `smoothness`, `mode` (`color` | `mono`), `threshold`
    /// and `original` (`hide` | `remove` | `keep`); each one left out takes its default.
    pub fn from_params(cmd: &str, p: &Value) -> Result<Self> {
        let d = Self::default();
        let mono = match string(p, "mode").as_deref() {
            None | Some("color") | Some("colour") => None,
            Some("mono") => Some(num_or(p, "threshold", 0.5).clamp(0.0, 1.0) as f32),
            Some(_) => return Err(bad(cmd, "mode: color|mono")),
        };
        let original = match string(p, "original") {
            None => d.original,
            Some(o) => Original::ALL.into_iter().find(|x| x.id() == o).ok_or_else(|| bad(cmd, "original: hide|remove|keep"))?,
        };
        Ok(Self {
            colors: num_or(p, "colors", d.colors as f64).round().clamp(2.0, MAX_COLORS as f64) as usize,
            detail: num_or(p, "detail", d.detail).clamp(0.0, 1.0),
            smoothness: num_or(p, "smoothness", d.smoothness).clamp(0.0, 1.0),
            mono,
            original,
        })
    }

    /// The tracer's options for an image of `pixels` pixels traced at `ratio` of the full
    /// resolution (a preview's outlines may stray as far, measured on the layer).
    pub fn options(&self, pixels: usize, ratio: f64) -> TraceOptions {
        let speckle = 2 + ((1.0 - self.detail).powi(2) * pixels as f64 * 0.0004) as usize;
        let tolerance = ((0.4 + self.smoothness * 1.6) * ratio.clamp(0.0, 1.0)).max(0.1);
        TraceOptions { colors: self.colors, speckle, tolerance, mono: self.mono }
    }
}

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
    pub original: Original,
}

/// One layer traced: its shapes in its pixels' units, bottom to top.
#[derive(Clone, Debug)]
pub struct Traced {
    pub node: NodeId,
    pub bounds: Rect,
    pub scale: f64,
    pub layers: Vec<TracedLayer>,
    pub original: Original,
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
pub fn run(sources: &[Source], progress: &mut dyn FnMut(f32) -> bool) -> std::result::Result<Vec<Traced>, TraceStop> {
    let n = sources.len().max(1) as f32;
    let mut out = Vec::with_capacity(sources.len());
    for (i, s) in sources.iter().enumerate() {
        let layers = trace_with(&s.rgba, s.width, s.height, &s.options, &mut |f| progress((i as f32 + f) / n))?;
        out.push(Traced { node: s.node, bounds: s.bounds, scale: s.scale, layers, original: s.original });
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

/// The longest side, in pixels, of a preview.
pub const PREVIEW_SIDE: f64 = 360.0;

/// A quick look at how a layer traces: its shapes drawn over a transparent picture, and how many
/// there are.
#[derive(Clone, Debug)]
pub struct Preview {
    pub image: figcraft_render::Pixmap,
    pub shapes: usize,
}

/// Trace layer `id` as it shows with `settings`, its longer side at most [`PREVIEW_SIDE`] pixels,
/// on this thread (a preview this size takes a moment). `None` when it has no image to trace.
pub fn preview(doc: &Document, id: NodeId, settings: &Settings) -> Option<Preview> {
    let source = crate::cmd::image::source(doc, id, settings, PREVIEW_SIDE)?;
    let traced = run(std::slice::from_ref(&source), &mut |_| true).ok()?;
    let layers = &traced.first()?.layers;
    let shapes: Vec<(&figcraft_geom::BezPath, [u8; 3])> = layers.iter().map(|l| (&l.path, l.color)).collect();
    let image = figcraft_render::fill_paths(u32::try_from(source.width).ok()?, u32::try_from(source.height).ok()?, &shapes)?;
    Some(Preview { image, shapes: layers.len() })
}
