//! Image paints: adjustment presets, built in and saved in the file, Crop image, and Vectorize
//! image (an image traced into filled shapes, [`figcraft_pathops::trace`]).

use figcraft_color::Color;
use figcraft_color::{ImageFilters, ImageScaleMode, Paint, PaintKind};
use figcraft_doc::{Document, ImagePreset, Node, NodeId, NodeKind, VectorNetwork};
use figcraft_geom::Affine;
use serde_json::{Value, json};

use super::{CommandSpec, ok};
use crate::params::{boolean, f64_of, get, req_index, req_string, req_targets, string, targets};
use crate::{EngineError, Result, Session, bad, spec};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        spec!(
            "image.adjust",
            "Adjust image",
            "{ids?, target? (fill|stroke, default fill), index? (default every image paint), preset? (vivid|muted|warm|cool|blackAndWhite|fade|reset, or the name of a preset saved in the file), filters? ({exposure, contrast, saturation, temperature, tint, highlights, shadows}: each -1..1, applied over the preset)}  sets the adjustments of the layers' image paints; returns {changed}",
            true,
            adjust
        ),
        spec!(
            "image.replace",
            "Replace image",
            "{ids?, target? (fill|stroke, default fill), index? (default each layer's top image paint), data (base64) | path}  puts the image in the paint, keeping its fill mode, crop, position and adjustments; a paint at `index` that isn't an image becomes one, filling the layer; returns {image, width, height, changed}",
            true,
            replace
        ),
        spec!(
            "image.savePreset",
            "Save image preset",
            "{name, filters? (default the adjustments of the first image paint of `ids`/the selection), ids?, target?, index?}  saved in the file; a preset with the same name is replaced",
            true,
            save_preset
        ),
        spec!("image.deletePreset", "Delete image preset", "{name}", true, |s, p| {
            let name = req_string("image.deletePreset", p, "name")?;
            let doc = s.doc_mut()?;
            let before = doc.image_presets.len();
            doc.image_presets.retain(|x| x.name != name);
            if doc.image_presets.len() == before {
                return Err(bad("image.deletePreset", format!("no preset named `{name}`")));
            }
            ok()
        }),
        spec!(
            "image.crop",
            "Crop image",
            "{ids? (one layer), index? (an image fill; default its top visible one)} → {id, index}: the image fill switches to Crop, keeping the picture where it was, and the canvas enters crop mode on it",
            true,
            crop
        ),
        spec!(
            "image.vectorize",
            "Vectorize image",
            "{ids?, colors? (2–64, default 16), detail? (0–1, default 0.5: higher keeps smaller shapes), smoothness? (0–1, default 0.5: higher lets outlines stray further from the pixels for smoother curves), mode? (color|mono, default color; mono makes one shape of the pixels darker than `threshold`, for logos and line art), threshold? (0–1 of white, default 0.5), original? (hide|remove|keep, default hide: what becomes of the traced layer), wait? (bool), cancel? (bool)}  traces each layer with an image fill as it shows (crop, fill mode and adjustments included) into filled vector shapes, one per colour, stacked largest first in a group placed just above it. Pixels more than half transparent are left out, so an image whose background was removed traces without it. The tracing runs on its own thread: the first call returns {state: working, done (0–1), images}; call again (ids optional) for its progress, and once it is done the next call in the same file puts the shapes in as one undo step and returns {state: vectorized, ids (the new groups), shapes}. `cancel: true` stops a running job (the next call returns {state: cancelled}); `wait: true` traces in this call and returns the shapes at once (the browser always does)",
            true,
            vectorize
        ),
        spec!(
            "image.vectorizePreview",
            "Vectorize image preview",
            "{ids? (the first layer with an image fill is shown), colors?, detail?, smoothness?, mode?, threshold?}  how `image.vectorize` would trace the layer with those options, at most 360 pixels on its longer side: {png (base64, the shapes over transparency), width, height, shapes}. Changes nothing. From the menu (Object ▸ Vectorize image…) it opens the options window, where the preview follows the options",
            false,
            vectorize_preview
        ),
        spec!("image.presets", "Image presets", "{} → {builtIn: [{id, label, filters}], saved: [{name, filters}]}", false, |s, _| {
            let built_in: Vec<Value> = ImageFilters::PRESETS.iter().map(|(id, label, f)| json!({"id": id, "label": label, "filters": f})).collect();
            Ok(json!({"builtIn": built_in, "saved": s.doc()?.image_presets}))
        }),
    ]
}

/// The longest name a saved preset may have.
const MAX_NAME: usize = 100;
/// The most presets a file keeps.
const MAX_PRESETS: usize = 500;

/// Whether `p` asks for strokes (`target: stroke`) rather than fills.
fn strokes(cmd: &str, p: &Value) -> Result<bool> {
    match string(p, "target").as_deref() {
        None | Some("fill") | Some("fills") => Ok(false),
        Some("stroke") | Some("strokes") => Ok(true),
        Some(_) => Err(bad(cmd, "target: fill|stroke")),
    }
}

/// The preset `name`: one saved in the file under that exact name, else a built-in one.
pub fn find_preset(doc: &Document, name: &str) -> Option<ImageFilters> {
    doc.image_presets.iter().find(|x| x.name == name).map(|x| x.filters).or_else(|| ImageFilters::preset(name))
}

/// `base` with the adjustments `v` names (an object of -1..1 numbers) set.
fn with_values(cmd: &str, base: ImageFilters, v: &Value) -> Result<ImageFilters> {
    let Value::Object(m) = v else { return Err(bad(cmd, "filters: an object of adjustments")) };
    let mut f = base;
    for (k, val) in m {
        let x = f64_of(val).ok_or_else(|| bad(cmd, format!("filters.{k}: a number")))? as f32;
        let slot = match k.as_str() {
            "exposure" => &mut f.exposure,
            "contrast" => &mut f.contrast,
            "saturation" => &mut f.saturation,
            "temperature" => &mut f.temperature,
            "tint" => &mut f.tint,
            "highlights" => &mut f.highlights,
            "shadows" => &mut f.shadows,
            _ => return Err(bad(cmd, format!("unknown adjustment `{k}`"))),
        };
        *slot = x;
    }
    Ok(f.sanitized())
}

fn adjust(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "image.adjust";
    let ids = req_targets(cmd, s, p)?;
    let stroke = strokes(cmd, p)?;
    let index = match get(p, "index") {
        None | Some(Value::Null) => None,
        Some(_) => Some(req_index(cmd, p, "index")?),
    };
    let preset = match string(p, "preset") {
        Some(name) => Some(find_preset(s.doc()?, &name).ok_or_else(|| bad(cmd, format!("no preset named `{name}`")))?),
        None => None,
    };
    let values = get(p, "filters").filter(|v| !v.is_null());
    if preset.is_none() && values.is_none() {
        return Err(bad(cmd, "pass `preset` or `filters`"));
    }
    let doc = s.doc_mut()?;
    let mut changed = 0;
    for id in ids {
        let n = doc.get(id)?;
        if !if stroke { &n.strokes } else { &n.fills }.iter().any(|x| matches!(x.kind, PaintKind::Image(_))) {
            continue;
        }
        let n = doc.get_mut(id)?;
        let list = if stroke { &mut n.strokes } else { &mut n.fills };
        let mut touched = false;
        for (i, paint) in list.iter_mut().enumerate() {
            let PaintKind::Image(img) = &mut paint.kind else { continue };
            if index.is_some_and(|k| k != i) {
                continue;
            }
            let base = preset.unwrap_or(img.filters);
            img.filters = match values {
                Some(v) => with_values(cmd, base, v)?,
                None => base,
            };
            touched = true;
            changed += 1;
        }
        if touched {
            // Like any paint edit, the layer leaves its paint style.
            if stroke {
                n.styles.stroke = None;
            } else {
                n.styles.fill = None;
            }
        }
    }
    if changed == 0 {
        return Err(bad(cmd, if index.is_some() { "no image paint at that index" } else { "the layers have no image paints" }));
    }
    Ok(json!({"changed": changed}))
}

fn replace(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "image.replace";
    let ids = req_targets(cmd, s, p)?;
    let stroke = strokes(cmd, p)?;
    let index = match get(p, "index") {
        None | Some(Value::Null) => None,
        Some(_) => Some(req_index(cmd, p, "index")?),
    };
    let bytes = super::create::image_bytes(cmd, p)?;
    let doc = s.doc_mut()?;
    let (image, width, height) = crate::cmd::create::add_readable_image(doc, bytes).map_err(|e| bad(cmd, e))?;
    let mut changed = 0;
    for id in ids {
        let n = doc.get_mut(id)?;
        let list = if stroke { &mut n.strokes } else { &mut n.fills };
        let at = index.or_else(|| list.iter().rposition(|x| matches!(x.kind, PaintKind::Image(_))));
        let Some(paint) = at.and_then(|i| list.get_mut(i)) else { continue };
        match &mut paint.kind {
            PaintKind::Image(img) => img.image.clone_from(&image),
            kind => *kind = Paint::image(image.clone(), ImageScaleMode::Fill).kind,
        }
        // Like any paint edit, the layer leaves its paint style.
        if stroke {
            n.styles.stroke = None;
        } else {
            n.styles.fill = None;
        }
        changed += 1;
    }
    if changed == 0 {
        return Err(bad(cmd, if index.is_some() { "no paint at that index" } else { "the layers have no image paints" }));
    }
    Ok(json!({"image": image, "width": width, "height": height, "changed": changed}))
}

/// The index of a layer's top visible image fill among the fills it shows (fills paint bottom to
/// top): a text's characters' shared list, so none while they are coloured differently.
pub fn top_image_fill(n: &Node) -> Option<usize> {
    crate::cmd::lists::character_fills(n)?.iter().rposition(|f| f.visible && matches!(f.kind, PaintKind::Image(_)))
}

fn crop(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "image.crop";
    let ids = req_targets(cmd, s, p)?;
    // A selection with nothing to crop is refused as unavailable, not as bad input.
    let refuse = |why: &str| EngineError::Disabled(cmd.into(), why.into());
    let [id] = ids[..] else { return Err(refuse("select one layer to crop its image")) };
    let doc = s.doc()?;
    let n = doc.get(id)?;
    // A text's fills are its characters' (the Design panel's rows): the crop edits that list, and
    // characters coloured differently have no row to crop ("Click + to replace mixed content").
    let shown =
        crate::cmd::lists::character_fills(n).ok_or_else(|| refuse("the characters have different fills, so there is no one fill to crop"))?;
    let index = match get(p, "index") {
        None | Some(Value::Null) => top_image_fill(n).ok_or_else(|| refuse("the layer has no image fill to crop"))?,
        Some(_) => req_index(cmd, p, "index")?,
    };
    let mut paint = shown.get(index).cloned().ok_or_else(|| bad(cmd, "no fill at that index"))?;
    let PaintKind::Image(img) = &mut paint.kind else { return Err(bad(cmd, "that fill is not an image")) };
    // An image already in Crop mode only enters crop mode: no edit, no undo step.
    if img.scale_mode != ImageScaleMode::Crop {
        let data = doc.images.get(&img.image).ok_or_else(|| bad(cmd, "the image's data is missing"))?;
        img.crop = Some(img.placement_crop(f64::from(data.width), f64::from(data.height), n.width, n.height));
        img.scale_mode = ImageScaleMode::Crop;
        let n = s.doc_mut()?.get_mut(id)?;
        // The characters' shared list becomes the layer's, so the crop shows on every one of them.
        crate::cmd::lists::take_character_fills(n);
        if let Some(f) = n.fills.get_mut(index) {
            *f = paint;
        }
        // Like any paint edit, the layer leaves its paint style.
        n.styles.fill = None;
    }
    s.request_ui(cmd, json!({"id": id, "index": index}));
    Ok(json!({"id": id, "index": index}))
}

fn save_preset(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "image.savePreset";
    let name: String = req_string(cmd, p, "name")?.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME).collect();
    if name.is_empty() {
        return Err(bad(cmd, "`name` is empty"));
    }
    let filters = match get(p, "filters").filter(|v| !v.is_null()) {
        Some(v) => with_values(cmd, ImageFilters::NONE, v)?,
        None => {
            let stroke = strokes(cmd, p)?;
            let index = match get(p, "index") {
                None | Some(Value::Null) => None,
                Some(_) => Some(req_index(cmd, p, "index")?),
            };
            let doc = s.doc()?;
            targets(s, p)
                .iter()
                .filter_map(|id| doc.node(*id))
                .flat_map(|n| if stroke { n.strokes.iter() } else { n.fills.iter() }.enumerate())
                .find_map(|(i, x)| match &x.kind {
                    PaintKind::Image(img) if index.is_none_or(|k| k == i) => Some(img.filters),
                    _ => None,
                })
                .ok_or_else(|| bad(cmd, "pass `filters`, or select a layer with an image paint"))?
        }
    };
    let doc = s.doc_mut()?;
    let preset = ImagePreset { name: name.clone(), filters };
    match doc.image_presets.iter().position(|x| x.name == name) {
        Some(i) => {
            if let Some(x) = doc.image_presets.get_mut(i) {
                *x = preset;
            }
        }
        None if doc.image_presets.len() >= MAX_PRESETS => return Err(bad(cmd, format!("a file keeps at most {MAX_PRESETS} presets"))),
        None => doc.image_presets.push(preset),
    }
    Ok(json!({"name": name, "filters": filters}))
}

/// The longest side, in pixels, a layer is traced at.
const TRACE_SIDE: f64 = 1024.0;

fn vectorize(s: &mut Session, p: &Value) -> Result<Value> {
    use crate::vectorize::{self as job, Job};
    let cmd = "image.vectorize";
    let refuse = |why: &str| EngineError::Disabled(cmd.into(), why.into());
    let uid = s.ds()?.uid;
    if boolean(p, "cancel") == Some(true) {
        return if job::cancel() { Ok(json!({"state": "cancelling"})) } else { Err(refuse("nothing is being vectorized")) };
    }
    // The browser has no threads to trace on; `wait` asks for the shapes in this call, leaving
    // any job alone.
    let wait = cfg!(target_arch = "wasm32") || boolean(p, "wait") == Some(true);
    // A job of this file that ended: its shapes go in, or it says why not.
    match if wait { None } else { job::take_finished(uid) } {
        Some(Job::Done { traced, .. }) => return put_traced(s, traced),
        Some(Job::Cancelled { .. }) => {
            let state = json!({"state": "cancelled"});
            s.request_ui(cmd, state.clone());
            return Ok(state);
        }
        Some(Job::Failed { error, .. }) => return Err(bad(cmd, error)),
        _ => {}
    }
    if let Some(Job::Running { doc, done, images }) = job::status().filter(|_| !wait) {
        if doc != uid {
            return Err(refuse("an image is being vectorized in another file; try again when it is done"));
        }
        let state = json!({"state": "working", "done": done, "images": images});
        s.request_ui(cmd, state.clone());
        return Ok(state);
    }
    let ids = req_targets(cmd, s, p)?;
    let settings = crate::vectorize::Settings::from_params(cmd, p)?;
    let doc = s.doc()?;
    let mut sources = Vec::new();
    for id in ids {
        if top_image_fill(doc.get(id)?).is_none() {
            continue;
        }
        if crate::components::owning_instance(doc, id).is_some() {
            return Err(refuse("a layer inside an instance can't be vectorized; detach the instance first"));
        }
        sources.push(source(doc, id, &settings, TRACE_SIDE).ok_or_else(|| refuse("the image couldn't be traced"))?);
    }
    if sources.is_empty() {
        return Err(refuse("select a layer with an image fill"));
    }
    if wait {
        let traced = job::run(&sources, &mut |_| true).map_err(|_| refuse("the image couldn't be traced"))?;
        return put_traced(s, traced);
    }
    let images = sources.len();
    #[cfg(not(target_arch = "wasm32"))]
    job::start(uid, sources);
    let state = json!({"state": "working", "done": 0.0, "images": images});
    s.request_ui(cmd, state.clone());
    Ok(state)
}

/// Layer `id` as it shows (crop, fill mode and adjustments included), ready to trace with its
/// longer side at most `side` pixels (a preview's is smaller).
pub(crate) fn source(doc: &Document, id: NodeId, settings: &crate::vectorize::Settings, side: f64) -> Option<crate::vectorize::Source> {
    let bounds = doc.world_bounds(id)?;
    // At most the image's own resolution: scaled up, its edges would blur into bands of colours
    // in between.
    let native = doc
        .node(id)
        .and_then(|n| top_image_fill(n).and_then(|i| n.fills.get(i)))
        .and_then(|f| if let PaintKind::Image(img) = &f.kind { doc.images.get(&img.image) } else { None })
        .map_or(f64::INFINITY, |d| (f64::from(d.width) / bounds.width().max(1.0)).max(f64::from(d.height) / bounds.height().max(1.0)));
    let longest = bounds.width().max(bounds.height()).max(1.0);
    let full = (TRACE_SIDE / longest).min(native.max(1.0));
    let scale = (side / longest).min(full);
    let pix = figcraft_render::render_area_at(doc, &[id], bounds, scale, 0.0)?;
    let (width, height) = (pix.width() as usize, pix.height() as usize);
    // The renderer's pixels are premultiplied; the tracer reads plain colours.
    let rgba: Vec<u8> = pix
        .data()
        .chunks_exact(4)
        .flat_map(|c| {
            let a = u16::from(c[3]);
            let plain = |v: u8| (u16::from(v) * 255 + a / 2).checked_div(a).map_or(0, |x| u8::try_from(x).unwrap_or(u8::MAX));
            [plain(c[0]), plain(c[1]), plain(c[2]), c[3]]
        })
        .collect();
    let options = settings.options(width * height, scale / full.max(1e-9));
    Some(crate::vectorize::Source { node: id, bounds, scale, rgba, width, height, options, original: settings.original })
}

/// Put traced layers in: each as a group of vectors just above its layer, which is hidden (one
/// undo step for them all). A layer deleted meanwhile is skipped.
fn put_traced(s: &mut Session, traced: Vec<crate::vectorize::Traced>) -> Result<Value> {
    let doc = s.doc_mut()?;
    let mut groups = Vec::new();
    let mut shapes = 0;
    for t in traced {
        if let Some((group, made)) = put_one(doc, &t) {
            groups.push(group);
            shapes += made;
        }
    }
    if !groups.is_empty() {
        s.set_selection(groups.clone());
    }
    let state = json!({"state": "vectorized", "ids": groups, "shapes": shapes});
    s.request_ui("image.vectorize", state.clone());
    Ok(state)
}

/// One traced layer as a group of vectors just above it, the layer hidden: the group's id and how
/// many vectors it holds.
fn put_one(doc: &mut Document, t: &crate::vectorize::Traced) -> Option<(NodeId, usize)> {
    let n = doc.get(t.node).ok()?;
    let parent = n.parent?;
    let name = format!("{} vector", n.name);
    let index = doc.index_in_parent(t.node).map(|i| i + 1);
    // The group sits where the layer's pixels were; its vectors are in page units from there.
    let gid = doc.alloc_id();
    let mut group = Node::new(gid, NodeKind::Group, name, t.bounds.width(), t.bounds.height());
    group.transform = doc.world_transform(parent).inverse() * Affine::translate((t.bounds.x0, t.bounds.y0));
    doc.insert(group, parent, index).ok()?;
    let to_group = Affine::scale(1.0 / t.scale);
    let mut made = 0;
    for layer in &t.layers {
        let path = to_group * layer.path.clone();
        let mut net = VectorNetwork::from_path(&path);
        let Some(bb) = net.bounds() else { continue };
        net.translate(-bb.x0, -bb.y0);
        let [r, g, b] = layer.color;
        let vid = doc.alloc_id();
        let mut v = Node::new(vid, NodeKind::Vector { network: net }, format!("#{r:02X}{g:02X}{b:02X}"), bb.width(), bb.height());
        v.transform = Affine::translate((bb.x0, bb.y0));
        v.fills = vec![Paint::solid(Color::from_rgba8(r, g, b, 255))];
        doc.insert(v, gid, None).ok()?;
        made += 1;
    }
    let _ = doc.fit_to_children(gid);
    match t.original {
        crate::vectorize::Original::Hide => {
            if let Ok(n) = doc.get_mut(t.node) {
                n.visible = false;
            }
        }
        crate::vectorize::Original::Remove => {
            let _ = doc.remove(t.node);
        }
        crate::vectorize::Original::Keep => {}
    }
    Some((gid, made))
}

fn vectorize_preview(s: &mut Session, p: &Value) -> Result<Value> {
    use base64::Engine as _;
    let cmd = "image.vectorizePreview";
    let settings = crate::vectorize::Settings::from_params(cmd, p)?;
    let ids = req_targets(cmd, s, p)?;
    let doc = s.doc()?;
    let Some(id) = ids.iter().copied().find(|id| doc.node(*id).and_then(top_image_fill).is_some()) else {
        return Err(EngineError::Disabled(cmd.into(), "select a layer with an image fill".into()));
    };
    let preview = crate::vectorize::preview(doc, id, &settings).ok_or_else(|| bad(cmd, "the image couldn't be traced"))?;
    let png = figcraft_render::encode_png(&preview.image).map_err(|e| bad(cmd, e))?;
    let out = json!({
        "png": base64::engine::general_purpose::STANDARD.encode(png),
        "width": preview.image.width(),
        "height": preview.image.height(),
        "shapes": preview.shapes,
    });
    s.request_ui("dialog.vectorize", json!({"ids": ids}));
    Ok(out)
}
