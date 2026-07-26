//! The six MCP tool payloads (T3.5), built to the v1 contract
//! (`docs/payload-schema.md`) and enforced by `scripts/check_payloads.py`.
//!
//! Compact JSON, hard token caps, `truncated` markers. The tool returns
//! evidence; the connected agent narrates. Every payload carries the ADR A4
//! provenance in `session.identified_by`.

use crate::model::{Action, ActionKind, Finding, Idx, Session};
use crate::report::Overview;
use crate::score::FileScore;
use serde_json::{Value, json};

/// Token-cap headroom uses chars/3.5 (compact JSON tokenizes hot).
/// Same divisor as `scripts/check_payloads.py`, so the checker and the code
/// count tokens the same way and can never disagree about "under cap".
const CHARS_PER_TOKEN: f64 = 3.5;

// The advertised budgets (`docs/payload-schema.md`). Each builder shrinks its
// own output until it fits, so the cap holds for ANY session (a 4 KB path, a
// finding with 5000 proving idxs, 500 distinct event types), not just tidy
// ones. Enforced by construction (ADR A5), never hoped about.
/// `session_overview` budget.
const CAP_OVERVIEW: usize = 1000;
/// `struggle_areas` budget.
const CAP_STRUGGLE: usize = 1500;
/// `file_story` budget.
const CAP_STORY: usize = 1500;
/// `blind_spots` budget.
const CAP_BLIND: usize = 1000;
/// `context_health` budget.
const CAP_HEALTH: usize = 1000;
/// `evidence` budget.
const CAP_EVIDENCE: usize = 1500;

/// Max findings shown per file in `struggle_areas`.
const FINDINGS_PER_FILE: usize = 4;
/// Hard ceiling on `struggle_areas(n)`: bigger asks are clamped, not honored.
/// 20 files is already more than the 1500-token budget can render, so this is
/// an argument-sanity guard, not the thing that keeps the payload small.
pub const STRUGGLE_FILES_MAX: usize = 20;
/// Max actions returned by `evidence`.
const EVIDENCE_MAX: usize = 10;
/// Max excerpt chars per evidence action.
const EXCERPT_MAX: usize = 600;
/// `file_story` keeps this many head and tail events, eliding the middle.
const STORY_EDGE: usize = 8;
/// Longest file path echoed in any payload. POSIX allows 4096, and one path
/// that long is ~1170 tokens: more than the whole `session_overview` budget.
const PATH_MAX: usize = 160;
/// Longest session id echoed. Real ids are 36-char uuids and the filename
/// they come from is OS-capped at 255 bytes, so this only fires on something
/// pathological, but "only fires on pathological input" is exactly what a
/// by-construction cap has to cover.
const SESSION_ID_MAX: usize = 120;
/// Longest timestamp string echoed (ISO-8601 is ~24 chars).
const TS_MAX: usize = 40;
/// Max proving `idxs` echoed per finding. `evidence()` dereferences at most
/// `EVIDENCE_MAX` of them anyway, so 10 loses nothing a caller could use.
const FINDING_IDXS_MAX: usize = 10;
/// Starting per-list cap in `blind_spots` (shrunk further when needed).
const BLIND_LIST_MAX: usize = 8;
/// Starting cap on distinct unknown event types listed in `session_overview`.
const UNKNOWN_TYPES_MAX: usize = 8;

/// Session identity + how it was resolved (ADR A4 provenance).
pub struct SessionMeta {
    /// Session id.
    pub id: String,
    /// `tool_use_id` | `explicit` | `cli_latest`.
    pub identified_by: String,
}

/// Approximate token count of a serialized payload.
pub fn est_tokens(v: &Value) -> usize {
    (v.to_string().len() as f64 / CHARS_PER_TOKEN).ceil() as usize
}

/// Cut the middle out of an over-long string, stating inline how much went.
///
/// WHY a marker instead of a plain cut: this codebase never drops data
/// silently. `file_story` already discloses its dropped events with an
/// `elided` count; this is the same promise applied to a single string. The
/// result is also unmistakably not a real path, so a caller can't feed it
/// back into `file_story` and wonder why the story is empty.
fn elide_middle(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max {
        return s.to_string();
    }
    // Keep both ends: for a path the tail (the file name) identifies it and
    // the head (the project root) locates it. Middle-out, same as `file_story`.
    let keep = max / 2;
    let head: String = s.chars().take(keep).collect();
    let tail: String = s.chars().skip(n - keep).collect();
    format!("{head}…[{} chars elided]…{tail}", n - 2 * keep)
}

/// Would `elide_middle` cut this string? Lets a builder flip `truncated`.
fn would_elide(s: &str, max: usize) -> bool {
    s.chars().count() > max
}

/// The `session` envelope block with the id capped (see `SESSION_ID_MAX`).
/// Returns the block plus "did the id have to be cut", which every builder
/// folds into its `truncated` flag.
fn session_block(meta: &SessionMeta) -> (Value, bool) {
    let cut = would_elide(&meta.id, SESSION_ID_MAX);
    let block = json!({
        "id": elide_middle(&meta.id, SESSION_ID_MAX),
        "identified_by": meta.identified_by
    });
    (block, cut)
}

/// Shrink until it fits: call `build(k)` with `k` walking down from `start`
/// to 0 and return the first payload inside `cap`.
///
/// This generalizes the loop `evidence()` has always used. The cap is held by
/// REBUILDING SMALLER, never by assuming the content was small. Two rules the
/// caller's `build` must satisfy: it shrinks monotonically as `k` falls, and
/// at `k == 0` all that is left is fixed-shape scalars and capped strings
/// (which is why paths, ids and timestamps are capped before we get here).
/// That pair is what makes the guarantee total rather than probable.
fn shrink_to_fit(cap: usize, start: usize, build: impl Fn(usize) -> Value) -> Value {
    let mut k = start;
    loop {
        let payload = build(k);
        if k == 0 || est_tokens(&payload) <= cap {
            return payload;
        }
        k -= 1;
    }
}

/// One finding rendered for a payload, with its two unbounded fields capped.
///
/// `idxs` is the dangerous one: a churn finding on a file edited 800 times
/// carries 800 indices (~4 KB of JSON on its own), and a `review_burden`
/// finding's idxs span every edit in a segment. We keep the first
/// `FINDING_IDXS_MAX` (head-kept, tail-dropped: the rule the schema
/// advertises for finding lists) and state the true length in `idxs_total`
/// so the count is never lost, only the list.
fn compact_finding(f: &Finding) -> Value {
    // Serializing a Finding is infallible; the fallback keeps this a library
    // function that cannot panic on a caller's data.
    let mut v = serde_json::to_value(f).unwrap_or_else(|_| json!({}));
    let Some(obj) = v.as_object_mut() else {
        return v;
    };
    if f.idxs.len() > FINDING_IDXS_MAX {
        obj.insert("idxs".into(), json!(&f.idxs[..FINDING_IDXS_MAX]));
        obj.insert("idxs_total".into(), json!(f.idxs.len()));
    }
    if let Some(p) = f.file.as_deref().filter(|p| would_elide(p, PATH_MAX)) {
        obj.insert("file".into(), json!(elide_middle(p, PATH_MAX)));
    }
    v
}

/// Did `compact_finding` have to drop anything from this finding?
fn finding_was_capped(f: &Finding) -> bool {
    f.idxs.len() > FINDING_IDXS_MAX || f.file.as_deref().is_some_and(|p| would_elide(p, PATH_MAX))
}

/// Pick up to `k` of a file's findings so the kept set MIRRORS THE SCORE
/// BREAKDOWN instead of detector emission order.
///
/// WHY: `score::all_findings` runs edit_shape → failures → dynamics →
/// comprehension, so the old `take(FINDINGS_PER_FILE)` handed every slot to
/// whatever the first detector emitted. A file scored on rework *and* failure
/// loops *and* blind writes showed four rework findings and no failure
/// evidence, even though its own `breakdown` said failures scored.
///
/// The rule: **round-robin over the scoring categories, most alarming first**
/// (`review::SEVERITY_ORDER`, the same fixed order the report lists reasons
/// in). Every category that contributed to the score gets one finding before
/// any category gets a second. Inside a category the detector's own order
/// (chronological) is preserved and the tail is what drops, which is the
/// tail-first rule the schema already advertises for this payload.
fn representative_findings(fs: &FileScore, k: usize) -> Vec<&Finding> {
    let order = crate::review::SEVERITY_ORDER;
    // One bucket per severity rank, plus a trailing bucket for anything
    // unranked. `score::rank` never puts an unranked finding in a FileScore,
    // but if that ever changes the finding sorts last instead of vanishing.
    let mut buckets: Vec<Vec<&Finding>> = vec![Vec::new(); order.len() + 1];
    for f in &fs.findings {
        let slot = crate::score::ranked_category(f)
            .and_then(|(c, _)| order.iter().position(|s| *s == c))
            .unwrap_or(order.len());
        buckets[slot].push(f);
    }
    let mut out: Vec<&Finding> = Vec::with_capacity(k);
    for round in 0.. {
        let mut progressed = false;
        for b in &buckets {
            if out.len() == k {
                return out;
            }
            if let Some(f) = b.get(round) {
                out.push(f);
                progressed = true;
            }
        }
        // Every bucket is exhausted: fewer findings than slots.
        if !progressed {
            return out;
        }
    }
    out
}

/// `session_overview()` — totals + top-3 struggle files.
///
/// Fixed shape, but two of its fields are caller-controlled and were
/// unbounded: `top_struggles` echoes file paths (POSIX allows 4 KB each) and
/// `flags.unknown_event_types` echoes every unmodeled `type` string in the
/// transcript (a 500-type transcript measured ~6450 tokens, 6× the budget).
/// Paths are capped, the type map is sampled most-frequent-first, and both
/// totals stay in the payload so nothing disappears quietly.
pub fn session_overview(s: &Session, ranked: &[FileScore], meta: &SessionMeta) -> Value {
    let o = Overview::from_session(s);
    // The overview's `session` block carries two extra fields, so it caps the
    // id itself instead of reusing `session_block`.
    let id = elide_middle(&meta.id, SESSION_ID_MAX);
    let id_cut = would_elide(&meta.id, SESSION_ID_MAX);
    let paths_cut = ranked
        .iter()
        .take(3)
        .any(|f| would_elide(&f.file, PATH_MAX));
    let top: Vec<Value> = ranked
        .iter()
        .take(3)
        .map(|f| {
            json!({
                "file": elide_middle(&f.file, PATH_MAX),
                "class": f.class, "edits": f.edits,
                "breakdown": f.breakdown
            })
        })
        .collect();
    // `type_counts` is the FULL histogram; a flag named "unknown" must not
    // list the types ingest models (Checkpoint D finding — "assistant: 132"
    // under "unknown" reads as a parser bug). Sorted most-frequent-first,
    // ties by name, so the sample we keep is the informative one and the
    // choice is deterministic.
    let mut unknown: Vec<(&String, &u64)> = s
        .type_counts
        .iter()
        .filter(|(t, _)| !matches!(t.as_str(), "assistant" | "user"))
        .collect();
    unknown.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    // Wall-clock span, first→last action. The debrief contract opens with a
    // duration, so the overview must carry one (mock contract's `started` +
    // `duration_min`; both null when the session has no parseable timestamps).
    let started = o
        .span
        .as_ref()
        .map(|(first, _)| elide_middle(first, TS_MAX));
    let duration_min = o.span.as_ref().and_then(|(first, last)| {
        match (crate::report::ts_secs(first), crate::report::ts_secs(last)) {
            (Some(a), Some(b)) if b >= a => Some((b - a) / 60),
            _ => None,
        }
    });
    // The only knob left is how many unknown event types we list; everything
    // else in this payload is a scalar or an already-capped string.
    shrink_to_fit(CAP_OVERVIEW, UNKNOWN_TYPES_MAX.min(unknown.len()), |k| {
        let shown: std::collections::BTreeMap<&str, &u64> = unknown
            .iter()
            .take(k)
            .map(|(t, c)| (t.as_str(), *c))
            .collect();
        json!({
            "v": 1,
            "session": {
                "id": id, "identified_by": meta.identified_by,
                "started": started, "duration_min": duration_min
            },
            "totals": {
                // `file_ops` (edits+writes) and `lines_written` lead: `edits` alone
                // omits Write, and any tool-call count omits the volume of change.
                // The edits/writes split is kept for consumers that want it.
                "actions": o.actions, "file_ops": o.file_ops,
                "lines_written": o.lines_written,
                "file_ops_unconfirmed": o.file_ops_unconfirmed,
                "edits": o.edits, "writes": o.writes,
                "reads": o.reads, "bash": o.bash, "files_touched": o.files_touched,
                "interrupts": s.interrupts
            },
            "tokens": {
                "output": o.output_tokens, "cache_read": o.cache_read_tokens,
                "cache_hit_ratio": o.cache_hit_ratio.map(round2)
            },
            "top_struggles": top,
            // How many files ranked at all. `top_struggles` shows 3; without
            // this the caller cannot tell 3-of-3 from 3-of-40.
            "top_struggles_total": ranked.len(),
            // Session roll-up of the per-segment opening moves (metrics-spec #9):
            // share of classified task segments that opened patch-first. Omitted
            // (null) when no segment was big enough to classify.
            "patch_first_segment_share":
                crate::signals::dynamics::patch_first_segment_share(s).map(round2),
            "flags": {
                "unknown_event_types": shown,
                // Distinct unmodeled types SEEN, which is what the sampled map
                // above can no longer tell you on its own.
                "unknown_event_types_total": unknown.len(),
                "parse_errors": o.parse_errors,
                "untimestamped_lines": o.untimestamped_lines,
                // Honest scope disclosure: subagent spawns we could not turn into
                // analyzed actions (missing/unreadable/empty child transcript, or
                // over the file-count cap). `0` once every spawn's work merged in.
                "subagent_files_missing": s.subagent_files_missing
            },
            // `ranked.len() > 3` is deliberately NOT a truncation trigger:
            // `top_struggles` is defined as the top 3, so showing 3 of 40 is
            // the field's shape rather than a cap that kicked in, and
            // `top_struggles_total` already discloses the real count. Firing
            // on it made `truncated` true for half of all real sessions,
            // which buries the case this flag exists for: the shrink loop
            // below actually dropping content.
            "truncated": unknown.len() > k || paths_cut || id_cut
        })
    })
}

/// `struggle_areas(n)`: ranked files with breakdown, ranking rule, findings.
///
/// Three caps stack, in the order the schema advertises (tail-first):
/// `n` is clamped to `STRUGGLE_FILES_MAX`, findings per file are capped and
/// chosen to represent the breakdown (`representative_findings`), and then
/// the payload is rebuilt smaller until it fits `CAP_STRUGGLE`: lowest-ranked
/// files dropped first, and only once a single file is left do its findings
/// start going. Measured before this existed: `n=99` on an ordinary 12-file
/// session produced 2827 tokens, and a 200-file session 1.6M.
pub fn struggle_areas(ranked: &[FileScore], meta: &SessionMeta, n: usize) -> Value {
    // `n` arrives straight from an MCP caller and was honored verbatim.
    let n = n.min(STRUGGLE_FILES_MAX);
    let (session, id_cut) = session_block(meta);

    let build = |files_k: usize, per_file: usize| -> Value {
        let shown = &ranked[..files_k.min(ranked.len())];
        let files: Vec<Value> = shown
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let kept = representative_findings(f, per_file);
                let mut entry = json!({
                    "rank": i + 1, "file": elide_middle(&f.file, PATH_MAX),
                    "class": f.class, "edits": f.edits,
                    "breakdown": f.breakdown,
                    "findings": kept.iter().map(|f| compact_finding(f)).collect::<Vec<_>>()
                });
                // Same disclosure contract as `file_story`'s `elided` count:
                // the caller always learns how many findings it is NOT seeing.
                let omitted = f.findings.len() - kept.len();
                if omitted > 0 {
                    entry["findings_omitted"] = json!(omitted);
                }
                entry
            })
            .collect();
        let content_cut = ranked.len() > shown.len()
            || shown.iter().any(|f| f.findings.len() > per_file)
            || shown.iter().any(|f| would_elide(&f.file, PATH_MAX))
            || shown
                .iter()
                .flat_map(|f| &f.findings)
                .any(finding_was_capped);
        json!({
            "v": 1,
            "session": session,
            "ranking_rule": crate::score::RANKING_RULE,
            "files": files,
            // How many files ranked in total, so a clamped or shrunk list is
            // legible as "20 of 200" rather than looking complete.
            "files_total": ranked.len(),
            "n_max": STRUGGLE_FILES_MAX,
            // The cap ACTUALLY applied, which the size loop may have lowered.
            "findings_per_file_cap": per_file,
            "truncated": content_cut || id_cut
        })
    };

    // Two knobs, shrunk in the documented order: drop the lowest-ranked files
    // first (tail-first), and only when one file is left start dropping its
    // findings. With paths capped and findings at 0, one file is a handful of
    // scalars, so this loop always terminates under the cap.
    let mut files_k = n.min(ranked.len());
    let mut per_file = FINDINGS_PER_FILE;
    loop {
        let payload = build(files_k, per_file);
        if est_tokens(&payload) <= CAP_STRUGGLE {
            return payload;
        }
        if files_k > 1 {
            files_k -= 1;
        } else if per_file > 0 {
            per_file -= 1;
        } else {
            return payload; // nothing left to drop
        }
    }
}

/// `file_story(path)` — chronological events for one file, elided middle-out.
///
/// Event count was already bounded at `2 * STORY_EDGE`, but the echoed `path`
/// was not: one 4 KB path alone measured 1510 tokens, past the 1500 budget
/// before a single event was rendered. The path is capped and the head/tail
/// edge shrinks if that is somehow still not enough.
pub fn file_story(s: &Session, path: &str, meta: &SessionMeta) -> Value {
    let events: Vec<&Action> = s
        .actions
        .iter()
        .filter(|a| a.file_path.as_deref() == Some(path))
        .collect();
    let (session, id_cut) = session_block(meta);
    let file = elide_middle(path, PATH_MAX);
    let path_cut = would_elide(path, PATH_MAX);
    let render = |a: &Action| {
        json!({
            "idx": a.idx, "t": elide_middle(&a.effective_ts, TS_MAX),
            "action": kind_str(&a.kind),
            "outcome": a.is_error.map(|e| if e {"fail"} else {"ok"})
        })
    };
    shrink_to_fit(CAP_STORY, STORY_EDGE, |edge| {
        // Middle-out, the rule this payload advertises: keep `edge` events at
        // each end and say how many went from between them.
        let (head, tail, elided) = if events.len() > 2 * edge {
            let head: Vec<Value> = events[..edge].iter().map(|a| render(a)).collect();
            let tail: Vec<Value> = events[events.len() - edge..]
                .iter()
                .map(|a| render(a))
                .collect();
            let between = json!({
                "count": events.len() - 2 * edge,
                "note": "middle events elided; fetch via evidence(idxs)"
            });
            (head, tail, Some(between))
        } else {
            (events.iter().map(|a| render(a)).collect(), Vec::new(), None)
        };
        json!({
            "v": 1,
            "session": session,
            "file": file,
            "events": head,
            "elided": elided,
            "tail": tail,
            "truncated": elided.is_some() || path_cut || id_cut
        })
    })
}

/// `blind_spots()` — blind-write attempts, review burden, approval outliers.
///
/// Every matching finding used to be emitted in full with `truncated: false`.
/// An ordinary 300-edit session measured 3166 tokens against a 1000 budget,
/// and one `review_burden` finding can carry every edit index in its segment.
/// Now each list is tail-truncated to the SAME cap `k`, shrinking until the
/// payload fits, with the true counts kept in `totals`.
///
/// WHY one shared `k` rather than a global budget spent list by list: the
/// three lists are different KINDS of blind spot, and a session with 2000
/// blind-write attempts would otherwise spend the whole payload on them and
/// push `review_burden` out entirely. Review burden is the metric this file
/// promises never to suppress, so crowding it out would break that promise by
/// the back door. Equal caps keep every category visible.
pub fn blind_spots(s: &Session, meta: &SessionMeta) -> Value {
    use crate::model::FindingKind;
    let all = crate::score::all_findings(s);
    let of_kind =
        |kind: FindingKind| -> Vec<&Finding> { all.iter().filter(|f| f.kind == kind).collect() };
    let blind = of_kind(FindingKind::BlindWriteAttempt);
    // The comprehension-layer anchor (metrics-spec #27): agent LOC per human
    // turn vs the 200–400 LOC review band. Never suppressed — it is exactly
    // the auto-accept mode where this matters most.
    let burden = of_kind(FindingKind::ReviewBurden);
    let outliers = of_kind(FindingKind::LargeWriteInstantAccept);
    let (session, id_cut) = session_block(meta);
    let findings_cut = blind
        .iter()
        .chain(&burden)
        .chain(&outliers)
        .any(|f| finding_was_capped(f));
    let longest = blind.len().max(burden.len()).max(outliers.len());
    // Start at the full cap even when the lists are shorter, so `list_cap`
    // reports the cap that was in force rather than "however many I had".
    shrink_to_fit(CAP_BLIND, BLIND_LIST_MAX, |k| {
        let list = |v: &[&Finding]| -> Vec<Value> {
            v.iter().take(k).map(|f| compact_finding(f)).collect()
        };
        json!({
            "v": 1,
            "session": session,
            "blind_write_attempts": list(&blind),
            "review_burden": list(&burden),
            "approval_outliers": list(&outliers),
            // Full counts, always present: the lists above are a sample, and
            // "2 shown" must never be mistaken for "2 happened".
            "totals": {
                "blind_write_attempts": blind.len(),
                "review_burden": burden.len(),
                "approval_outliers": outliers.len()
            },
            "list_cap": k,
            "suppression": {
                "approval_latency": if crate::signals::comprehension::approval_latency_active(s) {"active"} else {"suppressed"},
                "suppressed_when": "permissionMode grants auto-accept",
                "review_burden": "never suppressed"
            },
            "truncated": longest > k || findings_cut || id_cut
        })
    })
}

/// `context_health()` — cache ratio and token economics (informational only).
///
/// The one payload bounded by its SHAPE: every field is a fixed key holding a
/// number, a fixed prose note, or the session id. Once the id is capped
/// (`SESSION_ID_MAX`) no input can grow it, and there is nothing to sample:
/// the schema's "`read_never_referenced` sampled" rule describes a list the
/// shipped builder does not emit (`read_unreferenced` is a mock-only finding
/// kind); when it lands it must arrive with its own cap and total.
///
/// It still goes through `shrink_to_fit`, with the prose `note` as the single
/// droppable item, so that the guarantee is enforced by the same machinery as
/// the other five rather than by an argument in a comment that a later field
/// could quietly invalidate.
pub fn context_health(s: &Session, meta: &SessionMeta) -> Value {
    let o = Overview::from_session(s);
    let (session, id_cut) = session_block(meta);
    shrink_to_fit(CAP_HEALTH, 1, |keep_note| {
        json!({
            "v": 1,
            "session": session,
            "cache_hit_ratio": o.cache_hit_ratio.map(round2),
            "tokens": {
                "output": s.tokens.output, "input_uncached": s.tokens.input,
                "cache_read": s.tokens.cache_read, "cache_creation": s.tokens.cache_creation
            },
            // Localization dispersion (metrics-spec #28): distinct files read per
            // distinct file edited. Informational only in v0.1 — TRAJEVAL's ~22×
            // over-read baseline needs gold patches, and a personal cross-session
            // baseline (the v2 seam) is what would make "unusually dispersed"
            // meaningful. Omitted (null) for sessions that edited nothing:
            // a read-only session's ratio carries no localization signal.
            "read_edit_file_ratio": read_edit_file_ratio(s).map(round2),
            "note": if keep_note > 0 {
                "v0.1 reports context economics as information only; token-level waste deferred"
            } else { "" },
            // Reported rather than hard-coded false: this payload must not
            // claim "nothing was cut" when the id or the note was.
            "truncated": id_cut || keep_note == 0
        })
    })
}

/// Distinct-files-read : distinct-files-edited, `None` when nothing was edited.
fn read_edit_file_ratio(s: &Session) -> Option<f64> {
    use crate::model::ActionKind;
    let distinct = |want: fn(&ActionKind) -> bool| {
        s.actions
            .iter()
            .filter(|a| want(&a.kind))
            .filter_map(|a| a.file_path.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    };
    let edited = distinct(|k| matches!(k, ActionKind::Edit | ActionKind::Write));
    if edited == 0 {
        return None;
    }
    let read = distinct(|k| matches!(k, ActionKind::Read));
    Some(read as f64 / edited as f64)
}

/// `evidence(idxs)` — raw actions behind findings, capped.
///
/// Three caps stack here: ≤`EVIDENCE_MAX` actions, ≤`EXCERPT_MAX` chars per
/// excerpt, and the 1500-token payload cap. Ten dense excerpts can bust the
/// token cap even inside the first two, so the payload is shrunk tail-first
/// until it fits — the cap is enforced by construction (ADR A5), not hoped
/// about.
pub fn evidence(s: &Session, idxs: &[Idx], meta: &SessionMeta) -> Value {
    let (session, id_cut) = session_block(meta);
    let mut found = Vec::new();
    let mut not_found = Vec::new();
    for &Idx(i) in idxs.iter().take(EVIDENCE_MAX) {
        match s.actions.get(i as usize) {
            Some(a) => found.push(json!({
                "idx": a.idx, "t": elide_middle(&a.effective_ts, TS_MAX),
                "tool": kind_str(&a.kind),
                // Capped like every other echoed path, so one 4 KB path can't
                // evict the excerpts the caller actually asked for.
                "file": a.file_path.as_deref().map(|p| elide_middle(p, PATH_MAX)),
                "excerpt": excerpt(a)
            })),
            None => not_found.push(i),
        }
    }
    let mut dropped_for_cap = false;
    loop {
        let payload = json!({
            "v": 1,
            "session": session,
            "actions": found,
            "not_found": not_found,
            "caps": {"max_actions": EVIDENCE_MAX, "max_excerpt_chars": EXCERPT_MAX},
            "truncated": idxs.len() > EVIDENCE_MAX || dropped_for_cap || id_cut
        });
        // Under cap, or nothing left to drop (a pathological single excerpt
        // still fits: 600 chars ≈ 172 tokens) — done either way.
        if est_tokens(&payload) <= CAP_EVIDENCE || found.is_empty() {
            return payload;
        }
        found.pop();
        dropped_for_cap = true;
    }
}

fn excerpt(a: &Action) -> String {
    let raw = a
        .command
        .as_deref()
        .or(a.error.as_deref())
        .or(a.edit_new.as_deref())
        .unwrap_or("");
    let capped: String = raw.chars().take(EXCERPT_MAX).collect();
    // ADR A9(4): everything excerpted to a caller passes the redaction pass.
    // Redact AFTER capping so a PEM block cut by the cap still redacts to
    // the excerpt's end (the truncated-block case redact() handles).
    crate::redact::redact(&capped)
}

fn kind_str(k: &ActionKind) -> String {
    match k {
        ActionKind::Read => "Read".into(),
        ActionKind::Edit => "Edit".into(),
        ActionKind::Write => "Write".into(),
        ActionKind::Bash => "Bash".into(),
        ActionKind::Other(n) => n.clone(),
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::ingest_str;
    use crate::model::Lane;
    use crate::score::rank;

    fn meta() -> SessionMeta {
        SessionMeta {
            id: "abc".into(),
            identified_by: "explicit".into(),
        }
    }

    fn busy_session() -> Session {
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/a.ts","new_string":"x"}}}}]}}}}"#
            ));
        }
        ingest_str(&lines.join("\n"), Lane::Main)
    }

    /// One Edit action line, keyed by tool_use id and timestamp.
    fn edit(id: &str, ts: &str, file: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"content":[{{"type":"tool_use","id":"{id}","name":"Edit","input":{{"file_path":"{file}","new_string":"x"}}}}]}}}}"#
        )
    }

    /// Two same-class files with different edit counts: enough to rank.
    fn churny_session() -> crate::model::Session {
        let mut lines = Vec::new();
        for i in 0..4 {
            lines.push(edit(
                &format!("h{i}"),
                &format!("2026-01-01T00:00:0{i}Z"),
                "/a/hot.rs",
            ));
        }
        for i in 0..2 {
            lines.push(edit(
                &format!("w{i}"),
                &format!("2026-01-01T00:01:0{i}Z"),
                "/a/warm.rs",
            ));
        }
        crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main)
    }

    #[test]
    fn unknown_event_types_excludes_modeled_types() {
        // "assistant" and "user" are the two line types ingest actually
        // models — a flag named "unknown_event_types" listing them is lying
        // (Checkpoint D live finding, 2026-07-20). Truly unmodeled types
        // ("queue-operation" here) must still appear.
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/a"}}]}}"#,
            "\n",
            r#"{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":false}]}}"#,
            "\n",
            r#"{"type":"queue-operation","timestamp":"2026-01-01T00:00:02Z"}"#,
        );
        let s = ingest_str(raw, Lane::Main);
        let p = session_overview(&s, &[], &meta());
        let unknown = p["flags"]["unknown_event_types"].as_object().unwrap();
        assert!(
            !unknown.contains_key("assistant") && !unknown.contains_key("user"),
            "modeled types must not be reported as unknown: {unknown:?}"
        );
        assert_eq!(unknown["queue-operation"], 1);
    }

    #[test]
    fn dispersion_ratio_reads_over_edits() {
        // 10 distinct files read, 2 distinct edited ⇒ 5.0
        let mut lines = Vec::new();
        for i in 0..10 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:{i:02}Z","message":{{"content":[{{"type":"tool_use","id":"r{i}","name":"Read","input":{{"file_path":"/f{i}.rs"}}}}]}}}}"#
            ));
        }
        for i in 0..2 {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:{i:02}Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"/f{i}.rs","new_string":"x"}}}}]}}}}"#
            ));
        }
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let p = context_health(&s, &meta());
        assert_eq!(p["read_edit_file_ratio"], 5.0);
    }

    #[test]
    fn dispersion_ratio_omitted_for_read_only_sessions() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"r0","name":"Read","input":{"file_path":"/f.rs"}}]}}"#;
        let s = ingest_str(raw, Lane::Main);
        let p = context_health(&s, &meta());
        assert!(
            p["read_edit_file_ratio"].is_null(),
            "no edits ⇒ ratio is no localization signal"
        );
    }

    #[test]
    fn overview_reports_started_and_duration_min() {
        // The debrief output contract opens with a duration; the mock contract
        // has always promised `session.started` + `session.duration_min`, but
        // the shipped builder never emitted them (codex review 2026-07-22 P0).
        let raw = concat!(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"e0","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            "\n",
            r#"{"type":"assistant","timestamp":"2026-01-01T00:30:00Z","message":{"content":[{"type":"tool_use","id":"e1","name":"Edit","input":{"file_path":"/a.ts","new_string":"y"}}]}}"#,
        );
        let s = ingest_str(raw, Lane::Main);
        let p = session_overview(&s, &[], &meta());
        assert_eq!(p["session"]["started"], "2026-01-01T00:00:00Z");
        assert_eq!(p["session"]["duration_min"], 30);
    }

    #[test]
    fn overview_duration_is_null_for_empty_sessions() {
        let s = ingest_str("", Lane::Main);
        let p = session_overview(&s, &[], &meta());
        assert!(p["session"]["started"].is_null());
        assert!(p["session"]["duration_min"].is_null());
    }

    #[test]
    fn all_payloads_are_valid_json_with_provenance_and_under_cap() {
        let s = busy_session();
        let r = rank(&s);
        let m = meta();
        let caps = [
            (session_overview(&s, &r, &m), 1000),
            (struggle_areas(&r, &m, 10), 1500),
            (file_story(&s, "/a.ts", &m), 1500),
            (blind_spots(&s, &m), 1000),
            (context_health(&s, &m), 1000),
            (evidence(&s, &[Idx(0), Idx(1)], &m), 1500),
        ];
        for (payload, cap) in caps {
            assert_eq!(payload["v"], 1);
            assert_eq!(payload["session"]["identified_by"], "explicit");
            assert!(
                payload.get("truncated").is_some(),
                "truncated flag required"
            );
            assert!(est_tokens(&payload) <= cap, "over cap: {}", payload);
        }
    }

    #[test]
    fn evidence_stays_under_its_token_cap_with_ten_dense_excerpts() {
        // Ten Bash actions, each with a max-length command — the worst case
        // that used to bust the 1500-token cap on real data.
        let lines: Vec<String> = (0..10)
            .map(|i| {
                format!(
                    r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:0{i}Z","message":{{"content":[{{"type":"tool_use","id":"c{i}","name":"Bash","input":{{"command":"{}"}}}}]}}}}"#,
                    "x".repeat(700)
                )
            })
            .collect();
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let idxs: Vec<Idx> = (0..10).map(Idx).collect();
        let p = evidence(&s, &idxs, &meta());
        assert!(
            est_tokens(&p) <= 1500,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true, "dropping for cap must be visible");
        assert!(!p["actions"].as_array().unwrap().is_empty());
    }

    #[test]
    fn evidence_excerpts_are_redacted() {
        // A Bash action whose command carries a secret assignment.
        let line = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"b1","name":"Bash","input":{"command":"export API_KEY=abc123def456 && deploy"}}]}}"#;
        let s = ingest_str(line, Lane::Main);
        let p = evidence(&s, &[Idx(0)], &meta());
        let text = p["actions"][0]["excerpt"].as_str().unwrap();
        assert!(text.contains("[REDACTED]"), "secret must be masked: {text}");
        assert!(!text.contains("abc123def456"), "raw secret leaked: {text}");
    }

    #[test]
    fn struggle_areas_echoes_the_ranking_rule_and_breakdown() {
        let s = churny_session();
        let p = struggle_areas(&rank(&s), &meta(), 5);
        assert_eq!(p["v"], 1);
        // SPEC §7: ranking output is never an opaque number. The rule that
        // produced the order ships with the order.
        assert_eq!(p["ranking_rule"], crate::score::RANKING_RULE);
        assert!(p["files"][0]["breakdown"].is_object());
        assert!(p["files"][0]["class"].is_string());
        assert!(p["files"][0]["edits"].is_u64());
        assert!(
            p["files"][0].get("score").is_none(),
            "the weighted score is gone, not renamed"
        );
        assert!(p.get("weights").is_none(), "weights are gone");
    }

    #[test]
    fn session_overview_top_struggles_carry_class_and_edits() {
        let s = churny_session();
        let ranked = rank(&s);
        let p = session_overview(&s, &ranked, &meta());
        assert_eq!(p["v"], 1);
        let top = &p["top_struggles"][0];
        assert!(top["class"].is_string());
        assert!(top["edits"].is_u64());
        assert!(top.get("score").is_none());
    }

    // ---------------------------------------------------------------------
    // Adversarial cap enforcement (2026-07-25). Every builder must hold its
    // advertised budget for ANY session, not just well-behaved ones, so each
    // test below feeds the shape that used to blow the cap.
    // ---------------------------------------------------------------------

    /// A path longer than a payload can afford to echo verbatim. 4 KB is the
    /// POSIX `PATH_MAX`, so this is a legal path, not a fantasy one.
    fn huge_path(tag: usize) -> String {
        format!("/{}/f{tag}.rs", "d".repeat(4000))
    }

    /// One finding carrying an absurd number of proving idxs: the real shape
    /// of a churn finding on a file edited thousands of times, or of a
    /// `review_burden` finding whose segment covers the whole session.
    fn fat_finding(kind: crate::model::FindingKind, file: &str, idxs: usize) -> Finding {
        Finding {
            kind,
            tier: crate::model::Tier::T1,
            exact: true,
            confidence: crate::model::Confidence::High,
            idxs: (0..idxs as u32).map(Idx).collect(),
            file: Some(file.to_string()),
            note: Some("synthetic adversarial finding".into()),
            nums: Default::default(),
        }
    }

    /// `files` ranked files, each with a 4 KB path and `per_file` fat findings.
    fn adversarial_ranked(files: usize, per_file: usize) -> Vec<FileScore> {
        use crate::model::FindingKind;
        (0..files)
            .map(|i| {
                let file = huge_path(i);
                FileScore {
                    class: crate::file_class::classify(&file),
                    edits: 1,
                    file: file.clone(),
                    breakdown: [("churn".to_string(), 500u64), ("rework".to_string(), 90)]
                        .into_iter()
                        .collect(),
                    findings: (0..per_file)
                        .map(|_| fat_finding(FindingKind::Churn, &file, 500))
                        .collect(),
                }
            })
            .collect()
    }

    #[test]
    fn session_overview_holds_its_cap_with_hostile_types_and_paths() {
        // 500 distinct unmodeled event types + 4 KB struggle paths: the
        // `unknown_event_types` map and `top_struggles` were both unbounded.
        let lines: Vec<String> = (0..500)
            .map(|i| format!(r#"{{"type":"weird-event-{i}","timestamp":"2026-01-01T00:00:00Z"}}"#))
            .collect();
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let p = session_overview(&s, &adversarial_ranked(20, 4), &meta());
        assert!(
            est_tokens(&p) <= 1000,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true, "dropping content must be visible");
        // Honest disclosure: the payload still says how much it dropped.
        assert_eq!(p["flags"]["unknown_event_types_total"], 500);
        assert_eq!(p["top_struggles_total"], 20);
    }

    #[test]
    fn ordinary_session_with_many_ranked_files_is_not_truncated() {
        // WHY: `top_struggles` is DEFINED as the top 3. Showing 3 of 40 is the
        // field's shape, not a cap kicking in, and the schema table says so
        // ("fixed shape; top_struggles capped at 3"). Flagging `truncated` for
        // it fired on 20 of 40 real sessions, which drowns the signal for the
        // case that matters: the shrink loop actually dropping content. The
        // real count stays visible in `top_struggles_total` either way.
        let s = crate::ingest::ingest_str(
            r#"{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{"content":[{"type":"tool_use","id":"1","name":"Edit","input":{"file_path":"/a.ts","new_string":"x"}}]}}"#,
            crate::model::Lane::Main,
        );
        // 8 ranked files, ordinary short paths, one small finding each.
        let ranked: Vec<FileScore> = (0..8)
            .map(|i| {
                let file = format!("/src/f{i}.rs");
                FileScore {
                    class: crate::file_class::classify(&file),
                    edits: 1,
                    file,
                    breakdown: [("churn".to_string(), 2u64)].into_iter().collect(),
                    findings: vec![],
                }
            })
            .collect();
        let p = session_overview(&s, &ranked, &meta());
        assert_eq!(p["top_struggles"].as_array().unwrap().len(), 3);
        assert_eq!(p["top_struggles_total"], 8, "the real count stays visible");
        assert_eq!(
            p["truncated"], false,
            "the designed top-3 shape must not read as a truncation event"
        );
    }

    #[test]
    fn struggle_areas_clamps_n_and_holds_its_cap() {
        let ranked = adversarial_ranked(200, 40);
        let p = struggle_areas(&ranked, &meta(), usize::MAX);
        assert!(
            est_tokens(&p) <= 1500,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        let files = p["files"].as_array().unwrap();
        assert!(
            files.len() <= STRUGGLE_FILES_MAX,
            "n must be clamped, got {} files",
            files.len()
        );
        assert_eq!(p["truncated"], true);
        assert_eq!(p["files_total"], 200);
        // Per-file disclosure of the findings that did not fit.
        assert!(files[0]["findings_omitted"].as_u64().unwrap_or(0) > 0);
    }

    #[test]
    fn struggle_areas_keeps_findings_representative_of_the_breakdown() {
        // Detector order puts every rework finding first, so the old
        // `take(FINDINGS_PER_FILE)` spent all four slots on rework and never
        // showed the failure_loop / fumble evidence that also scored.
        use crate::model::FindingKind;
        let file = "/a.ts";
        let mut findings: Vec<Finding> = (0..6)
            .map(|_| fat_finding(FindingKind::Rework, file, 2))
            .collect();
        findings.push(fat_finding(FindingKind::FailureLoop, file, 2));
        findings.push(fat_finding(FindingKind::BlindWriteAttempt, file, 1));
        let ranked = vec![FileScore {
            class: crate::file_class::classify(file),
            edits: 6,
            file: file.into(),
            breakdown: [
                ("rework".to_string(), 6u64),
                ("failure_loops".to_string(), 1),
                ("fumbles".to_string(), 1),
            ]
            .into_iter()
            .collect(),
            findings,
        }];
        let p = struggle_areas(&ranked, &meta(), 5);
        let kinds: Vec<&str> = p["files"][0]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["kind"].as_str().unwrap())
            .collect();
        assert!(
            kinds.contains(&"failure_loop") && kinds.contains(&"blind_write_attempt"),
            "every scoring category must be represented, got {kinds:?}"
        );
    }

    #[test]
    fn file_story_holds_its_cap_with_a_hostile_path() {
        let path = huge_path(0);
        let lines: Vec<String> = (0..100)
            .map(|i| {
                format!(
                    r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"{path}","new_string":"x"}}}}]}}}}"#
                )
            })
            .collect();
        let s = ingest_str(&lines.join("\n"), Lane::Main);
        let p = file_story(&s, &path, &meta());
        assert!(
            est_tokens(&p) <= 1500,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true);
    }

    /// A session with `n` rejected blind writes plus one enormous review
    /// burden segment. These are the two lists `blind_spots` emitted in full.
    fn blind_spot_storm(n: usize) -> Session {
        let mut lines = Vec::new();
        for i in 0..n {
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:00Z","message":{{"content":[{{"type":"tool_use","id":"e{i}","name":"Edit","input":{{"file_path":"{}","new_string":"{}"}}}}]}}}}"#,
                huge_path(i),
                "line\\n".repeat(20)
            ));
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:01Z","message":{{"content":[{{"type":"tool_result","tool_use_id":"e{i}","is_error":true,"content":"File has not been read yet"}}]}}}}"#
            ));
        }
        ingest_str(&lines.join("\n"), Lane::Main)
    }

    #[test]
    fn blind_spots_holds_its_cap_with_thousands_of_findings() {
        let s = blind_spot_storm(2000);
        let p = blind_spots(&s, &meta());
        assert!(
            est_tokens(&p) <= 1000,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true);
        assert_eq!(p["totals"]["blind_write_attempts"], 2000);
    }

    #[test]
    fn context_health_holds_its_cap_with_a_hostile_session_id() {
        let s = blind_spot_storm(50);
        let m = SessionMeta {
            id: "z".repeat(8000),
            identified_by: "explicit".into(),
        };
        let p = context_health(&s, &m);
        assert!(
            est_tokens(&p) <= 1000,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true, "an elided id is dropped content");
    }

    #[test]
    fn every_payload_holds_its_cap_on_an_adversarial_session() {
        // The one test that covers all six at once: hostile paths, hostile
        // session id, thousands of findings, hundreds of event types.
        let s = blind_spot_storm(1500);
        let ranked = rank(&s);
        let m = SessionMeta {
            id: "z".repeat(8000),
            identified_by: "explicit".into(),
        };
        let path = huge_path(0);
        let caps = [
            (session_overview(&s, &ranked, &m), 1000),
            (struggle_areas(&ranked, &m, usize::MAX), 1500),
            (file_story(&s, &path, &m), 1500),
            (blind_spots(&s, &m), 1000),
            (context_health(&s, &m), 1000),
            (
                evidence(&s, &(0..50).map(Idx).collect::<Vec<_>>(), &m),
                1500,
            ),
        ];
        for (payload, cap) in caps {
            assert!(
                est_tokens(&payload) <= cap,
                "over cap {cap}: ~{} tokens",
                est_tokens(&payload)
            );
        }
    }
}
