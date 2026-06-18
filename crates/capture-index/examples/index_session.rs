//! End-to-end runner for the v3 capture-index port: build a real index for a session against a
//! live OpenAI-compatible vision endpoint. Used to validate the Rust port vs the Python on the
//! eval corpora (#62 piece 5).
//!
//! Usage:
//!   CAPTURE_INDEX_URL=http://192.168.31.217:1234/v1/chat/completions \
//!   CAPTURE_INDEX_MODEL=qwen/qwen3.5-9b \
//!   cargo run -p capture-index --example index_session -- <session_dir> [sample_rate]
//!
//! It writes index.json / AGENTS.md / index_summary.txt / index_prompts.json into <session_dir>
//! (a prior complete index.json is backed up to index.prev.json), then prints a summary.

use std::collections::BTreeMap;
use std::path::Path;

use capture_index::{build, vision};

fn main() {
    let mut args = std::env::args().skip(1);
    let session = args
        .next()
        .expect("usage: index_session <session_dir> [sample_rate]");
    let sample_rate: f64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(0.5);
    let model = std::env::var("CAPTURE_INDEX_MODEL").ok();

    let client = vision::load(None, model.as_deref()).expect("CAPTURE_INDEX_URL not set");
    eprintln!(
        "endpoint available: {} | model: {} | sample_rate: {}",
        client.available(),
        model.as_deref().unwrap_or("(default)"),
        sample_rate
    );

    let opts = build::BuildOptions {
        sample_rate,
        model_label: model.as_deref(),
        ..Default::default()
    };
    let mut prog = |phase: &str, done: usize, total: usize, _lo: f64, _hi: Option<f64>| {
        eprint!("\r  {phase} {done}/{total}        ");
    };
    let idx = build::build_index(Path::new(&session), &client, &opts, Some(&mut prog))
        .expect("build_index failed");
    eprintln!();

    // Leaf content-type distribution + #51 flags (the structural signal to compare vs Python).
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    let mut uncertain = 0usize;
    let mut narration = 0usize;
    for n in idx.nodes.iter().filter(|n| n.children.is_empty()) {
        *types.entry(n.content_type.clone()).or_default() += 1;
        if let Some(d) = &n.data {
            if d.get("ocr_uncertain").is_some() {
                uncertain += 1;
            }
            if d.get("narration_values").is_some() {
                narration += 1;
            }
        }
    }
    println!("leaves={} nodes={} complete={}", idx.leaf_count, idx.node_count, idx.complete);
    println!("leaf_content_types={types:?}");
    println!("ocr_uncertain={uncertain} narration_values_leaves={narration} (#51)");
    println!("root_summary: {}", idx.root_summary);
}
