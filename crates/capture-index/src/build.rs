//! `build_index` — the multimodal merge-tree (#62; 1:1 port of `core/indexer.py`).
//!
//! Builds a balanced **binary tree** over the timeline: split the leaf frames at their midpoint
//! recursively, **caption each leaf** with the remote vision model (descent), then **combine
//! children up to a root summary** (conquer), fusing the time-aligned transcript at each combine.
//! Every node keeps its raw artifacts (vision caption, transcript slice) beside the fused summary,
//! so the index is inspectable and re-combinable. The build is checkpointed to `index.json` after
//! every node, so a dropped LAN connection resumes.
//!
//! Design + decisions: docs/specs/indexing.md. Vision runs only at leaves (D2); the transcript is
//! fused into combines (D3) but capped in length so the root combine stays bounded; the full slice
//! is stored raw regardless. The `index.json` shape + the tree logic are LOAD-BEARING (verified by
//! the 7 eval corpora) — match the Python byte-for-byte. The `Vision` trait abstracts the duck-typed
//! Python `client` so the build is testable with a mock (no endpoint).

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use capture_core::frames::{list_frames, select_leaves, Frame};
use capture_core::time::{iso, now};
use capture_core::transcript::{load_transcript, transcript_slice, Segment};

use crate::prompts::{
    classify_prompt, classify_schema, code_types, combine_prompt, content_prompt, content_types,
    default_preset,
};

/// `INDEX_VERSION` — bump only on a breaking on-disk index change.
pub const INDEX_VERSION: u32 = 1;
/// `TRANSCRIPT_FEED_CAP` — max chars of transcript fed to a single combine call.
pub const TRANSCRIPT_FEED_CAP: usize = 1500;

/// The vision model surface the indexer needs (the Python `client` is duck-typed:
/// `caption_image` / `structured_image` / `combine`). Abstracted as a trait so `build_index`
/// is testable with a mock — no endpoint required.
pub trait Vision {
    /// Vision call: describe `path` per `prompt`; returns the model text. `max_px` overrides the
    /// client default downscale (e.g. higher res for code).
    fn caption_image(&self, path: &Path, prompt: &str, max_px: Option<u32>) -> Result<String, String>;
    /// STRUCTURED vision call: a JSON object validated against `schema`.
    fn structured_image(
        &self,
        path: &Path,
        prompt: &str,
        schema: &Value,
        max_px: Option<u32>,
    ) -> Result<Value, String>;
    /// Text-only call: fuse child summaries (+ transcript) into a range summary.
    fn combine(&self, prompt: &str) -> Result<String, String>;
}

/// Thin forwarding impl so the real `VisionClient` satisfies the trait (it already has these shapes).
impl Vision for crate::vision::VisionClient {
    fn caption_image(&self, path: &Path, prompt: &str, max_px: Option<u32>) -> Result<String, String> {
        crate::vision::VisionClient::caption_image(self, path, prompt, max_px)
    }
    fn structured_image(
        &self,
        path: &Path,
        prompt: &str,
        schema: &Value,
        max_px: Option<u32>,
    ) -> Result<Value, String> {
        crate::vision::VisionClient::structured_image(self, path, prompt, schema, max_px)
    }
    fn combine(&self, prompt: &str) -> Result<String, String> {
        crate::vision::VisionClient::combine(self, prompt)
    }
}

/// The representative frame of a node: the source screenshot path + its display ISO stamp.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ReprFrame {
    pub path: String,
    pub iso: String,
}

/// One node of the merge-tree (leaf or range). The JSON keys match the Python `_node` EXACTLY.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Node {
    pub id: String,
    pub depth: u32,
    pub lo_idx: usize,
    pub hi_idx: usize,
    pub t_lo: f64,
    /// `t_hi` is `None` for the right edge (`+inf` in Python).
    pub t_hi: Option<f64>,
    pub repr_frame: ReprFrame,
    pub represents_n_frames: usize,
    pub content_type: String,
    /// leaves: the STRUCTURED extraction (participants, active_speaker, …); ranges: `None`.
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub vision_caption: Option<String>,
    pub transcript_slice: String,
    pub summary: String,
    pub children: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
}

/// The build params recorded in `index.json` (the resume key — a changed param ⇒ a fresh build).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Params {
    pub sample_rate: f64,
    pub max_leaves: usize,
    pub fuse_transcript: bool,
    pub prompt_preset: String,
    pub leaf_prompt: Option<String>,
    pub leaf_schema: Option<Value>,
}

/// The assembled index — written to `index.json`. Keys match the Python `_assemble` EXACTLY.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Index {
    pub index_version: u32,
    pub model: Option<String>,
    pub params: Params,
    pub created_at: String,
    pub leaf_count: usize,
    pub node_count: usize,
    pub complete: bool,
    pub root_id: String,
    pub root_summary: String,
    pub nodes: Vec<Node>,
}

/// Longest-edge downscale to use for code/terminal extraction (default 2048; base is 1024).
/// Mirrors `_code_max_px` (env `CAPTURE_INDEX_CODE_MAX_PX`, parse failure → 2048).
fn code_max_px_env() -> u32 {
    match std::env::var("CAPTURE_INDEX_CODE_MAX_PX") {
        Ok(s) => s.trim().parse::<u32>().unwrap_or(2048),
        Err(_) => 2048,
    }
}

/// Options for [`build_index`] (Rust has no kwargs; the defaults mirror the Python signature).
pub struct BuildOptions<'a> {
    pub sample_rate: f64,
    pub max_leaves: usize,
    pub fuse_transcript: bool,
    pub prompt_preset: Option<&'a str>,
    pub leaf_prompt: Option<&'a str>,
    pub leaf_schema: Option<&'a Value>,
    pub classify_prompt: Option<&'a str>,
    pub code_max_px: Option<u32>,
    pub model_label: Option<&'a str>,
}

impl Default for BuildOptions<'_> {
    fn default() -> Self {
        BuildOptions {
            sample_rate: 0.5,
            max_leaves: 512,
            fuse_transcript: true,
            prompt_preset: None,
            leaf_prompt: None,
            leaf_schema: None,
            classify_prompt: None,
            code_max_px: None,
            model_label: None,
        }
    }
}

/// `on_progress(phase, done, total, t_lo, t_hi)` — `phase` is `"caption"` (a leaf) or `"combine"`
/// (an internal node); `t_hi` is `None` for the right-edge `+inf`. Mirrors the Python callback
/// (which passes `[t_lo, t_hi]`); the Rust callback receives the pair unpacked.
pub type ProgressFn<'a> = dyn FnMut(&str, usize, usize, f64, Option<f64>) + 'a;

/// Build (or resume) the index for `session_dir`; returns the index (also written to `index.json`
/// + `index_summary.txt` + `index_prompts.json` + `AGENTS.md`).
///
/// `client` is any [`Vision`]. `on_progress` is called per node. Returns `Err("no screenshots to
/// index")` when the session has no leaves (mirrors the Python `ValueError`). A faithful port of
/// `core/indexer.py::build_index` (lines 278–420).
#[allow(clippy::too_many_arguments)]
pub fn build_index(
    session_dir: &Path,
    client: &dyn Vision,
    opts: &BuildOptions,
    mut on_progress: Option<&mut ProgressFn>,
) -> Result<Index, String> {
    let d = session_dir;
    let all_frames = list_frames(d);
    let leaves = select_leaves(&all_frames, opts.sample_rate, opts.max_leaves);
    if leaves.is_empty() {
        return Err("no screenshots to index".to_string());
    }

    let segments: Vec<Segment> = if opts.fuse_transcript {
        load_transcript(d)
    } else {
        Vec::new()
    };
    let n = leaves.len();
    let total_nodes = 2 * n - 1;

    let preset: String = opts
        .prompt_preset
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_preset().to_string());

    // "auto": CLASSIFY each frame (structured enum) then run that type's STRUCTURED extraction.
    // A fixed preset (e.g. "meeting") skips classification. Custom prompts (typically crafted by a
    // frontier model calling capture_index, executed cheaply by the LOCAL model):
    //   • custom_leaf + leaf_schema → a custom STRUCTURED extractor (one schema for every frame).
    //   • custom_leaf alone        → a custom free-text caption.
    //   • classify_prompt          → overrides the auto classifier's prompt.
    let custom_leaf: Option<String> = opts
        .leaf_prompt
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            std::env::var("CAPTURE_INDEX_LEAF_PROMPT")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
    let custom_struct = custom_leaf.is_some() && opts.leaf_schema.is_some();
    let classify_prompt_used: String = opts
        .classify_prompt
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| classify_prompt().to_string());
    let auto = preset == "auto" && custom_leaf.is_none();
    let cmpx = opts.code_max_px.unwrap_or_else(code_max_px_env); // higher-res for code/terminal

    let code_set = code_types();
    let is_code_type = |t: &str| code_set.iter().any(|c| c == t);

    // A fixed preset's combine focus (None in auto / custom modes → resolved per-range below).
    let fixed_focus: Option<String> = if auto || custom_leaf.is_some() {
        None
    } else {
        Some(content_prompt(&preset).combine_focus)
    };

    let params = Params {
        sample_rate: opts.sample_rate,
        max_leaves: opts.max_leaves,
        fuse_transcript: opts.fuse_transcript,
        prompt_preset: preset.clone(),
        leaf_prompt: custom_leaf.clone(),
        leaf_schema: opts.leaf_schema.cloned(),
    };

    // Per-leaf end offset (the span a leaf represents): the next leaf's offset, or +inf for the
    // last, so trailing transcript is still captured at the right edge.
    let leaf_end: Vec<f64> = (0..n)
        .map(|i| {
            if i + 1 < n {
                leaves[i + 1].offset
            } else {
                f64::INFINITY
            }
        })
        .collect();

    let existing = load_checkpoint(d, &params, opts.model_label); // id -> prior node (reused if it has a summary)
    let mut nodes: HashMap<String, Node> = HashMap::new();
    let mut done: usize = 0;
    let mut backup_done = false;
    let root_id = format!("0-{}", n - 1);

    // The recursion is iterative-via-helper to thread the &mut state Rust requires. The shape
    // (mid = (lo+hi)/2, left=(lo,mid), right=(mid+1,hi)) is byte-identical to the Python `visit`.
    let ctx = VisitCtx {
        leaves: &leaves,
        leaf_end: &leaf_end,
        segments: &segments,
        existing: &existing,
        custom_leaf: custom_leaf.as_deref(),
        custom_struct,
        leaf_schema: opts.leaf_schema,
        classify_prompt_used: &classify_prompt_used,
        auto,
        preset: &preset,
        cmpx,
        is_code_type: &is_code_type,
        fixed_focus: fixed_focus.as_deref(),
    };

    let root_node = visit(
        &ctx,
        client,
        &mut nodes,
        0,
        n - 1,
        0,
        d,
        &params,
        opts.model_label,
        &root_id,
        n,
        total_nodes,
        &mut backup_done,
        &mut done,
        &mut on_progress,
    )?;

    // Stamp parents (children carry ids; set the reverse link in one pass).
    let parent_links: Vec<(String, String)> = nodes
        .values()
        .flat_map(|node| {
            let pid = node.id.clone();
            node.children.iter().map(move |cid| (cid.clone(), pid.clone()))
        })
        .collect();
    for (cid, pid) in parent_links {
        if let Some(child) = nodes.get_mut(&cid) {
            child.parent = Some(pid);
        }
    }

    flag_code_reliability(&mut nodes); // #51: mark cross-frame-disagreeing code + surface dictated tokens
    let index = assemble(&params, opts.model_label, &nodes, &root_node.id, n, total_nodes);
    write_index(d, &index);
    write_prompts_record(
        d,
        opts.model_label,
        &preset,
        &nodes,
        n,
        if auto { Some(&classify_prompt_used) } else { None },
        opts.classify_prompt,
        custom_leaf.as_deref(),
        opts.leaf_schema,
    );
    write_agents_md(d, &index, opts.model_label, &nodes);
    Ok(index)
}

/// Immutable per-build context threaded through `visit` (the closures + slices the recursion reads).
struct VisitCtx<'a> {
    leaves: &'a [Frame],
    leaf_end: &'a [f64],
    segments: &'a [Segment],
    existing: &'a HashMap<String, Node>,
    custom_leaf: Option<&'a str>,
    custom_struct: bool,
    leaf_schema: Option<&'a Value>,
    classify_prompt_used: &'a str,
    auto: bool,
    preset: &'a str,
    cmpx: u32,
    is_code_type: &'a dyn Fn(&str) -> bool,
    fixed_focus: Option<&'a str>,
}

/// The recursive `visit(lo, hi, depth)` — leaf (lo==hi) classifies+extracts; internal combines its
/// two children. Checkpoints after EVERY node. A faithful port of the Python inner `visit`.
#[allow(clippy::too_many_arguments)]
fn visit(
    ctx: &VisitCtx,
    client: &dyn Vision,
    nodes: &mut HashMap<String, Node>,
    lo: usize,
    hi: usize,
    depth: u32,
    d: &Path,
    params: &Params,
    model_label: Option<&str>,
    root_id: &str,
    leaf_count: usize,
    node_count: usize,
    backup_done: &mut bool,
    done: &mut usize,
    on_progress: &mut Option<&mut ProgressFn>,
) -> Result<Node, String> {
    let nid = format!("{lo}-{hi}");
    let mid = (lo + hi) / 2;
    let repr_leaf = &ctx.leaves[mid];
    let t_lo = ctx.leaves[lo].offset;
    let t_hi = ctx.leaf_end[hi];
    let cached = ctx.existing.get(&nid);

    let node: Node = if lo == hi {
        // leaf — classify (auto), then STRUCTURED extraction.
        let mut ctype: Option<String> = cached.map(|c| c.content_type.clone());
        let mut caption: Option<String> = cached.and_then(|c| c.vision_caption.clone());
        let mut data: Option<Value> = cached.and_then(|c| c.data.clone());

        if caption.is_none() {
            if ctx.custom_struct {
                // a custom STRUCTURED extractor (prompt + schema), no classify.
                ctype = Some("custom".to_string());
                let schema = ctx.leaf_schema.cloned().unwrap_or_else(|| json!({}));
                let extracted = client.structured_image(
                    &repr_leaf.path,
                    ctx.custom_leaf.unwrap_or(""),
                    &schema,
                    None,
                )?;
                let summary = extracted
                    .get("summary")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                caption = Some(if !summary.is_empty() {
                    summary
                } else {
                    serde_json::to_string(&extracted).unwrap_or_default()
                });
                data = Some(extracted);
            } else if let Some(custom) = ctx.custom_leaf {
                // a custom free-text prompt → caption, no structured fields.
                ctype = Some(ctx.preset.to_string());
                data = None;
                caption = Some(client.caption_image(&repr_leaf.path, custom, None)?);
            } else {
                let resolved_type = if ctx.auto {
                    let cls = client.structured_image(
                        &repr_leaf.path,
                        ctx.classify_prompt_used,
                        &classify_schema(),
                        None,
                    )?;
                    cls.get("content_type")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .unwrap_or("other")
                        .to_string()
                } else {
                    ctx.preset.to_string()
                };
                let cp = content_prompt(&resolved_type);
                // Code/terminal frames carry small dense text → extract at higher resolution.
                let mpx = if (ctx.is_code_type)(&resolved_type) {
                    Some(ctx.cmpx)
                } else {
                    None
                };
                let extracted =
                    client.structured_image(&repr_leaf.path, &cp.prompt, &cp.schema, mpx)?;
                caption = Some(
                    extracted
                        .get("summary")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                );
                data = Some(extracted);
                ctype = Some(resolved_type);
            }
        }

        let tslice = transcript_slice(ctx.segments, t_lo, t_hi);
        let caption_str = caption.unwrap_or_default();
        let node = make_node(
            &nid,
            depth,
            lo,
            hi,
            repr_leaf,
            t_lo,
            t_hi,
            1,
            ctype.unwrap_or_default(),
            Some(caption_str.clone()),
            tslice,
            caption_str,
            Vec::new(),
            data,
        );
        report(on_progress, done, "caption", node_count, t_lo, t_hi);
        node
    } else {
        let left = visit(
            ctx, client, nodes, lo, mid, depth + 1, d, params, model_label, root_id, leaf_count,
            node_count, backup_done, done, on_progress,
        )?;
        let right = visit(
            ctx, client, nodes, mid + 1, hi, depth + 1, d, params, model_label, root_id,
            leaf_count, node_count, backup_done, done, on_progress,
        )?;
        // A range's type is its children's if they agree, else "mixed" → general focus.
        let lt = &left.content_type;
        let rt = &right.content_type;
        let ctype = if lt == rt { lt.clone() } else { "mixed".to_string() };
        // "mixed" routes to "general" for the combine focus (per the Python ternary).
        let focus: String = match ctx.fixed_focus {
            Some(f) => f.to_string(),
            None => {
                let route = if ctype != "mixed" { ctype.as_str() } else { "general" };
                content_prompt(route).combine_focus
            }
        };
        let tslice = transcript_slice(ctx.segments, t_lo, t_hi);
        let summary: String = match cached.map(|c| c.summary.clone()).filter(|s| !s.is_empty()) {
            Some(s) => s,
            None => {
                let feed: String = tslice.chars().take(TRANSCRIPT_FEED_CAP).collect();
                client.combine(&combine_prompt(&left.summary, &right.summary, &feed, &focus))?
            }
        };
        let node = make_node(
            &nid,
            depth,
            lo,
            hi,
            repr_leaf,
            t_lo,
            t_hi,
            hi - lo + 1,
            ctype,
            None,
            tslice,
            summary,
            vec![left.id.clone(), right.id.clone()],
            None,
        );
        report(on_progress, done, "combine", node_count, t_lo, t_hi);
        node
    };

    nodes.insert(nid.clone(), node.clone());
    // Checkpoint after each node so a crash/network drop resumes (skip done nodes).
    save_checkpoint(
        d,
        params,
        model_label,
        nodes,
        root_id,
        leaf_count,
        node_count,
        backup_done,
    );
    Ok(node)
}

/// Bump the progress counter + invoke the callback (errors swallowed, like the Python `try`).
fn report(
    on_progress: &mut Option<&mut ProgressFn>,
    done: &mut usize,
    phase: &str,
    total: usize,
    t_lo: f64,
    t_hi: f64,
) {
    *done += 1;
    if let Some(cb) = on_progress {
        let t_hi_opt = if t_hi.is_infinite() { None } else { Some(t_hi) };
        cb(phase, *done, total, t_lo, t_hi_opt);
    }
}

/// Build a node — mirrors the Python `_node` (rounds `t_lo` to 3dp; `t_hi=None` for `+inf`).
#[allow(clippy::too_many_arguments)]
fn make_node(
    nid: &str,
    depth: u32,
    lo: usize,
    hi: usize,
    repr_leaf: &Frame,
    t_lo: f64,
    t_hi: f64,
    n_frames: usize,
    content_type: String,
    vision_caption: Option<String>,
    transcript_slice: String,
    summary: String,
    children: Vec<String>,
    data: Option<Value>,
) -> Node {
    Node {
        id: nid.to_string(),
        depth,
        lo_idx: lo,
        hi_idx: hi,
        t_lo: round_dp(t_lo, 3),
        t_hi: if t_hi.is_infinite() {
            None
        } else {
            Some(round_dp(t_hi, 3))
        },
        repr_frame: ReprFrame {
            path: repr_leaf.path.to_string_lossy().to_string(),
            iso: repr_leaf.iso.clone(),
        },
        represents_n_frames: n_frames,
        content_type,
        data,
        vision_caption,
        transcript_slice,
        summary,
        children,
        parent: None,
    }
}

/// Round half-to-even to `ndigits` places — matches Python's `round(x, n)` (banker's rounding).
fn round_dp(x: f64, ndigits: i32) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let scale = 10f64.powi(ndigits);
    let scaled = x * scale;
    let r = scaled.round(); // half away from zero
    let rounded = if (scaled - scaled.trunc()).abs() == 0.5 {
        let floor = scaled.floor();
        if (floor as i64) % 2 == 0 {
            floor
        } else {
            floor + 1.0
        }
    } else {
        r
    };
    rounded / scale
}

// -- #51 code-reliability flagging --------------------------------------------

/// `_NUM_RE` — `-?\d+(?:\.\d+)?`.
fn num_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"-?\d+(?:\.\d+)?").unwrap())
}

/// `_IDENT_RE` — CamelCase / ALL_CAPS / dotted identifiers spoken in narration.
fn ident_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"\b(?:[A-Z][a-z0-9]+){2,}\b|\b[A-Z][A-Z0-9_]{2,}\b|\b\w+\.\w+\b").unwrap()
    })
}

/// Candidate DICTATED tokens spoken in the narration over a frame — numbers + identifier-like
/// words. Heuristic, deterministic. Port of `_narration_values`.
fn narration_values(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let nums: Vec<String> = num_re()
        .find_iter(text)
        .take(14)
        .map(|m| m.as_str().to_string())
        .collect();
    let idents: Vec<String> = ident_re()
        .find_iter(text)
        .take(10)
        .map(|m| m.as_str().to_string())
        .collect();
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for v in nums.into_iter().chain(idents.into_iter()) {
        if seen.insert(v.clone()) {
            out.push(v);
        }
    }
    out.truncate(18);
    out
}

/// Content types whose leaves carry code worth reliability-flagging (#51).
/// `_CODE_RELIABILITY_TYPES = CODE_TYPES ∪ {"custom","lecture"}`.
fn is_code_reliability_type(t: &str) -> bool {
    code_types().iter().any(|c| c == t) || t == "custom" || t == "lecture"
}

/// Read `data.file` else `data.file_or_asset` (lowercased, trimmed) — `""` if absent.
fn data_file_key(data: Option<&Value>) -> String {
    let d = match data {
        Some(v) => v,
        None => return String::new(),
    };
    let f = d
        .get("file")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| d.get("file_or_asset").and_then(|v| v.as_str()))
        .unwrap_or("");
    f.trim().to_lowercase()
}

/// #51 (flag-now half): on code leaves, attach `narration_values` (dictated tokens from the
/// transcript) and set `ocr_uncertain` where the OCR'd file name DISAGREES across frames — a
/// file/asset seen only once amid several differently-named code leaves is the local model's
/// confabulation signature. Returns the count flagged uncertain. Port of `_flag_code_reliability`.
fn flag_code_reliability(nodes: &mut HashMap<String, Node>) -> usize {
    // The ids of the code leaves (leaf == no children, type in the reliability set).
    let code_leaf_ids: Vec<String> = nodes
        .values()
        .filter(|nd| nd.children.is_empty() && is_code_reliability_type(&nd.content_type))
        .map(|nd| nd.id.clone())
        .collect();
    if code_leaf_ids.is_empty() {
        return 0;
    }
    // Counter over the (non-empty) file names.
    let mut files: HashMap<String, usize> = HashMap::new();
    for id in &code_leaf_ids {
        let f = data_file_key(nodes[id].data.as_ref());
        if !f.is_empty() {
            *files.entry(f).or_insert(0) += 1;
        }
    }
    let distinct_files = files.len();
    let code_leaves = code_leaf_ids.len();
    let mut flagged = 0;
    for id in &code_leaf_ids {
        let nv = narration_values(&nodes[id].transcript_slice);
        let f = data_file_key(nodes[id].data.as_ref());
        // A singleton file name amid ≥3 distinct names across ≥4 code leaves ⇒ likely confabulated.
        let uncertain = !f.is_empty()
            && files.get(&f).copied().unwrap_or(0) <= 1
            && distinct_files >= 3
            && code_leaves >= 4;

        // Materialize `data` as an object (mirrors `d = nd.get("data") or {}`).
        let node = nodes.get_mut(id).unwrap();
        let mut obj = match node.data.take() {
            Some(Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        if !nv.is_empty() {
            obj.insert("narration_values".to_string(), json!(nv));
        }
        if uncertain {
            obj.insert("ocr_uncertain".to_string(), json!(true));
            flagged += 1;
        }
        node.data = Some(Value::Object(obj));
    }
    flagged
}

// -- prompts record (#56 corpus) ----------------------------------------------

/// Persist the prompts/schemas this index used to `<session>/index_prompts.json` — the corpus the
/// tuning skill ingests. Records the per-type content distribution, the (default or overridden)
/// classify prompt, and any caller-supplied custom prompts/schemas. Port of `_write_prompts_record`.
#[allow(clippy::too_many_arguments)]
fn write_prompts_record(
    d: &Path,
    model_label: Option<&str>,
    preset: &str,
    nodes: &HashMap<String, Node>,
    leaf_count: usize,
    classify_prompt_record: Option<&str>,
    custom_classify_prompt: Option<&str>,
    custom_leaf_prompt: Option<&str>,
    custom_leaf_schema: Option<&Value>,
) {
    // counts: Counter of leaf content_types (non-empty).
    let mut counts: serde_json::Map<String, Value> = serde_json::Map::new();
    // Preserve insertion order so the JSON matches a Python Counter's first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut tally: HashMap<String, i64> = HashMap::new();
    // Iterate nodes in id-sort order for a deterministic first-seen order.
    let ordered_ids = sorted_ids(nodes);
    for id in &ordered_ids {
        let nd = &nodes[id];
        if nd.children.is_empty() && !nd.content_type.is_empty() {
            if !tally.contains_key(&nd.content_type) {
                order.push(nd.content_type.clone());
            }
            *tally.entry(nd.content_type.clone()).or_insert(0) += 1;
        }
    }
    for t in &order {
        counts.insert(t.clone(), json!(tally[t]));
    }

    // extract_defaults: the default extractors that actually fired (CONTENT_PROMPTS ∩ counts).
    // Iterate in CONTENT_PROMPTS order; content_types() preserves that order minus "general", and
    // "general" can also fire (a leaf typed "general"), so check both.
    let mut extract_used: serde_json::Map<String, Value> = serde_json::Map::new();
    let mut prompt_type_order: Vec<String> = vec!["general".to_string()];
    for t in content_types() {
        if t != "other" {
            prompt_type_order.push(t);
        }
    }
    for t in &prompt_type_order {
        if tally.contains_key(t) {
            let cp = content_prompt(t);
            // Only true CONTENT_PROMPTS entries (general + the real types); "other"/unknown route
            // to general but never appear here because counts holds resolved types.
            extract_used.insert(
                t.clone(),
                json!({ "prompt": cp.prompt, "schema": cp.schema }),
            );
        }
    }

    let classify_block = classify_prompt_record.map(|p| {
        json!({ "prompt": p, "enum": content_types() })
    });

    // custom: {classify_prompt, leaf_prompt, leaf_schema} filtered to truthy values.
    let mut custom: serde_json::Map<String, Value> = serde_json::Map::new();
    if let Some(cp) = custom_classify_prompt.filter(|s| !s.is_empty()) {
        custom.insert("classify_prompt".to_string(), json!(cp));
    }
    if let Some(lp) = custom_leaf_prompt.filter(|s| !s.is_empty()) {
        custom.insert("leaf_prompt".to_string(), json!(lp));
    }
    if let Some(ls) = custom_leaf_schema {
        custom.insert("leaf_schema".to_string(), ls.clone());
    }

    let record = json!({
        "index_version": INDEX_VERSION,
        "model": model_label,
        "preset": preset,
        "created_at": iso(Some(now())),
        "leaf_count": leaf_count,
        "type_counts": Value::Object(counts),
        "classify": classify_block,
        "extract_defaults": Value::Object(extract_used),
        "custom": Value::Object(custom),
    });
    if let Ok(s) = serde_json::to_string_pretty(&record) {
        let _ = std::fs::write(d.join("index_prompts.json"), s);
    }
}

// -- AGENTS.md (#57) ----------------------------------------------------------

/// Write `<session>/AGENTS.md` — a trust-calibration + usage guide for any agent that later
/// consumes this capture (#57). Content-aware via the leaf type mix. Port of `_write_agents_md`.
fn write_agents_md(d: &Path, index: &Index, model_label: Option<&str>, nodes: &HashMap<String, Node>) {
    // Leaves in id-sort order (so the flagged-frames list + mix order are deterministic).
    let ordered_ids = sorted_ids(nodes);
    let leaves: Vec<&Node> = ordered_ids
        .iter()
        .map(|id| &nodes[id])
        .filter(|nd| nd.children.is_empty())
        .collect();

    // counts: Counter of leaf content_types (non-empty), most_common order.
    let mut order: Vec<String> = Vec::new();
    let mut tally: HashMap<String, i64> = HashMap::new();
    for nd in &leaves {
        if !nd.content_type.is_empty() {
            if !tally.contains_key(&nd.content_type) {
                order.push(nd.content_type.clone());
            }
            *tally.entry(nd.content_type.clone()).or_insert(0) += 1;
        }
    }
    // most_common(): sort by count desc, ties keep first-seen order (Python Counter behavior).
    let mut common: Vec<(String, i64)> =
        order.iter().map(|t| (t.clone(), tally[t])).collect();
    common.sort_by(|a, b| b.1.cmp(&a.1)); // stable sort preserves first-seen for ties
    let mix = if common.is_empty() {
        "mixed".to_string()
    } else {
        common
            .iter()
            .map(|(t, c)| format!("{c} {t}"))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let has_count = |t: &str| tally.contains_key(t);
    let has_code = ["coding", "terminal", "lecture", "custom"]
        .iter()
        .any(|t| has_count(t));
    let has_meeting = has_count("meeting");
    let uncertain: Vec<&&Node> = leaves
        .iter()
        .filter(|nd| {
            nd.data
                .as_ref()
                .and_then(|d| d.get("ocr_uncertain"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        })
        .collect();
    let has_nv = leaves.iter().any(|nd| {
        nd.data
            .as_ref()
            .and_then(|d| d.get("narration_values"))
            .map(|v| !v.is_null())
            .unwrap_or(false)
    });

    // title / recorded from session.json summary.
    let mut title = d
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let mut recorded = String::new();
    if let Ok(text) = std::fs::read_to_string(d.join("session.json")) {
        if let Ok(meta) = serde_json::from_str::<Value>(&text) {
            if let Some(sm) = meta.get("summary") {
                let wt = sm.get("window_title").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                let an = sm.get("app_name").and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                if let Some(t) = wt.or(an) {
                    title = t.to_string();
                }
                recorded = sm
                    .get("started_at")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }
        }
    }

    let model_str = model_label.filter(|s| !s.is_empty()).unwrap_or("local VLM");

    let mut out: Vec<String> = vec![format!("# Capture: {title}"), String::new()];
    if !recorded.is_empty() {
        out.push(format!("_Recorded {recorded}_\n"));
    }
    if !index.root_summary.is_empty() {
        out.push(index.root_summary.clone());
        out.push(String::new());
    }
    out.extend([
        "## Artifacts".to_string(),
        "- `index.json` — hierarchical index: per-frame leaf captions → range summaries → a root summary. Each".to_string(),
        "  leaf node carries `repr_frame.path` (the source screenshot), `content_type`, `data` (the structured".to_string(),
        "  extraction), and `transcript_slice` (the narration over that span).".to_string(),
        "- `transcript.jsonl` — the time-aligned spoken audio. **Authoritative.**".to_string(),
        "- `screenshots/` — the full-resolution source frames. Re-read these for anything you must trust verbatim.".to_string(),
        "- `index_prompts.json` — the model + prompts/schemas this index was built with.".to_string(),
        String::new(),
        "## How to trust this index (read first)".to_string(),
        format!("The structured `data` was extracted by a small LOCAL vision model (`{model_str}`). Treat"),
        "it as a cheap **scaffold for navigation, not ground truth**:".to_string(),
        "- **Transcript = reliable.** Where the narration states a value (a name, number, command, note), prefer it".to_string(),
        "  over the on-screen OCR.".to_string(),
        "- **Cross-frame disagreement = a red flag.** When the same on-screen content is captured differently across".to_string(),
        "  nearby leaves, that region is OCR-unreliable — verify it against the source frame.".to_string(),
        "- **Summaries / topics = directionally reliable** for locating things; exact details need the frame.".to_string(),
    ]);
    if has_code {
        out.extend([
            "- **Verbatim `code` is OCR and hallucination-prone** — the model can misread an identifier (e.g. drop a".to_string(),
            "  leading letter, `AActor`→`Actor`) or confabulate whole snippets. Before reproducing any code:".to_string(),
            "  cross-check the transcript, and **re-read the frame at `repr_frame.path`** (full resolution) for the".to_string(),
            "  exact tokens. Do not ship the index's `code` verbatim without verifying it against the frame.".to_string(),
            "- **Denoise by cross-frame consensus**: a token recurring across many reads of the same file is real; use the majority. On a split, prefer the MORE-SPECIFIC variant (OCR drops chars, rarely adds — e.g. `\\TFLog_` over `\\Log_`). Never NORMALIZE or 'fix' string literals / typos (`[*ERROR*]`, a `\": \"` separator) — preserve them or treat as uncertain; silently correcting them corrupts the code.".to_string(),
        ]);
        if has_nv {
            out.push("- **`data.narration_values`** holds tokens (numbers/identifiers) SPOKEN over a code frame — prefer these over the OCR'd `code` when they conflict (the narrator is more reliable than the OCR).".to_string());
        }
        if !uncertain.is_empty() {
            out.push(format!(
                "- **`data.ocr_uncertain: true`** marks the {} code frame(s) whose file name disagreed across frames (a confabulation signature) — re-read those FIRST (listed below).",
                uncertain.len()
            ));
        }
    }
    if has_meeting {
        out.extend([
            "- **Meeting fields** — participant names, task assignments, and decisions are reliable when the".to_string(),
            "  transcript corroborates them; small-font shared-board text (ticket IDs, dates) may be misread — verify".to_string(),
            "  from the frame.".to_string(),
        ]);
    }
    if !uncertain.is_empty() {
        out.extend([
            String::new(),
            "## Frames flagged for verification (#51)".to_string(),
            "These code frames disagreed with their neighbours (likely OCR confabulation) — re-read them first:".to_string(),
        ]);
        for nd in uncertain.iter().take(20) {
            let fp = nd.repr_frame.path.clone();
            let claimed = nd
                .data
                .as_ref()
                .map(|d| {
                    d.get("file")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .or_else(|| d.get("file_or_asset").and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
                        .unwrap_or("?")
                        .to_string()
                })
                .unwrap_or_else(|| "?".to_string());
            out.push(format!("- `{fp}` — claimed `{claimed}`"));
        }
    }
    out.extend([
        String::new(),
        "## This capture".to_string(),
        format!("- Content mix: {mix}"),
        format!("- {} leaves / {} nodes", index.leaf_count, index.node_count),
        String::new(),
    ]);

    let _ = std::fs::write(d.join("AGENTS.md"), out.join("\n"));
}

// -- assembly + checkpoint / output -------------------------------------------

/// `_id_sort_key`: sort by `(lo:int, hi:int)` parsed from the `"lo-hi"` id (fallback huge).
fn id_sort_key(nid: &str) -> (i64, i64) {
    let mut parts = nid.splitn(2, '-');
    if let (Some(lo), Some(hi)) = (parts.next(), parts.next()) {
        if let (Ok(lo), Ok(hi)) = (lo.parse::<i64>(), hi.parse::<i64>()) {
            return (lo, hi);
        }
    }
    // Python's fallback `(1 << 30, nid)` — strings sort after the int branch; a huge hi suffices
    // here because ids are well-formed in practice (the fallback never fires).
    (1 << 30, i64::MAX)
}

/// Node ids sorted by `_id_sort_key`.
fn sorted_ids(nodes: &HashMap<String, Node>) -> Vec<String> {
    let mut ids: Vec<String> = nodes.keys().cloned().collect();
    ids.sort_by_key(|a| id_sort_key(a));
    ids
}

/// Assemble the index dict from the node map. Port of `_assemble`.
fn assemble(
    params: &Params,
    model_label: Option<&str>,
    nodes: &HashMap<String, Node>,
    root_id: &str,
    leaf_count: usize,
    node_count: usize,
) -> Index {
    let ordered: Vec<Node> = sorted_ids(nodes).into_iter().map(|id| nodes[&id].clone()).collect();
    let root_summary = nodes.get(root_id).map(|n| n.summary.clone()).unwrap_or_default();
    Index {
        index_version: INDEX_VERSION,
        model: model_label.map(|s| s.to_string()),
        params: params.clone(),
        created_at: iso(Some(now())),
        leaf_count,
        node_count,
        complete: nodes.len() == node_count,
        root_id: root_id.to_string(),
        root_summary,
        nodes: ordered,
    }
}

fn index_path(session_dir: &Path) -> std::path::PathBuf {
    session_dir.join("index.json")
}

/// Prior nodes from a matching, incomplete `index.json` (same params + model), keyed by id — so a
/// resumed build reuses captions/summaries instead of re-calling the model. Port of `_load_checkpoint`.
fn load_checkpoint(
    session_dir: &Path,
    params: &Params,
    model_label: Option<&str>,
) -> HashMap<String, Node> {
    let p = index_path(session_dir);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(_) => return HashMap::new(),
    };
    let prev: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    // params + model must match exactly (else a fresh build overwrites the old index).
    let prev_params: Option<Params> =
        prev.get("params").and_then(|v| serde_json::from_value(v.clone()).ok());
    let prev_model = prev.get("model").and_then(|v| v.as_str());
    let model = model_label;
    if prev_params.as_ref() != Some(params) || prev_model != model {
        return HashMap::new();
    }
    let mut out: HashMap<String, Node> = HashMap::new();
    if let Some(arr) = prev.get("nodes").and_then(|v| v.as_array()) {
        for nv in arr {
            // Keep only nodes that already have a (non-empty) summary.
            let has_summary = nv
                .get("summary")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if !has_summary {
                continue;
            }
            if let Ok(node) = serde_json::from_value::<Node>(nv.clone()) {
                out.insert(node.id.clone(), node);
            }
        }
    }
    out
}

/// Checkpoint the in-progress index to `index.json` (and back up a prior COMPLETE index once).
/// Port of `_save_checkpoint`.
#[allow(clippy::too_many_arguments)]
fn save_checkpoint(
    session_dir: &Path,
    params: &Params,
    model_label: Option<&str>,
    nodes: &HashMap<String, Node>,
    root_id: &str,
    leaf_count: usize,
    node_count: usize,
    backup_once: &mut bool,
) {
    let p = index_path(session_dir);
    // Back up a prior COMPLETE index once, before the first checkpoint overwrites it.
    if !*backup_once {
        *backup_once = true;
        if let Ok(text) = std::fs::read_to_string(&p) {
            let was_complete = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("complete").and_then(|c| c.as_bool()))
                .unwrap_or(false);
            if was_complete {
                let _ = std::fs::rename(&p, session_dir.join("index.prev.json"));
            }
        }
    }
    let idx = assemble(params, model_label, nodes, root_id, leaf_count, node_count);
    if let Ok(s) = serde_json::to_string_pretty(&idx) {
        let _ = std::fs::write(&p, s);
    }
}

/// Write `index.json` + `index_summary.txt`. Port of `_write_index`.
fn write_index(session_dir: &Path, index: &Index) {
    if let Ok(s) = serde_json::to_string_pretty(index) {
        let _ = std::fs::write(index_path(session_dir), s);
    }
    let summary = format!("{}\n", index.root_summary.trim());
    let _ = std::fs::write(session_dir.join("index_summary.txt"), summary);
}

/// The built index for a session, or `None` if not indexed / unreadable. Port of `load_index`.
pub fn load_index(session_dir: &Path) -> Option<Index> {
    let text = std::fs::read_to_string(index_path(session_dir)).ok()?;
    serde_json::from_str::<Index>(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A hermetic mock Vision — returns canned structured `{"summary":"s", "file":...}` and
    /// `"combined"` for combines. Counts its calls so the resume test can assert zero re-calls.
    struct MockVision {
        structured_calls: AtomicUsize,
        caption_calls: AtomicUsize,
        combine_calls: AtomicUsize,
        /// Optional per-path structured override (for the flag-reliability test).
        overrides: RefCell<std::collections::HashMap<String, Value>>,
    }

    impl MockVision {
        fn new() -> Self {
            MockVision {
                structured_calls: AtomicUsize::new(0),
                caption_calls: AtomicUsize::new(0),
                combine_calls: AtomicUsize::new(0),
                overrides: RefCell::new(std::collections::HashMap::new()),
            }
        }
        fn total_calls(&self) -> usize {
            self.structured_calls.load(Ordering::SeqCst)
                + self.caption_calls.load(Ordering::SeqCst)
                + self.combine_calls.load(Ordering::SeqCst)
        }
    }

    impl Vision for MockVision {
        fn caption_image(&self, _p: &Path, _prompt: &str, _max_px: Option<u32>) -> Result<String, String> {
            self.caption_calls.fetch_add(1, Ordering::SeqCst);
            Ok("caption".to_string())
        }
        fn structured_image(
            &self,
            path: &Path,
            prompt: &str,
            _schema: &Value,
            _max_px: Option<u32>,
        ) -> Result<Value, String> {
            self.structured_calls.fetch_add(1, Ordering::SeqCst);
            // A classify call asks for `content_type`; route it to "general" so extraction runs.
            if prompt.contains("Classify what this screenshot") {
                return Ok(json!({ "content_type": "general", "app": "Mock" }));
            }
            let key = path.to_string_lossy().to_string();
            if let Some(v) = self.overrides.borrow().get(&key) {
                return Ok(v.clone());
            }
            Ok(json!({ "summary": "s", "file": "x.rs" }))
        }
        fn combine(&self, _prompt: &str) -> Result<String, String> {
            self.combine_calls.fetch_add(1, Ordering::SeqCst);
            Ok("combined".to_string())
        }
    }

    /// Unique temp session dir with N screenshots + a session.json + transcript.jsonl.
    fn mk_session(tag: &str, n: usize) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-index-build-{tag}-{nanos}"));
        fs::create_dir_all(d.join("screenshots")).unwrap();
        // Anchor the epoch via session.json started_at, with N screenshots 1s apart.
        let base = capture_core::time::fs_stamp(Some(1_700_000_000.0));
        let base_iso = capture_core::time::iso(Some(1_700_000_000.0));
        fs::write(
            d.join("session.json"),
            format!(
                r#"{{"summary":{{"started_at":"{base_iso}","window_title":"My Window","app_name":"App"}}}}"#
            ),
        )
        .unwrap();
        for i in 0..n {
            let stamp = capture_core::time::fs_stamp(Some(1_700_000_000.0 + i as f64));
            fs::write(d.join("screenshots").join(format!("{stamp}.png")), b"fakepng").unwrap();
        }
        let _ = base;
        // A small transcript so slices are non-trivial.
        fs::write(
            d.join("transcript.jsonl"),
            "{\"start_offset\":0.0,\"end_offset\":100.0,\"text\":\"hello world 42 FooBar\"}\n",
        )
        .unwrap();
        d
    }

    fn opts_general<'a>() -> BuildOptions<'a> {
        BuildOptions {
            sample_rate: 1.0,
            prompt_preset: Some("general"),
            ..Default::default()
        }
    }

    #[test]
    fn build_general_tree_shape_and_artifacts() {
        let n = 4;
        let d = mk_session("general", n);
        let client = MockVision::new();
        let idx = build_index(&d, &client, &opts_general(), None).expect("build ok");

        assert_eq!(idx.nodes.len(), 2 * n - 1, "node count = 2N-1");
        assert_eq!(idx.leaf_count, n);
        assert_eq!(idx.node_count, 2 * n - 1);
        assert!(idx.complete, "complete");
        assert_eq!(idx.root_id, format!("0-{}", n - 1));

        // Exactly one root (no parent).
        let roots: Vec<&Node> = idx.nodes.iter().filter(|nd| nd.parent.is_none()).collect();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].id, idx.root_id);

        // Every non-leaf has exactly 2 children; every leaf is content_type "general"; parents set.
        for nd in &idx.nodes {
            if nd.children.is_empty() {
                assert_eq!(nd.content_type, "general", "leaf type");
            } else {
                assert_eq!(nd.children.len(), 2, "internal has 2 children");
                for cid in &nd.children {
                    let child = idx.nodes.iter().find(|c| &c.id == cid).unwrap();
                    assert_eq!(child.parent.as_deref(), Some(nd.id.as_str()), "parent link");
                }
            }
        }

        // Artifacts written.
        assert!(d.join("index.json").is_file());
        assert!(d.join("AGENTS.md").is_file());
        assert!(d.join("index_prompts.json").is_file());
        assert!(d.join("index_summary.txt").is_file());

        // AGENTS.md picked up the window title from session.json.
        let agents = fs::read_to_string(d.join("AGENTS.md")).unwrap();
        assert!(agents.contains("# Capture: My Window"), "title from session.json");

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn json_keys_match_python() {
        let d = mk_session("keys", 1);
        let client = MockVision::new();
        let idx = build_index(&d, &client, &opts_general(), None).expect("build ok");
        let v = serde_json::to_value(&idx).unwrap();
        // Top-level keys.
        for k in [
            "index_version", "model", "params", "created_at", "leaf_count", "node_count",
            "complete", "root_id", "root_summary", "nodes",
        ] {
            assert!(v.get(k).is_some(), "index key {k}");
        }
        // Node keys (exact names).
        let node = &v["nodes"][0];
        for k in [
            "id", "depth", "lo_idx", "hi_idx", "t_lo", "t_hi", "repr_frame",
            "represents_n_frames", "content_type", "data", "vision_caption",
            "transcript_slice", "summary", "children", "parent",
        ] {
            assert!(node.get(k).is_some(), "node key {k}");
        }
        assert!(node["repr_frame"].get("path").is_some());
        assert!(node["repr_frame"].get("iso").is_some());
        // params keys.
        for k in [
            "sample_rate", "max_leaves", "fuse_transcript", "prompt_preset",
            "leaf_prompt", "leaf_schema",
        ] {
            assert!(v["params"].get(k).is_some(), "params key {k}");
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn single_leaf_one_node() {
        let d = mk_session("n1", 1);
        let client = MockVision::new();
        let idx = build_index(&d, &client, &opts_general(), None).expect("build ok");
        assert_eq!(idx.nodes.len(), 1);
        assert_eq!(idx.node_count, 1);
        assert!(idx.nodes[0].children.is_empty());
        assert_eq!(idx.root_id, "0-0");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn three_leaves_five_nodes() {
        let d = mk_session("n3", 3);
        let client = MockVision::new();
        let idx = build_index(&d, &client, &opts_general(), None).expect("build ok");
        assert_eq!(idx.nodes.len(), 5);
        assert_eq!(idx.node_count, 5);
        // Tree: root 0-2 → (0-1, 2-2); 0-1 → (0-0, 1-1).
        let ids: std::collections::HashSet<String> =
            idx.nodes.iter().map(|n| n.id.clone()).collect();
        for want in ["0-0", "1-1", "2-2", "0-1", "0-2"] {
            assert!(ids.contains(want), "missing node {want}");
        }
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn resume_reuses_cached_nodes() {
        let n = 4;
        let d = mk_session("resume", n);
        let client = MockVision::new();
        let idx1 = build_index(&d, &client, &opts_general(), None).expect("first build");
        let summaries1: Vec<String> = idx1.nodes.iter().map(|nd| nd.summary.clone()).collect();
        let first_calls = client.total_calls();
        assert!(first_calls > 0, "first build calls the model");

        // Second build, SAME params → every node is cached (has a summary) → ZERO new model calls.
        let client2 = MockVision::new();
        let idx2 = build_index(&d, &client2, &opts_general(), None).expect("resume build");
        assert_eq!(client2.total_calls(), 0, "resume re-calls the model zero times");
        let summaries2: Vec<String> = idx2.nodes.iter().map(|nd| nd.summary.clone()).collect();
        assert_eq!(summaries1, summaries2, "prior summaries kept");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn no_screenshots_errors() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-index-empty-{nanos}"));
        fs::create_dir_all(&d).unwrap();
        let client = MockVision::new();
        let err = build_index(&d, &client, &opts_general(), None).unwrap_err();
        assert_eq!(err, "no screenshots to index");
        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn flag_code_reliability_singleton_only() {
        // Construct 4 code (coding) leaves: 3 share file "a.rs", 1 is a singleton "weird.rs".
        // distinct_files = 2 ({a.rs, weird.rs})? No — need ≥3 distinct. Use a.rs (x2), b.rs, weird.rs:
        //   files = {a.rs:2, b.rs:1, weird.rs:1}; distinct=3; code_leaves=4.
        //   b.rs and weird.rs are both singletons → BOTH flagged (count<=1).
        // To flag ONLY one singleton, make the others non-singleton:
        //   a.rs x2, b.rs x1? b.rs would also flag. The Python flags ALL singletons meeting the
        //   gate. So assert the singletons flag and the repeated file does NOT.
        let mut nodes: HashMap<String, Node> = HashMap::new();
        let files = ["a.rs", "a.rs", "b.rs", "weird.rs"]; // distinct={a,b,weird}=3, leaves=4
        for (i, f) in files.iter().enumerate() {
            let id = format!("{i}-{i}");
            nodes.insert(
                id.clone(),
                Node {
                    id,
                    depth: 0,
                    lo_idx: i,
                    hi_idx: i,
                    t_lo: i as f64,
                    t_hi: Some((i + 1) as f64),
                    repr_frame: ReprFrame { path: format!("{i}.png"), iso: String::new() },
                    represents_n_frames: 1,
                    content_type: "coding".to_string(),
                    data: Some(json!({ "summary": "s", "file": f })),
                    vision_caption: Some("s".to_string()),
                    transcript_slice: String::new(),
                    summary: "s".to_string(),
                    children: Vec::new(),
                    parent: None,
                },
            );
        }
        let flagged = flag_code_reliability(&mut nodes);
        // a.rs appears twice (count 2 > 1) → never flagged; b.rs + weird.rs are singletons → flagged.
        assert_eq!(flagged, 2, "both singletons flagged");
        let uncertain = |id: &str| {
            nodes[id]
                .data
                .as_ref()
                .and_then(|d| d.get("ocr_uncertain"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        assert!(!uncertain("0-0"), "a.rs (repeated) not flagged");
        assert!(!uncertain("1-1"), "a.rs (repeated) not flagged");
        assert!(uncertain("2-2"), "b.rs singleton flagged");
        assert!(uncertain("3-3"), "weird.rs singleton flagged");
    }

    #[test]
    fn flag_code_reliability_single_singleton() {
        // To flag EXACTLY one: a.rs x2, b.rs x2, weird.rs x1 → distinct=3, leaves=5, only weird flagged.
        let mut nodes: HashMap<String, Node> = HashMap::new();
        let files = ["a.rs", "a.rs", "b.rs", "b.rs", "weird.rs"];
        for (i, f) in files.iter().enumerate() {
            let id = format!("{i}-{i}");
            nodes.insert(
                id.clone(),
                Node {
                    id,
                    depth: 0,
                    lo_idx: i,
                    hi_idx: i,
                    t_lo: i as f64,
                    t_hi: Some((i + 1) as f64),
                    repr_frame: ReprFrame { path: format!("{i}.png"), iso: String::new() },
                    represents_n_frames: 1,
                    content_type: "coding".to_string(),
                    data: Some(json!({ "summary": "s", "file": f })),
                    vision_caption: Some("s".to_string()),
                    transcript_slice: String::new(),
                    summary: "s".to_string(),
                    children: Vec::new(),
                    parent: None,
                },
            );
        }
        let flagged = flag_code_reliability(&mut nodes);
        assert_eq!(flagged, 1, "only the singleton flagged");
        let uncertain = |id: &str| {
            nodes[id]
                .data
                .as_ref()
                .and_then(|d| d.get("ocr_uncertain"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
        };
        assert!(uncertain("4-4"), "weird.rs singleton flagged");
        for id in ["0-0", "1-1", "2-2", "3-3"] {
            assert!(!uncertain(id), "{id} (repeated file) not flagged");
        }
    }

    #[test]
    fn narration_values_numbers_and_idents() {
        let nv = narration_values("the value is 42 and -3.14 with FooBar and ALL_CAPS plus a.b");
        assert!(nv.contains(&"42".to_string()));
        assert!(nv.contains(&"-3.14".to_string()));
        assert!(nv.contains(&"FooBar".to_string()));
        assert!(nv.contains(&"ALL_CAPS".to_string()));
        assert!(nv.iter().any(|s| s == "a.b"));
        // Empty text → empty.
        assert!(narration_values("").is_empty());
        // Dedup + cap 18.
        assert!(nv.len() <= 18);
    }
}
