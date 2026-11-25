//! Remove background: a segmentation model finds the subject of an image and every other pixel
//! turns transparent. Desktop only.
//!
//! The model is BiRefNet (lite, general use) by Zheng Peng et al., "Bilateral Reference for
//! High-Resolution Dichotomous Image Segmentation" (CAAI AIR 2024), MIT licence, as its authors
//! release it in ONNX form (<https://github.com/ZhengPeng7/BiRefNet/releases/tag/v1>). It is
//! downloaded once (214 MiB, SHA-256 pinned) into the suite's shared model folder through
//! craft-ai's downloader, and only after the user allowed it in the app: as with fonts, no command
//! turns downloading on. It runs on the CPU with tract (pure Rust) on a thread of its own: about
//! 15 seconds an image on a desktop, after a first load of about 8.
//!
//! `image.removeBackground` starts the job and reports its progress when called again; once the
//! job is done, the next call puts the cut-out images in their paints as one undo step.

use std::sync::{Mutex, PoisonError};

use figcraft_doc::NodeId;

/// Where the job is.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Getting the model: bytes so far and in all.
    Downloading { done: u64, total: u64 },
    /// Reading the model into memory (once per run of the app).
    Loading,
    /// Images done so far and in all.
    Working { done: usize, total: usize },
}

/// One image paint to cut out: the layer, the paint's index in its fills and the image it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub node: NodeId,
    pub fill: usize,
    pub image: String,
}

/// A finished cut-out: what it replaces and the new image (PNG).
#[derive(Clone, Debug)]
pub struct Cut {
    pub target: Target,
    pub png: Vec<u8>,
}

/// The process's one job, for the document `doc` (its uid).
#[derive(Clone, Debug)]
pub enum Job {
    Running { doc: u64, phase: Phase },
    Done { doc: u64, cuts: Vec<Cut> },
    Failed { doc: u64, error: String },
}

impl Job {
    pub fn doc(&self) -> u64 {
        match self {
            Job::Running { doc, .. } | Job::Done { doc, .. } | Job::Failed { doc, .. } => *doc,
        }
    }
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

fn job() -> std::sync::MutexGuard<'static, Option<Job>> {
    JOB.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The job as it is now (for the app's progress window), without changing it.
pub fn status() -> Option<Job> {
    job().clone()
}

/// Take a finished or failed job of document `doc`, leaving none.
pub fn take_finished(doc: u64) -> Option<Job> {
    let mut slot = job();
    match slot.as_ref() {
        Some(Job::Done { doc: d, .. } | Job::Failed { doc: d, .. }) if *d == doc => slot.take(),
        _ => None,
    }
}

pub(crate) fn set(state: Job) {
    *job() = Some(state);
}

/// The model's size (MiB), licence and source, for the window that asks to download it.
pub const MODEL_SIZE_MB: u64 = 214;
pub const MODEL_LICENSE: &str = "MIT";
pub const MODEL_SOURCE: &str = "https://github.com/ZhengPeng7/BiRefNet";

#[cfg(not(target_arch = "wasm32"))]
pub use native::{allow_download, ready, start};

/// On the web there is no model to run: the command says so.
#[cfg(target_arch = "wasm32")]
pub fn ready() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
pub fn allow_download() -> Result<(), String> {
    Err("Remove background works in the desktop app".into())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, PoisonError};

    use craft_ai::{Arch, ModelEntry, Quality};
    use figcraft_render::Pixmap;
    use tract_onnx::prelude::{DatumExt, Framework, InferenceModelExt, IntoRunnable, Tensor, TractResult, TypedRunnableModel, tvec};

    use super::{Cut, Job, MODEL_LICENSE, MODEL_SIZE_MB, Phase, Target, set};

    /// The model, as craft-ai's downloader takes it. `arch` and the tokenizer belong to its
    /// language models and go unused here.
    static MODEL: ModelEntry = ModelEntry {
        id: "birefnet-lite-general",
        name: "BiRefNet lite (general use)",
        url: "https://github.com/ZhengPeng7/BiRefNet/releases/download/v1/BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
        file: "BiRefNet-general-bb_swin_v1_tiny-epoch_232.onnx",
        tokenizer_url: "",
        sha256: Some("5600024376f572a557870a5eb0afb1e5961636bef4e1e22132025467d0f03333"),
        size_mb: MODEL_SIZE_MB,
        min_ram_gb: 4.0,
        rec_ram_gb: 8.0,
        license: MODEL_LICENSE,
        quality: Quality::Small,
        arch: Arch::Qwen2,
        langs: "",
    };

    /// The side of the square the model reads.
    const SIDE: usize = 1024;
    /// The colour normalisation the model was trained with (ImageNet's mean and deviation).
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];

    fn models_dir() -> Option<PathBuf> {
        // This crate's tests never touch the user's models.
        if cfg!(test) {
            return Some(std::env::temp_dir().join(format!("figcraft-models-tests-{}", std::process::id())));
        }
        craft_ai::probe::models_dir()
    }

    /// The file that records the user's go-ahead to download the model.
    fn consent_file() -> Option<PathBuf> {
        Some(models_dir()?.join("figcraft-remove-background.allowed"))
    }

    /// Whether the model may be used: it is on disk already, or the user allowed downloading it.
    pub fn ready() -> bool {
        models_dir().is_some_and(|d| craft_ai::fetch::is_downloaded(&MODEL, &d)) || consent_file().is_some_and(|f| f.is_file())
    }

    /// Allow downloading the model. Only from the user's own action in the app's window, never
    /// from a command: agents can't.
    pub fn allow_download() -> Result<(), String> {
        let file = consent_file().ok_or_else(|| "this system has no folder for models".to_string())?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&file, b"The user allowed FigCraft to download the background removal model.\n").map_err(|e| e.to_string())
    }

    /// The model, loaded once.
    static PLAN: Mutex<Option<Arc<TypedRunnableModel>>> = Mutex::new(None);

    fn plan(path: &std::path::Path) -> TractResult<Arc<TypedRunnableModel>> {
        let mut slot = PLAN.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(p) = slot.as_ref() {
            return Ok(p.clone());
        }
        let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        tract_linalg::multithread::set_default_executor(tract_linalg::multithread::Executor::multithread(threads));
        let model =
            tract_onnx::onnx().model_for_path(path)?.with_input_fact(0, f32::fact([1, 3, SIDE, SIDE]).into())?.into_optimized()?.into_runnable()?;
        *slot = Some(model.clone());
        Ok(model)
    }

    /// Start cutting out `images` (each target with its image's encoded bytes) for document `doc`
    /// on a thread of its own. The caller checked there is no job running.
    pub fn start(doc: u64, images: Vec<(Target, Arc<Vec<u8>>)>) {
        set(Job::Running { doc, phase: Phase::Downloading { done: 0, total: 0 } });
        std::thread::spawn(move || {
            let work = std::panic::AssertUnwindSafe(|| run(doc, &images));
            let state = match std::panic::catch_unwind(work) {
                Ok(Ok(cuts)) => Job::Done { doc, cuts },
                Ok(Err(error)) => Job::Failed { doc, error },
                Err(_) => Job::Failed { doc, error: "the background removal stopped unexpectedly".into() },
            };
            set(state);
        });
    }

    fn run(doc: u64, images: &[(Target, Arc<Vec<u8>>)]) -> Result<Vec<Cut>, String> {
        let dir = models_dir().ok_or("this system has no folder for models")?;
        let progress = |e: &craft_ai::fetch::Event| {
            if let craft_ai::fetch::Event::Progress { done, total, .. } = e {
                set(Job::Running { doc, phase: Phase::Downloading { done: *done, total: *total } });
            }
        };
        let path = craft_ai::fetch::download(&MODEL, &dir, progress, None).map_err(|e| format!("the model couldn't be downloaded: {e}"))?;
        set(Job::Running { doc, phase: Phase::Loading });
        let plan = plan(&path).map_err(|e| format!("the model couldn't be loaded: {e}"))?;
        let mut cuts = Vec::new();
        for (i, (target, bytes)) in images.iter().enumerate() {
            set(Job::Running { doc, phase: Phase::Working { done: i, total: images.len() } });
            let pix = figcraft_render::images::decode(bytes).ok_or("an image couldn't be read")?;
            let mask = mask(&plan, &pix).map_err(|e| format!("the model failed: {e}"))?;
            let png = figcraft_render::encode_png(&apply_mask(&pix, &mask))?;
            cuts.push(Cut { target: target.clone(), png });
        }
        Ok(cuts)
    }

    /// The colour of `pix` near (`x`, `y`) (pixels, centres at .5), not premultiplied, 0–1.
    fn sample(pix: &Pixmap, x: f32, y: f32) -> [f32; 3] {
        let (w, h) = (pix.width() as usize, pix.height() as usize);
        let at = |x: usize, y: usize| {
            let c = pix.data().get((y * w + x) * 4..(y * w + x) * 4 + 4).unwrap_or(&[0, 0, 0, 0]);
            let a = f32::from(c[3]);
            if a == 0.0 { [1.0, 1.0, 1.0] } else { [f32::from(c[0]) / a, f32::from(c[1]) / a, f32::from(c[2]) / a] }
        };
        let fx = (x - 0.5).clamp(0.0, (w - 1) as f32);
        let fy = (y - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (fx as usize, fy as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|k| (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty)
    }

    /// How much each of the model's `SIDE` × `SIDE` cells belongs to the subject (0–1).
    fn mask(plan: &Arc<TypedRunnableModel>, pix: &Pixmap) -> Result<Vec<f32>, String> {
        let (sx, sy) = (pix.width() as f32 / SIDE as f32, pix.height() as f32 / SIDE as f32);
        let input: Tensor = tract_onnx::prelude::tract_ndarray::Array4::<f32>::from_shape_fn((1, 3, SIDE, SIDE), |(_, c, y, x)| {
            let p = sample(pix, (x as f32 + 0.5) * sx, (y as f32 + 0.5) * sy);
            (p[c] - MEAN[c]) / STD[c]
        })
        .into();
        let out = plan.run(tvec!(input.into())).map_err(|e| e.to_string())?;
        let view = out.first().ok_or("no output")?.to_plain_array_view::<f32>().map_err(|e| e.to_string())?;
        // The model gives scores; the subject is where their sigmoid is near 1.
        Ok(view.iter().map(|v| 1.0 / (1.0 + (-v).exp())).collect())
    }

    /// `pix` with each pixel kept as much as the mask (read between its cells) says.
    fn apply_mask(pix: &Pixmap, mask: &[f32]) -> Pixmap {
        let (w, h) = (pix.width() as usize, pix.height() as usize);
        let at = |x: usize, y: usize| mask.get(y * SIDE + x).copied().unwrap_or(0.0);
        let mut out = pix.clone();
        for (i, px) in out.data_mut().chunks_exact_mut(4).enumerate() {
            let fx = (((i % w) as f32 + 0.5) * SIDE as f32 / w as f32 - 0.5).clamp(0.0, (SIDE - 1) as f32);
            let fy = (((i / w) as f32 + 0.5) * SIDE as f32 / h as f32 - 0.5).clamp(0.0, (SIDE - 1) as f32);
            let (x0, y0) = (fx as usize, fy as usize);
            let (x1, y1) = ((x0 + 1).min(SIDE - 1), (y0 + 1).min(SIDE - 1));
            let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
            let m = (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty) + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty;
            // Premultiplied: colour and alpha scale together.
            for c in px.iter_mut() {
                *c = (f32::from(*c) * m.clamp(0.0, 1.0)).round() as u8;
            }
        }
        out
    }

    #[cfg(test)]
    pub(crate) fn apply_mask_for_tests(pix: &Pixmap, mask: &[f32]) -> Pixmap {
        apply_mask(pix, mask)
    }

    #[cfg(test)]
    pub(crate) const SIDE_FOR_TESTS: usize = SIDE;
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) use native::{SIDE_FOR_TESTS, apply_mask_for_tests};
