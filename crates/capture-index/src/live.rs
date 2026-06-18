//! Live / online incremental indexing (#55; 1:1 port of `core/live_index.py`).
//!
//! Builds the multimodal index AS a session captures, one leaf at a time, so a navigable index
//! exists in near-real-time instead of only after a post-capture batch build. The tree is a
//! **binary merge-tree**: appending a frame extracts a leaf, then merges it with equal-sized
//! right-edge subtrees (a binary counter), so each append is O(log n) NEW combines and NEVER
//! recomputes existing summaries. The forest of power-of-2 subtrees collapses into a single root
//! at [`LiveIndex::finalize`].
//!
//! Runs only when a vision endpoint is reachable (a daemon worker drives [`LiveIndex::add_frame`]
//! off the capture hot path). No endpoint → the session falls back to the post-capture
//! [`crate::build::build_index`]. The output is shape-identical to a batch index (same node /
//! assemble / `AGENTS.md`), so everything downstream (the GUI tree, the eval/tuning skills) works
//! unchanged — it REUSES the helpers in [`crate::build`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::{json, Value};

use capture_core::frames::{list_frames, Frame};
use capture_core::transcript::{load_transcript, transcript_slice, Segment};

use crate::build::{
    assemble, code_max_px_env, flag_code_reliability, make_node, write_agents_md, write_index,
    write_prompts_record, Index, Node, ReprFrame, Vision, TRANSCRIPT_FEED_CAP,
};
use crate::prompts::{classify_prompt, classify_schema, code_types, combine_prompt, content_prompt};

/// Minimal stand-in for a [`Frame`] (just `path`/`iso`) so [`make_node`] can build a `repr_frame`
/// for an internal node from a child's stored [`ReprFrame`]. Mirrors the Python `_Repr`. The
/// `stamp`/`offset` are unused by `make_node` (it only reads `path`/`iso`).
fn repr_as_frame(repr: &ReprFrame) -> Frame {
    Frame {
        path: PathBuf::from(&repr.path),
        stamp: 0.0,
        offset: 0.0,
        iso: repr.iso.clone(),
    }
}

/// Char-safe truncation to at most `max` chars (never slices mid-UTF8). Mirrors Python `s[:max]`.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Classify (auto) → type-specific structured extraction for one frame → `(ctype, caption, data)`.
/// Mirrors the auto-path leaf step in `build_index`, including the #49 code-resolution bump
/// (`code_max_px` for code/terminal types). Port of `_extract_leaf`.
fn extract_leaf(
    client: &dyn Vision,
    frame_path: &Path,
    preset: &str,
    code_max_px: u32,
) -> Result<(String, String, Value), String> {
    let ctype: String = if !preset.is_empty() && preset != "auto" && preset != "general" {
        preset.to_string()
    } else {
        let cls = client.structured_image(frame_path, classify_prompt(), &classify_schema(), None)?;
        cls.get("content_type")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("other")
            .to_string()
    };
    let cp = content_prompt(&ctype);
    let mpx = if code_types().iter().any(|c| c == &ctype) {
        Some(code_max_px)
    } else {
        None
    };
    let data = client.structured_image(frame_path, &cp.prompt, &cp.schema, mpx)?;
    let caption = {
        let s = data
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if s.is_empty() {
            "(no caption)".to_string()
        } else {
            s
        }
    };
    Ok((ctype, caption, data))
}

/// The mutable state guarded by the lock (`nodes`, `forest`, `n`).
struct State {
    /// All nodes built so far, keyed by id (ordered so iteration is deterministic).
    nodes: BTreeMap<String, Node>,
    /// Completed subtrees, strictly DECREASING span left→right (the binary counter).
    forest: Vec<Node>,
    /// Number of leaves appended.
    n: usize,
}

/// Incremental binary merge-tree index, appended leaf-by-leaf. Thread-safe ([`Self::add_frame`],
/// [`Self::finalize`], [`Self::checkpoint`] lock once each); cheap [`Self::checkpoint`] writes a
/// partial tree during capture, [`Self::finalize`] does the one real root-combine at stop.
pub struct LiveIndex<'a> {
    d: PathBuf,
    client: &'a dyn Vision,
    preset: String,
    fuse_transcript: bool,
    model_label: Option<String>,
    code_max_px: u32,
    state: Mutex<State>,
}

impl<'a> LiveIndex<'a> {
    /// Construct a live index over `session_dir`. `preset` empty → `"auto"`.
    pub fn new(
        session_dir: &Path,
        client: &'a dyn Vision,
        preset: &str,
        fuse_transcript: bool,
        model_label: Option<&str>,
    ) -> Self {
        let preset = if preset.is_empty() { "auto" } else { preset }.to_string();
        LiveIndex {
            d: session_dir.to_path_buf(),
            client,
            preset,
            fuse_transcript,
            model_label: model_label.map(|s| s.to_string()),
            code_max_px: code_max_px_env(),
            state: Mutex::new(State {
                nodes: BTreeMap::new(),
                forest: Vec::new(),
                n: 0,
            }),
        }
    }

    /// Number of leaves appended so far.
    pub fn n(&self) -> usize {
        self.state.lock().unwrap().n
    }

    // -- building --------------------------------------------------------------

    /// The transcript segments (empty when transcript fusion is off). Mirrors `_segments`.
    fn segments(&self) -> Vec<Segment> {
        if self.fuse_transcript {
            load_transcript(&self.d)
        } else {
            Vec::new()
        }
    }

    /// Extract one frame into a leaf and merge it into the tree (one combine per power-of-2 carry).
    /// Mirrors `add_frame` — the never-recompute binary-counter merge.
    pub fn add_frame(&self, frame: &Frame, frame_end: f64) -> Result<(), String> {
        let (ctype, caption, data) =
            extract_leaf(self.client, &frame.path, &self.preset, self.code_max_px)?;
        let segments = self.segments();
        let mut st = self.state.lock().unwrap();
        let i = st.n;
        let tslice = transcript_slice(&segments, frame.offset, frame_end);
        let leaf = make_node(
            &format!("{i}-{i}"),
            0,
            i,
            i,
            frame,
            frame.offset,
            frame_end,
            1,
            ctype,
            Some(caption.clone()),
            tslice,
            caption,
            Vec::new(),
            Some(data),
        );
        st.nodes.insert(leaf.id.clone(), leaf.clone());
        st.n += 1;
        // Binary-counter merge: collapse equal-span right-edge subtrees into the carry.
        let mut carry = leaf;
        while st
            .forest
            .last()
            .map(|last| span(last) == span(&carry))
            .unwrap_or(false)
        {
            let left = st.forest.pop().unwrap();
            carry = self.combine(&mut st, &left, &carry, &segments);
        }
        st.forest.push(carry);
        Ok(())
    }

    /// Combine two consecutive subtrees into a parent node (one model `combine` call). The new node
    /// is recorded in `nodes`. Mirrors `_combine`.
    fn combine(&self, st: &mut State, left: &Node, right: &Node, segments: &[Segment]) -> Node {
        let lo = left.lo_idx;
        let hi = right.hi_idx;
        let t_lo = left.t_lo;
        let t_hi = right.t_hi.unwrap_or(right.t_lo);
        let tslice = transcript_slice(segments, t_lo, t_hi);
        let ctype = if left.content_type == right.content_type {
            left.content_type.clone()
        } else {
            "mixed".to_string()
        };
        let route = if ctype != "mixed" { ctype.as_str() } else { "general" };
        let focus = content_prompt(route).combine_focus;
        let feed = truncate_chars(&tslice, TRANSCRIPT_FEED_CAP);
        let summary = self
            .client
            .combine(&combine_prompt(&left.summary, &right.summary, &feed, &focus))
            .unwrap_or_default();
        let repr = repr_as_frame(&left.repr_frame);
        let node = make_node(
            &format!("{lo}-{hi}"),
            0,
            lo,
            hi,
            &repr,
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
        st.nodes.insert(node.id.clone(), node.clone());
        node
    }

    // -- snapshots -------------------------------------------------------------

    /// Return `(root_id, nodes_copy)`. `real=true` LLM-combines the forest into one root (finalize);
    /// `real=false` makes a CHEAP synthetic root (text join, no model call) for live checkpoints —
    /// neither pollutes the live `forest`/`nodes` used by ongoing merges. Mirrors `_materialize_root`.
    fn materialize_root(
        &self,
        st: &mut State,
        real: bool,
    ) -> Option<(String, BTreeMap<String, Node>)> {
        if st.forest.is_empty() {
            return None;
        }
        if st.forest.len() == 1 {
            return Some((st.forest[0].id.clone(), st.nodes.clone()));
        }
        if real {
            // Fold-combine the forest left→right into one real root. These nodes ARE kept (the spine).
            let segments = self.segments();
            let mut root = st.forest[0].clone();
            let rest: Vec<Node> = st.forest[1..].to_vec();
            for nxt in &rest {
                root = self.combine(st, &root, nxt, &segments);
            }
            return Some((root.id, st.nodes.clone()));
        }
        // Cheap synthetic root over the current forest (no model call), over a COPY of nodes so it
        // doesn't pollute the live forest.
        let mut nodes = st.nodes.clone();
        let lo = st.forest[0].lo_idx;
        let hi = st.forest[st.forest.len() - 1].hi_idx;
        let joined: String = st
            .forest
            .iter()
            .map(|f| truncate_chars(&f.summary, 120))
            .collect::<Vec<_>>()
            .join(" · ");
        let summary = format!("Live index (in progress): {joined}");
        let rid = format!("{lo}-{hi}~live");
        let last = &st.forest[st.forest.len() - 1];
        let t_hi = last.t_hi.unwrap_or(last.t_lo);
        let repr = repr_as_frame(&st.forest[0].repr_frame);
        let children: Vec<String> = st.forest.iter().map(|f| f.id.clone()).collect();
        let node = make_node(
            &rid,
            0,
            lo,
            hi,
            &repr,
            st.forest[0].t_lo,
            t_hi,
            hi - lo + 1,
            "mixed".to_string(),
            None,
            String::new(),
            summary,
            children,
            None,
        );
        nodes.insert(rid.clone(), node);
        Some((rid, nodes))
    }

    /// Stamp parents, flag code reliability, assemble + write `index.json` + `AGENTS.md`. Mirrors
    /// `_write`. The live params shape differs from batch (`sample_rate`/`max_leaves` null, plus
    /// `live: true`).
    fn write(&self, st: &State, root_id: &str, mut nodes: BTreeMap<String, Node>) -> Index {
        // Stamp parents (reset then link from each node's children).
        for nd in nodes.values_mut() {
            nd.parent = None;
        }
        let links: Vec<(String, String)> = nodes
            .values()
            .flat_map(|nd| {
                let pid = nd.id.clone();
                nd.children.iter().map(move |cid| (cid.clone(), pid.clone()))
            })
            .collect();
        for (cid, pid) in links {
            if let Some(child) = nodes.get_mut(&cid) {
                child.parent = Some(pid);
            }
        }
        // build's helpers take a HashMap; convert (assemble re-sorts by id internally).
        let mut map: std::collections::HashMap<String, Node> = nodes.into_iter().collect();
        flag_code_reliability(&mut map);
        let params = json!({
            "sample_rate": Value::Null,
            "max_leaves": Value::Null,
            "fuse_transcript": self.fuse_transcript,
            "prompt_preset": self.preset,
            "live": true,
        });
        let index = assemble(
            params,
            self.model_label.as_deref(),
            &map,
            root_id,
            st.n,
            map.len(),
        );
        write_index(&self.d, &index);
        write_agents_md(&self.d, &index, self.model_label.as_deref(), &map);
        index
    }

    /// Write a partial `index.json` + `AGENTS.md` mid-capture (cheap synthetic root). Mirrors
    /// `checkpoint`.
    pub fn checkpoint(&self) -> Option<Index> {
        let mut st = self.state.lock().unwrap();
        let (root_id, nodes) = self.materialize_root(&mut st, false)?;
        Some(self.write(&st, &root_id, nodes))
    }

    /// One real root-combine + a final `index.json`/`AGENTS.md` (the navigable, complete tree) +
    /// the prompts record. Mirrors `finalize`. `None` if no leaves were appended.
    pub fn finalize(&self) -> Option<Index> {
        let mut st = self.state.lock().unwrap();
        if st.nodes.is_empty() {
            return None;
        }
        let (root_id, nodes) = self.materialize_root(&mut st, true)?;
        let idx = self.write(&st, &root_id, nodes);
        // Write the prompts record (classify prompt only when the preset is auto/general).
        let map: std::collections::HashMap<String, Node> =
            st.nodes.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let classify = if self.preset == "auto" || self.preset == "general" {
            Some(classify_prompt())
        } else {
            None
        };
        write_prompts_record(
            &self.d,
            self.model_label.as_deref(),
            &self.preset,
            &map,
            st.n,
            classify,
            None,
            None,
            None,
        );
        eprintln!(
            "INFO capture-index::live: live-indexed {}: {} leaves, {} nodes",
            self.d.file_name().and_then(|s| s.to_str()).unwrap_or(""),
            st.n,
            st.nodes.len()
        );
        Some(idx)
    }
}

/// `span = hi_idx - lo_idx + 1`. Mirrors `_span`.
fn span(node: &Node) -> usize {
    node.hi_idx - node.lo_idx + 1
}

/// Drive a [`LiveIndex`] from a session's growing screenshots dir until `stop` is set, then
/// finalize. Samples every `round(1/sample_rate)`-th NEW frame (aligning with `select_leaves`),
/// keeping one behind the live edge so `frame_end` is known. Checkpoints every `checkpoint_every`
/// appended frames. Never panics out — logs and finalizes what it has so a flaky endpoint can't
/// break capture. Returns the finalized index, or `None` if there were no frames. Port of
/// `run_worker`.
#[allow(clippy::too_many_arguments)]
pub fn run_worker(
    session_dir: &Path,
    client: &dyn Vision,
    preset: &str,
    sample_rate: f64,
    fuse_transcript: bool,
    model_label: Option<&str>,
    stop: &AtomicBool,
    poll_seconds: f64,
    checkpoint_every: usize,
) -> Option<Index> {
    let live = LiveIndex::new(session_dir, client, preset, fuse_transcript, model_label);
    // step = max(1, round(1 / clamp(sample_rate, 1e-3, 1.0))).
    let rate = sample_rate.clamp(1e-3, 1.0);
    let step = ((1.0 / rate).round() as usize).max(1);
    let mut consumed = 0usize; // frames examined (sampled by `step`)
    let mut since_ckpt = 0usize;

    loop {
        let stopping = stop.load(Ordering::SeqCst);
        let all_frames = list_frames(session_dir);
        // Sample the not-yet-consumed tail; keep one behind the live edge so `frame_end` is known.
        let limit = if stopping {
            all_frames.len()
        } else {
            all_frames.len().saturating_sub(1)
        };
        while consumed < limit {
            let frame = &all_frames[consumed];
            if consumed % step == 0 {
                let frame_end = if consumed + 1 < all_frames.len() {
                    all_frames[consumed + 1].offset
                } else {
                    f64::INFINITY
                };
                match live.add_frame(frame, frame_end) {
                    Ok(()) => since_ckpt += 1,
                    Err(e) => eprintln!("WARN capture-index::live: add_frame failed (continuing): {e}"),
                }
            }
            consumed += 1;
            if since_ckpt >= checkpoint_every {
                since_ckpt = 0;
                live.checkpoint();
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs_f64(poll_seconds));
    }
    if live.n() > 0 {
        live.finalize()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering as O;

    /// A hermetic mock Vision — canned structured `{"summary":"s"}` + `"combined"` for combines.
    /// Counts its `combine` calls so the never-recompute property can be asserted.
    struct MockVision {
        structured_calls: AtomicUsize,
        combine_calls: AtomicUsize,
    }
    impl MockVision {
        fn new() -> Self {
            MockVision {
                structured_calls: AtomicUsize::new(0),
                combine_calls: AtomicUsize::new(0),
            }
        }
        fn combines(&self) -> usize {
            self.combine_calls.load(O::SeqCst)
        }
    }
    impl Vision for MockVision {
        fn caption_image(&self, _p: &Path, _prompt: &str, _max_px: Option<u32>) -> Result<String, String> {
            Ok("caption".to_string())
        }
        fn structured_image(
            &self,
            _path: &Path,
            prompt: &str,
            _schema: &Value,
            _max_px: Option<u32>,
        ) -> Result<Value, String> {
            self.structured_calls.fetch_add(1, O::SeqCst);
            if prompt.contains("Classify what this screenshot") {
                return Ok(json!({ "content_type": "general", "app": "Mock" }));
            }
            Ok(json!({ "summary": "s" }))
        }
        fn combine(&self, _prompt: &str) -> Result<String, String> {
            self.combine_calls.fetch_add(1, O::SeqCst);
            Ok("combined".to_string())
        }
    }

    /// Unique temp session dir with N screenshots + session.json + transcript.jsonl.
    fn mk_session(tag: &str, n: usize) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!("capture-index-live-{tag}-{nanos}"));
        fs::create_dir_all(d.join("screenshots")).unwrap();
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
        fs::write(
            d.join("transcript.jsonl"),
            "{\"start_offset\":0.0,\"end_offset\":100.0,\"text\":\"hello world 42 FooBar\"}\n",
        )
        .unwrap();
        d
    }

    /// The frames of a session (so a test can append them one by one with the right `frame_end`).
    fn frames_of(d: &Path) -> Vec<Frame> {
        list_frames(d)
    }

    /// popcount of a usize (number of set bits).
    fn popcount(mut x: usize) -> usize {
        let mut c = 0;
        while x != 0 {
            c += x & 1;
            x >>= 1;
        }
        c
    }

    #[test]
    fn binary_counter_shape_and_never_recompute() {
        // Append 4 leaves one by one; after each, the forest spans match N's binary representation,
        // and the cumulative combine count == N - popcount(N) (the never-recompute property).
        let n = 4;
        let d = mk_session("counter", n);
        let frames = frames_of(&d);
        let client = MockVision::new();
        let live = LiveIndex::new(&d, &client, "general", true, None);

        // After 1 leaf: forest spans [1]; binary(1) = 1 → one subtree of span 1. 0 combines.
        live.add_frame(&frames[0], frames[1].offset).unwrap();
        assert_eq!(forest_spans(&live), vec![1], "after 1 → [1]");
        assert_eq!(client.combines(), 1 - popcount(1), "1 combines = 1 - popcount(1) = 0");

        // After 2 leaves: forest spans [2]; binary(2)=10. 1 combine total.
        live.add_frame(&frames[1], frames[2].offset).unwrap();
        assert_eq!(forest_spans(&live), vec![2], "after 2 → [2]");
        assert_eq!(client.combines(), 2 - popcount(2), "= 2 - 1 = 1");

        // After 3 leaves: forest spans [2,1]; binary(3)=11. still 1 combine total.
        live.add_frame(&frames[2], frames[3].offset).unwrap();
        assert_eq!(forest_spans(&live), vec![2, 1], "after 3 → [2,1]");
        assert_eq!(client.combines(), 3 - popcount(3), "= 3 - 2 = 1");

        // After 4 leaves: forest spans [4]; binary(4)=100. 3 combines total.
        live.add_frame(&frames[3], f64::INFINITY).unwrap();
        assert_eq!(forest_spans(&live), vec![4], "after 4 → [4]");
        assert_eq!(client.combines(), 4 - popcount(4), "= 4 - 1 = 3 (never-recompute)");

        fs::remove_dir_all(&d).ok();
    }

    /// The spans of the live forest (read through the lock).
    fn forest_spans(live: &LiveIndex) -> Vec<usize> {
        let st = live.state.lock().unwrap();
        st.forest.iter().map(span).collect()
    }

    #[test]
    fn finalize_single_root_and_artifacts() {
        let n = 4;
        let d = mk_session("finalize", n);
        let frames = frames_of(&d);
        let client = MockVision::new();
        let live = LiveIndex::new(&d, &client, "general", true, None);
        for i in 0..n {
            let end = if i + 1 < n { frames[i + 1].offset } else { f64::INFINITY };
            live.add_frame(&frames[i], end).unwrap();
        }
        let idx = live.finalize().expect("finalize ok");

        // A single real root over [0..N-1], total real nodes = 2N-1.
        assert_eq!(idx.root_id, format!("0-{}", n - 1), "root spans the whole timeline");
        assert_eq!(idx.nodes.len(), 2 * n - 1, "real node count = 2N-1");
        assert_eq!(idx.node_count, 2 * n - 1);
        assert_eq!(idx.leaf_count, n);
        assert!(idx.complete, "complete (node count matches)");
        assert_eq!(idx.params["live"], json!(true), "live params flag");
        assert_eq!(idx.params["sample_rate"], Value::Null);
        assert_eq!(idx.params["max_leaves"], Value::Null);

        // Exactly one root (no parent), and it is the root_id; parent links set on the rest.
        let roots: Vec<&Node> = idx.nodes.iter().filter(|nd| nd.parent.is_none()).collect();
        assert_eq!(roots.len(), 1, "exactly one root");
        assert_eq!(roots[0].id, idx.root_id);
        for nd in &idx.nodes {
            if !nd.children.is_empty() {
                assert_eq!(nd.children.len(), 2, "internal has 2 children");
                for cid in &nd.children {
                    let child = idx.nodes.iter().find(|c| &c.id == cid).unwrap();
                    assert_eq!(child.parent.as_deref(), Some(nd.id.as_str()), "parent link");
                }
            }
        }

        // index.json on disk says complete + has the live root; AGENTS.md + index_prompts.json written.
        assert!(d.join("index.json").is_file());
        let on_disk: Value =
            serde_json::from_str(&fs::read_to_string(d.join("index.json")).unwrap()).unwrap();
        assert_eq!(on_disk["complete"], json!(true));
        assert_eq!(on_disk["root_id"], json!(format!("0-{}", n - 1)));
        assert_eq!(on_disk["node_count"], json!(2 * n - 1));
        assert!(d.join("AGENTS.md").is_file());
        assert!(d.join("index_prompts.json").is_file());

        fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn checkpoint_synthetic_root_no_extra_model_call() {
        // Append 3 → forest [2,1] (one real combine). Checkpoint makes a ~live synthetic root with
        // NO model call, so combine count stays at the 1 real combine.
        let n = 3;
        let d = mk_session("ckpt", n);
        let frames = frames_of(&d);
        let client = MockVision::new();
        let live = LiveIndex::new(&d, &client, "general", true, None);
        for i in 0..n {
            let end = if i + 1 < n { frames[i + 1].offset } else { f64::INFINITY };
            live.add_frame(&frames[i], end).unwrap();
        }
        let combines_before = client.combines();
        assert_eq!(combines_before, 1, "3 appends did exactly 1 real combine");

        let idx = live.checkpoint().expect("checkpoint ok");
        assert_eq!(
            client.combines(),
            combines_before,
            "checkpoint makes NO extra model call"
        );
        // The synthetic root id is "{lo}-{hi}~live" and is content_type "mixed".
        assert!(idx.root_id.ends_with("~live"), "synthetic root id: {}", idx.root_id);
        assert_eq!(idx.root_id, format!("0-{}~live", n - 1));
        let root = idx.nodes.iter().find(|nd| nd.id == idx.root_id).unwrap();
        assert_eq!(root.content_type, "mixed");
        assert!(root.summary.starts_with("Live index (in progress): "));
        // index.json exists with the synthetic root.
        let on_disk: Value =
            serde_json::from_str(&fs::read_to_string(d.join("index.json")).unwrap()).unwrap();
        assert_eq!(on_disk["root_id"], json!(format!("0-{}~live", n - 1)));

        fs::remove_dir_all(&d).ok();
    }
}
