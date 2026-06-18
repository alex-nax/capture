//! Golden-replay validation of the v3 Rust `capture-index` port — NO NETWORK.
//!
//! The LM Studio vision endpoint is unreachable from a shell binary (the macOS Local-Network gate
//! blocks it), so we can't re-run the model. Instead we REPLAY: take a complete Python-built index
//! (the ground truth) and feed its actual per-leaf model outputs (the classify result + the
//! structured extraction `data`) back through the Rust `build_index` via a mock `Vision`. Because
//! the replay supplies identical model outputs, every DETERMINISTIC step of the port — leaf
//! selection, tree shape, classification routing, the #51 reliability flagging, and AGENTS.md
//! assembly — must reproduce the Python's output exactly. Model-generated text (combine summaries,
//! the root summary) is intentionally NOT compared.
//!
//! Usage:
//!   cargo run -p capture-index --example golden_replay -- <python_index.json> <session_dir> <sample_rate>
//!
//! Exits non-zero if any deterministic check FAILS.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use capture_index::build::{self, BuildOptions, Index, Vision};
use serde_json::{json, Value};

/// The replayed model output for one leaf, keyed by frame stem.
#[derive(Clone)]
struct Leaf {
    content_type: String,
    /// The base structured extraction, with the DERIVED #51 fields (`ocr_uncertain`,
    /// `narration_values`) STRIPPED — so the Rust pipeline recomputes them from scratch, which is
    /// the actual determinism we want to test.
    data: Option<Value>,
}

/// Replay `Vision`: never touches the network. Looks each call up by the frame's stem.
struct ReplayVision {
    by_stem: HashMap<String, Leaf>,
}

impl ReplayVision {
    fn lookup(&self, path: &Path) -> Option<&Leaf> {
        let stem = path.file_stem().and_then(|s| s.to_str())?;
        self.by_stem.get(stem)
    }
}

/// True iff `schema` is the CLASSIFY schema (its `properties` carries a `content_type` key).
fn is_classify_schema(schema: &Value) -> bool {
    schema
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|p| p.contains_key("content_type"))
        .unwrap_or(false)
}

impl Vision for ReplayVision {
    fn caption_image(&self, _path: &Path, _prompt: &str, _max_px: Option<u32>) -> Result<String, String> {
        Ok(String::new())
    }

    fn structured_image(
        &self,
        path: &Path,
        _prompt: &str,
        schema: &Value,
        _max_px: Option<u32>,
    ) -> Result<Value, String> {
        let leaf = self.lookup(path).ok_or_else(|| {
            format!(
                "replay: no Python leaf for frame stem {:?}",
                path.file_stem().and_then(|s| s.to_str()).unwrap_or("?")
            )
        })?;
        if is_classify_schema(schema) {
            Ok(json!({ "content_type": leaf.content_type }))
        } else {
            // An extraction call → return the Python's recorded structured data.
            Ok(leaf.data.clone().unwrap_or_else(|| json!({})))
        }
    }

    fn combine(&self, _prompt: &str) -> Result<String, String> {
        Ok("replayed-combine".to_string())
    }
}

/// Strip the #51-derived fields the Rust pipeline recomputes, so the replay feeds only the BASE
/// extraction. (If we left them in, a stale `ocr_uncertain:true` from the Python could survive a
/// Rust recompute that decided "not uncertain", masking a real port bug.)
fn strip_derived(data: Option<Value>) -> Option<Value> {
    match data {
        Some(Value::Object(mut m)) => {
            m.remove("ocr_uncertain");
            m.remove("narration_values");
            Some(Value::Object(m))
        }
        other => other,
    }
}

fn stem_of(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

/// One PASS/FAIL check. Returns true on pass.
fn check(label: &str, ok: bool, detail: impl FnOnce() -> String) -> bool {
    if ok {
        println!("PASS  {label}");
    } else {
        println!("FAIL  {label}\n        {}", detail());
    }
    ok
}

/// Leaf nodes (empty `children`) sorted by (lo_idx, hi_idx).
fn ordered_leaves(idx: &Index) -> Vec<&build::Node> {
    let mut v: Vec<&build::Node> = idx.nodes.iter().filter(|n| n.children.is_empty()).collect();
    v.sort_by_key(|n| (n.lo_idx, n.hi_idx));
    v
}

/// The set of leaf stems whose `data` flags a given boolean / present key.
fn flagged_stems(idx: &Index, present_key: &str, require_true: bool) -> BTreeSet<String> {
    ordered_leaves(idx)
        .iter()
        .filter(|n| {
            n.data
                .as_ref()
                .and_then(|d| d.get(present_key))
                .map(|v| if require_true { v.as_bool().unwrap_or(false) } else { !v.is_null() })
                .unwrap_or(false)
        })
        .map(|n| stem_of(&n.repr_frame.path))
        .collect()
}

/// Normalize an AGENTS.md for structural comparison:
///  - drop the root-summary paragraph (the only model-generated prose),
///  - rewrite absolute frame paths in flagged-frame lines to just the basename (the two session
///    dirs have different prefixes; the basenames/claimed-names are what must match),
///  - drop the "Denoise by cross-frame consensus" trust bullet. That bullet IS emitted by the
///    current Python source (`core/indexer.py` ~L504) and by the Rust port, but the captured golden
///    fixture `daf420.python.AGENTS.md` predates it. Excusing this one source-evolution line keeps
///    the test asserting byte-equality on every OTHER structural line.
fn normalize_agents(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with("- **Denoise by cross-frame consensus**") {
            continue;
        }
        // Flagged-frame bullet: "- `<abs path>` — claimed `<x>`" → key on the basename only.
        if let Some(rest) = line.strip_prefix("- `") {
            if let Some(close) = rest.find('`') {
                let path = &rest[..close];
                if path.contains('/') && rest[close..].contains("claimed") {
                    let base = stem_of(path);
                    out.push(format!("- `{base}`{}", &rest[close + 1..]));
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    // Drop the root-summary paragraph: it sits between the "_Recorded …_" line and "## Artifacts".
    // (When present, it's a single non-empty paragraph followed by a blank line before "## Artifacts".)
    let recorded = out.iter().position(|l| l.starts_with("_Recorded"));
    let artifacts = out.iter().position(|l| l == "## Artifacts");
    if let (Some(r), Some(a)) = (recorded, artifacts) {
        // Keep up to and including the blank line after "_Recorded …_"; drop everything until
        // "## Artifacts". This removes the root-summary prose deterministically.
        let mut kept: Vec<String> = Vec::new();
        kept.extend(out[..=r].iter().cloned());
        // Preserve a single blank separator.
        kept.push(String::new());
        kept.extend(out[a..].iter().cloned());
        return kept;
    }
    out
}

fn main() {
    let mut args = std::env::args().skip(1);
    let py_path = args.next().expect("usage: golden_replay <python_index.json> <session_dir> <sample_rate>");
    let session = args.next().expect("usage: golden_replay <python_index.json> <session_dir> <sample_rate>");
    let sample_rate: f64 = args.next().and_then(|s| s.parse().ok()).expect("sample_rate must be a float");

    // --- Load the Python ground-truth index. -------------------------------------------------
    let py_text = std::fs::read_to_string(&py_path).expect("read python index");
    let py: Index = serde_json::from_str(&py_text).expect("deserialize python index into build::Index");

    // Build the replay map keyed by frame stem.
    let mut by_stem: HashMap<String, Leaf> = HashMap::new();
    for n in py.nodes.iter().filter(|n| n.children.is_empty()) {
        let stem = stem_of(&n.repr_frame.path);
        by_stem.insert(
            stem,
            Leaf {
                content_type: n.content_type.clone(),
                data: strip_derived(n.data.clone()),
            },
        );
    }
    eprintln!(
        "loaded python index: {} leaves, {} nodes, complete={}, {} replay leaves",
        py.leaf_count,
        py.node_count,
        py.complete,
        by_stem.len()
    );

    let replay = ReplayVision { by_stem };

    // --- Run the Rust build via the replay client. -------------------------------------------
    let opts = BuildOptions {
        sample_rate,
        prompt_preset: None, // auto (classify → per-type extract)
        model_label: Some("qwen/qwen3.5-9b"),
        ..Default::default()
    };
    let _rust: Index = build::build_index(Path::new(&session), &replay, &opts, None)
        .expect("rust build_index failed");

    // Re-read what was actually written (the authoritative output on disk).
    let rust_disk: Index = build::load_index(Path::new(&session)).expect("re-read rust index.json");

    // --- Deterministic checks. ----------------------------------------------------------------
    println!("\n=== golden-replay checks (deterministic; summaries excluded) ===");
    let mut all_ok = true;
    let mut fail = |b: bool| {
        if !b {
            all_ok = false;
        }
    };

    fail(check(
        &format!("leaf_count == {}", py.leaf_count),
        rust_disk.leaf_count == py.leaf_count,
        || format!("rust={} python={}", rust_disk.leaf_count, py.leaf_count),
    ));
    fail(check(
        &format!("node_count == {}", py.node_count),
        rust_disk.node_count == py.node_count,
        || format!("rust={} python={}", rust_disk.node_count, py.node_count),
    ));
    fail(check(
        &format!("complete == {}", py.complete),
        rust_disk.complete == py.complete,
        || format!("rust={} python={}", rust_disk.complete, py.complete),
    ));

    // Node id set identical (deterministic tree structure).
    let py_ids: BTreeSet<String> = py.nodes.iter().map(|n| n.id.clone()).collect();
    let rust_ids: BTreeSet<String> = rust_disk.nodes.iter().map(|n| n.id.clone()).collect();
    fail(check("node id set identical", py_ids == rust_ids, || {
        let only_rust: Vec<_> = rust_ids.difference(&py_ids).collect();
        let only_py: Vec<_> = py_ids.difference(&rust_ids).collect();
        format!("only-in-rust={only_rust:?} only-in-python={only_py:?}")
    }));

    // Ordered leaf frame STEMS identical (deterministic select_leaves).
    let py_stems: Vec<String> = ordered_leaves(&py).iter().map(|n| stem_of(&n.repr_frame.path)).collect();
    let rust_stems: Vec<String> = ordered_leaves(&rust_disk).iter().map(|n| stem_of(&n.repr_frame.path)).collect();
    fail(check("ordered leaf stems identical", py_stems == rust_stems, || {
        format!("rust={rust_stems:?}\n        python={py_stems:?}")
    }));

    // Per-leaf content_type matches (replayed → must be exact).
    let py_ct: BTreeMap<String, String> =
        ordered_leaves(&py).iter().map(|n| (n.id.clone(), n.content_type.clone())).collect();
    let rust_ct: BTreeMap<String, String> =
        ordered_leaves(&rust_disk).iter().map(|n| (n.id.clone(), n.content_type.clone())).collect();
    let ct_mismatches: Vec<String> = py_ct
        .iter()
        .filter_map(|(id, t)| {
            let r = rust_ct.get(id);
            if r != Some(t) {
                Some(format!("{id}: rust={:?} python={t}", r))
            } else {
                None
            }
        })
        .collect();
    fail(check("per-leaf content_type matches", ct_mismatches.is_empty(), || ct_mismatches.join("; ")));

    // #51 ocr_uncertain: count + the SET of flagged leaves.
    let py_unc = flagged_stems(&py, "ocr_uncertain", true);
    let rust_unc = flagged_stems(&rust_disk, "ocr_uncertain", true);
    fail(check(
        &format!("#51 ocr_uncertain count == {}", py_unc.len()),
        rust_unc.len() == py_unc.len(),
        || format!("rust={} python={}", rust_unc.len(), py_unc.len()),
    ));
    fail(check("#51 ocr_uncertain leaf SET matches", rust_unc == py_unc, || {
        format!(
            "only-in-rust={:?} only-in-python={:?}",
            rust_unc.difference(&py_unc).collect::<Vec<_>>(),
            py_unc.difference(&rust_unc).collect::<Vec<_>>()
        )
    }));

    // #51 narration_values presence: count + the SET of leaves carrying it.
    let py_nv = flagged_stems(&py, "narration_values", false);
    let rust_nv = flagged_stems(&rust_disk, "narration_values", false);
    fail(check(
        &format!("#51 narration_values count == {}", py_nv.len()),
        rust_nv.len() == py_nv.len(),
        || format!("rust={} python={}", rust_nv.len(), py_nv.len()),
    ));
    fail(check("#51 narration_values leaf SET matches", rust_nv == py_nv, || {
        format!(
            "only-in-rust={:?} only-in-python={:?}",
            rust_nv.difference(&py_nv).collect::<Vec<_>>(),
            py_nv.difference(&rust_nv).collect::<Vec<_>>()
        )
    }));

    // AGENTS.md structural sections byte-identical (root-summary paragraph normalized away;
    // flagged-frame paths reduced to basenames since the session dirs differ).
    let py_agents_path = Path::new(&py_path)
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("daf420.python.AGENTS.md");
    let py_agents = std::fs::read_to_string(&py_agents_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", py_agents_path.display()));
    let rust_agents = std::fs::read_to_string(Path::new(&session).join("AGENTS.md")).expect("read rust AGENTS.md");
    let py_norm = normalize_agents(&py_agents);
    let rust_norm = normalize_agents(&rust_agents);
    let agents_ok = py_norm == rust_norm;
    fail(check("AGENTS.md structural sections byte-identical", agents_ok, || {
        // Show the first differing line for diagnosis.
        let max = py_norm.len().max(rust_norm.len());
        for i in 0..max {
            let p = py_norm.get(i).map(String::as_str).unwrap_or("<eof>");
            let r = rust_norm.get(i).map(String::as_str).unwrap_or("<eof>");
            if p != r {
                return format!("first diff @line {i}:\n          python: {p:?}\n          rust:   {r:?}");
            }
        }
        "lengths differ".to_string()
    }));

    println!("\n=== summary ===");
    if all_ok {
        println!("ALL DETERMINISTIC CHECKS PASS");
    } else {
        println!("SOME CHECKS FAILED");
        std::process::exit(1);
    }
}
