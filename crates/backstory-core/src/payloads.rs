//! The six MCP tool payloads (T3.5), built to the v1 contract
//! (`docs/payload-schema.md`) and enforced by `scripts/check_payloads.py`.
//!
//! Compact JSON, hard token caps, `truncated` markers. The tool returns
//! evidence; the connected agent narrates. Every payload carries the ADR A4
//! provenance in `session.identified_by`.

use crate::assemble::AssembledUnit;
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
/// Token cap for `review_context`. Larger than the other payloads because it
/// carries quoted prose rather than counts, and smaller than
/// `session_intent` because it is PUSHED into the reviewer's context by
/// default. Keeping the pushed payload small is what avoids the
/// overcorrection effect the design is built around.
const CAP_REVIEW_CONTEXT: usize = 2000;
/// Starting list length for each block, walked down by `shrink_to_fit`.
const CONTEXT_LIST_MAX: usize = 12;
/// Longest single quoted string in `review_context`. A quote longer than this
/// belongs in `session_intent`, which the reviewer pulls deliberately.
const QUOTE_MAX: usize = 500;
/// Token cap for `session_intent`. Far larger than every other payload,
/// `review_context` included, because this one is PULLED deliberately by a
/// reviewer that has decided it needs the full request text, not pushed at
/// one that did not ask.
const CAP_INTENT: usize = 20_000;

/// The grouping rule, printed verbatim in every work-unit payload so a reader
/// never has to consult the source to audit a grouping.
pub const WORK_UNIT_RULE: &str = "same project; joined when a transcript overlaps the previous or starts within 30 min of its end";

/// Session identity + how it was resolved (ADR A4 provenance).
pub struct SessionMeta {
    /// Session id.
    pub id: String,
    /// `tool_use_id` | `explicit` | `cli_latest`.
    pub identified_by: String,
    /// The work unit this report covers, or `None` for a single transcript.
    pub unit: Option<UnitMeta>,
}

/// The work-unit grouping behind a report, when there was one.
///
/// Every count and list here describes the transcripts that were ACTUALLY
/// ANALYZED, so the schema invariants hold under a partial load: `sessions`
/// equals `session_ids.len()`, and `joined_gaps_min` is one shorter. What
/// could not be analyzed is disclosed separately (`members_unreadable`,
/// `siblings_unplaced`, `dropped`) rather than making the analyzed counts
/// lie in either direction.
#[derive(Debug, Clone)]
pub struct UnitMeta {
    /// Transcripts merged (loaded and analyzed).
    pub sessions: usize,
    /// Gap in minutes before each analyzed member after the first. Negative
    /// means the transcript overlapped the running span, so it was a
    /// concurrent Claude Code instance rather than a continuation.
    pub joined_gaps_min: Vec<f64>,
    /// First timestamp across the analyzed members.
    pub span_start: String,
    /// Last timestamp across the analyzed members.
    pub span_end: String,
    /// Analyzed transcript ids, oldest first.
    pub session_ids: Vec<String>,
    /// Members dropped by the size cap.
    pub dropped: u64,
    /// Discovered members of THIS unit that could not be loaded (unreadable
    /// file, over the byte ceiling). Their gaps and span are excluded above.
    pub members_unreadable: u64,
    /// Same-directory transcripts whose time span could not be read at
    /// discovery. Unit membership unknown; see `WorkUnit::unplaced`.
    pub siblings_unplaced: u64,
}

/// Turn an assembled work unit into the `UnitMeta` a payload discloses, or
/// `None` when there is nothing to disclose: exactly one discovered member,
/// which loaded, with nothing dropped and nothing unplaced. That is the
/// plain single-transcript analysis the schema documents as carrying no
/// `work_unit` block, and gating it HERE keeps the CLI and the MCP server
/// (the two callers) incapable of disagreeing about when the block appears.
pub fn unit_meta_from(a: &AssembledUnit) -> Option<UnitMeta> {
    if a.unit.members.len() <= 1
        && a.members_missing == 0
        && a.unit.unplaced == 0
        && a.unit.dropped == 0
    {
        return None;
    }
    // The analyzed members, oldest first: the unit's discovered members
    // filtered down to the ones that actually loaded. Gaps and span are
    // recomputed over this subset so every field describes the same set of
    // transcripts the report analyzed (a discovered-but-unreadable member
    // must not contribute a gap entry the schema then miscounts).
    let loaded: Vec<crate::work_unit::Member> = a
        .unit
        .members
        .iter()
        .filter(|m| a.member_paths.contains(&m.path))
        .cloned()
        .collect();
    Some(UnitMeta {
        sessions: loaded.len(),
        joined_gaps_min: crate::work_unit::gaps_between(&loaded),
        span_start: loaded
            .first()
            .map(|m| m.span.first.clone())
            .unwrap_or_default(),
        span_end: loaded
            .iter()
            .map(|m| m.span.last.clone())
            .max()
            .unwrap_or_default(),
        session_ids: a.session.session_ids.clone(),
        dropped: a.unit.dropped,
        members_unreadable: a.members_missing,
        siblings_unplaced: a.unit.unplaced,
    })
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

/// The first 8 characters of a transcript uuid. Short enough to keep the
/// payload inside its token cap, long enough to identify a transcript, and it
/// leaks neither the home directory nor the username.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Which transcript a finding's evidence came from, as a short id. `None`
/// when the finding has no evidence indices, the analysis was a single
/// transcript, or the caller never supplied a work unit.
///
/// Both conditions matter, not just `session_ids.len()`: the schema (T7)
/// guarantees a `session` key on a finding only when a `work_unit` block is
/// also present on the payload, and `work_unit` is driven entirely by
/// `meta.unit`, not by the session table. Without this check, a caller that
/// passes a merged multi-transcript `Session` while leaving `unit: None`
/// (exactly what `backstory-mcp/src/server.rs` does today) would emit `session`
/// keys on findings with no `work_unit` block to explain them.
fn finding_session(s: &Session, meta: &SessionMeta, idxs: &[Idx]) -> Option<String> {
    if meta.unit.is_none() || s.session_ids.len() < 2 {
        return None;
    }
    let first = idxs.first()?;
    let a = s.actions.get(first.0 as usize)?;
    s.session_ids
        .get(a.session_ix as usize)
        .map(|x| short_id(x))
}

/// Which transcript an item's own `session_ix` names, as a short id. Same
/// conditionality as `finding_session` (a `work_unit` block must be present
/// AND the unit must actually span more than one transcript), but for the
/// `context` module's extraction structs (`Request`, `DecisionOut`,
/// `UnfinishedTask`, `Claim`), which already carry `session_ix` directly
/// rather than needing it resolved through an `Idx` into `Session::actions`.
///
/// This is the fix for the v3 payload citation gap: `review_context` and
/// `session_intent` cover the whole work unit by default, where `line_no`
/// alone is per-transcript and collides across members, so a bare "line": 7
/// cannot say which transcript it names. Adding this alongside `line` (only
/// when there is more than one transcript to disambiguate) makes every
/// citation resolvable again without touching the single-transcript shape.
fn item_session(s: &Session, meta: &SessionMeta, session_ix: u16) -> Option<String> {
    if meta.unit.is_none() || s.session_ids.len() < 2 {
        return None;
    }
    s.session_ids.get(session_ix as usize).map(|x| short_id(x))
}

/// The `work_unit` disclosure block, from `meta.unit`: which physically
/// separate transcripts were merged into this analysis, the rule that
/// merged them, and the gaps between them. `None` for a single-transcript
/// analysis (no `unit` supplied), which is what keeps that payload
/// byte-identical to v0.1/v2 apart from the version bump.
///
/// Shared by `session_overview` (its original home) and `review_context`
/// (Important #2's fix: without this, a multi-transcript `review_context`
/// payload discloses nowhere that several transcripts were merged at all).
/// The second element of the returned pair is "did rendering this block
/// have to elide a timestamp", which the caller folds into its own
/// `truncated` the same way `session_overview` always has: only ever
/// flipping it to `true`, never masking an earlier cut.
fn work_unit_block(meta: &SessionMeta) -> Option<(Value, bool)> {
    let u = meta.unit.as_ref()?;
    // See `session_overview`'s inline comment (same elision, same reason):
    // `span_start`/`span_end` are scraped straight out of a transcript's
    // JSONL lines and are therefore untrusted, caller-controlled text.
    let span_start_cut = would_elide(&u.span_start, TS_MAX);
    let span_end_cut = would_elide(&u.span_end, TS_MAX);
    let block = json!({
        "rule": WORK_UNIT_RULE,
        "sessions": u.sessions,
        "joined_gaps_min": u.joined_gaps_min.iter().copied().map(round2).collect::<Vec<_>>(),
        "span_start": elide_middle(&u.span_start, TS_MAX),
        "span_end": elide_middle(&u.span_end, TS_MAX),
        "session_ids": u.session_ids
            .iter()
            .map(|s| short_id(s))
            .collect::<Vec<_>>(),
        "dropped": u.dropped,
        // The two exclusion disclosures (spec §8): members of this unit that
        // could not be loaded, and same-directory transcripts that could not
        // be placed in time at all. Always present so the shape is stable;
        // both are usually 0.
        "members_unreadable": u.members_unreadable,
        "siblings_unplaced": u.siblings_unplaced,
    });
    Some((block, span_start_cut || span_end_cut))
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

/// One finding rendered for a payload, with its two unbounded fields capped
/// and (when this is a multi-transcript work unit) a `session` key added.
///
/// `idxs` is the dangerous one: a churn finding on a file edited 800 times
/// carries 800 indices (~4 KB of JSON on its own), and a `review_burden`
/// finding's idxs span every edit in a segment. We keep the first
/// `FINDING_IDXS_MAX` (head-kept, tail-dropped: the rule the schema
/// advertises for finding lists) and state the true length in `idxs_total`
/// so the count is never lost, only the list.
///
/// `session` is resolved by the caller (`finding_session`) rather than here,
/// because that lookup needs the whole `Session` (to walk from the finding's
/// first `idx` to the action's `session_ix`) and not every caller of this
/// function has one at hand.
fn compact_finding(f: &Finding, session: Option<String>) -> Value {
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
    // Only present for a multi-transcript work unit (see `finding_session`),
    // so a single-transcript payload keeps the exact v0.1 finding shape.
    if let Some(sess) = session {
        obj.insert("session".into(), json!(sess));
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
        // `mode` and `permission-mode` are the newer and older names of the
        // same event; ingest reads both (running permission mode), so neither
        // may be listed as "unknown".
        .filter(|(t, _)| {
            !matches!(
                t.as_str(),
                "assistant" | "user" | "mode" | "permission-mode"
            )
        })
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
        let mut out = json!({
            "v": 2,
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
        });
        // Disclose the grouping in the same auditable spirit as `ranking_rule`:
        // the rule states itself, and the actual gaps are listed, so any
        // grouping can be checked by hand. Absent entirely for a
        // single-transcript analysis, which is what keeps that payload
        // byte-identical to v0.1 apart from the version bump. Note that
        // `work_unit` is MEASURED by the `shrink_to_fit` loop above but is
        // not one of its knobs (the loop only shrinks `unknown_event_types`),
        // so a transcript whose timestamp field is a 20 KB string would blow
        // `CAP_OVERVIEW` with nothing left able to trim it back down; that is
        // why `work_unit_block` elides its own timestamps independently.
        if let Some((block, cut)) = work_unit_block(meta) {
            out["work_unit"] = block;
            // Fold into the same `truncated` disclosure every other cut in this
            // file uses: only ever flip it to `true`, never back to `false`,
            // so an earlier cut (a sampled type map, a capped id) can never be
            // masked by this one not firing.
            if cut {
                out["truncated"] = json!(true);
            }
        }
        out
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
///
/// Takes `s: &Session` (in addition to the already-ranked `FileScore`s) so
/// each finding can carry a `session` key: `FileScore` only stores `Finding`s
/// and their `idxs`, not the `Action`s those `idxs` point into, and it is the
/// action's `session_ix` that says which transcript a finding's evidence
/// actually came from (see `finding_session`).
pub fn struggle_areas(s: &Session, ranked: &[FileScore], meta: &SessionMeta, n: usize) -> Value {
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
                    "findings": kept.iter()
                        .map(|f| compact_finding(f, finding_session(s, meta, &f.idxs)))
                        .collect::<Vec<_>>()
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
            "v": 2,
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
            "v": 2,
            "session": session,
            "file": file,
            "events": head,
            "elided": elided,
            "tail": tail,
            "truncated": elided.is_some() || path_cut || id_cut
        })
    })
}

/// `blind_spots()` — secrets touches, blind-write attempts, review burden,
/// approval outliers.
///
/// Every matching finding used to be emitted in full with `truncated: false`.
/// An ordinary 300-edit session measured 3166 tokens against a 1000 budget,
/// and one `review_burden` finding can carry every edit index in its segment.
/// Now each list is tail-truncated to the SAME cap `k`, shrinking until the
/// payload fits, with the true counts kept in `totals`.
///
/// WHY one shared `k` rather than a global budget spent list by list: the
/// four lists are different KINDS of blind spot, and a session with 2000
/// blind-write attempts would otherwise spend the whole payload on them and
/// push `review_burden` out entirely. Review burden is the metric this file
/// promises never to suppress, so crowding it out would break that promise by
/// the back door. Equal caps keep every category visible.
pub fn blind_spots(s: &Session, meta: &SessionMeta) -> Value {
    use crate::model::FindingKind;
    let all = crate::score::all_findings(s);
    let of_kind =
        |kind: FindingKind| -> Vec<&Finding> { all.iter().filter(|f| f.kind == kind).collect() };
    // Zero-tolerance: a .env/credentials/key touch, surfaced here rather than
    // ranked because `file_class` puts Config in the last ranking tier and
    // burying this would defeat the point (file_class.rs module doc).
    let secrets = of_kind(FindingKind::SecretsFileTouched);
    let blind = of_kind(FindingKind::BlindWriteAttempt);
    // The comprehension-layer anchor (metrics-spec #27): agent LOC per human
    // turn vs the 200–400 LOC review band. Never suppressed — it is exactly
    // the auto-accept mode where this matters most.
    let burden = of_kind(FindingKind::ReviewBurden);
    let outliers = of_kind(FindingKind::LargeWriteInstantAccept);
    let (session, id_cut) = session_block(meta);
    let findings_cut = secrets
        .iter()
        .chain(&blind)
        .chain(&burden)
        .chain(&outliers)
        .any(|f| finding_was_capped(f));
    let longest = secrets
        .len()
        .max(blind.len())
        .max(burden.len())
        .max(outliers.len());
    // Start at the full cap even when the lists are shorter, so `list_cap`
    // reports the cap that was in force rather than "however many I had".
    shrink_to_fit(CAP_BLIND, BLIND_LIST_MAX, |k| {
        let list = |v: &[&Finding]| -> Vec<Value> {
            v.iter()
                .take(k)
                .map(|f| compact_finding(f, finding_session(s, meta, &f.idxs)))
                .collect()
        };
        json!({
            "v": 2,
            "session": session,
            "secrets_file_touched": list(&secrets),
            "blind_write_attempts": list(&blind),
            "review_burden": list(&burden),
            "approval_outliers": list(&outliers),
            // Full counts, always present: the lists above are a sample, and
            // "2 shown" must never be mistaken for "2 happened".
            "totals": {
                "secrets_file_touched": secrets.len(),
                "blind_write_attempts": blind.len(),
                "review_burden": burden.len(),
                "approval_outliers": outliers.len()
            },
            "list_cap": k,
            "suppression": {
                "approval_latency": if crate::signals::comprehension::approval_latency_active(s) {"active"} else {"suppressed"},
                "suppressed_when": "every main-lane action ran under an auto-accept permission mode",
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
            "v": 2,
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
            "v": 2,
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

/// Recorded session context for a reviewing agent (`v: 3`).
///
/// Four blocks (`scope`, `decisions`, `incomplete`, `claims`), all exact. (A
/// fifth heuristic `constraints` block was cut before implementation; see the
/// plan's Task 9.) Every list is a capped sample and every corresponding
/// total is unconditional, so "3 shown" can never be read as "3 happened".
/// The same discipline reaches one level down: `options_not_chosen`, nested
/// inside each decision, is itself a capped sample with an unconditional
/// `options_not_chosen_total`, and EVERY transcript-derived string this
/// payload echoes (requests, file paths, questions, answers, option labels,
/// task subjects, statuses, commands, claim text) is elided before it is
/// emitted, never raw. `truncated` reflects every one of those elisions
/// (see `would_elide`), not just list cardinality and the session id, so a
/// single oversized field can never be dropped invisibly.
///
/// This is a fix for an adversarial review of the original commit: two
/// fields (`options_not_chosen`, `last_status`) were echoed verbatim with no
/// per-string cap, and because `shrink_to_fit` walks ONE shared `k` down
/// across all six lists, a single oversized value forced `k` all the way to
/// 0, emptying every OTHER block too. Splitting `shrink_to_fit` so each
/// block got its own budget was considered and rejected here: it is a
/// larger change to a helper five other payloads share, and it is
/// unnecessary once every nested string is capped, since that is what let
/// one field force the shared `k` to 0 in the first place.
pub fn review_context(s: &Session, meta: &SessionMeta) -> Value {
    let scope = crate::context::scope(s);
    let decisions = crate::context::decisions(s);
    let incomplete = crate::context::incomplete(s);
    let claims = crate::context::claims(s);
    let (session, id_cut) = session_block(meta);

    // The true counts, computed once and never subject to the cap below.
    // `agent_texts_excluded` rides alongside `claims` for the same
    // disclosure reason `unattributed_turns` rides alongside `requests`:
    // a payload that quotes only what it could attribute must say how much
    // it could not, or a short list reads as complete when it is not.
    let totals = json!({
        "requests": scope.requests.len(),
        "files_edited": scope.files_edited.len(),
        "decisions": decisions.len(),
        "unfinished_tasks": incomplete.unfinished_tasks.len(),
        "failing_commands": incomplete.failing_commands.len(),
        "claims": claims.len(),
        "agent_texts_excluded": s.agent_texts_excluded
    });
    let longest = scope
        .requests
        .len()
        .max(scope.files_edited.len())
        .max(decisions.len())
        .max(claims.len())
        .max(incomplete.unfinished_tasks.len())
        .max(incomplete.failing_commands.len());

    // Top-level, not nested a level down where the disclosure went unread
    // (the review's actual finding): a reviewer that only checks
    // `truncated` and `attributed_intent_via` must still see this.
    // `attributed_intent_via` re-derives from the SAME attributed requests
    // already shown in `scope.requests` above; it CANNOT recover a turn
    // `requests` excluded (`unattributed_turns`, never attributed to begin
    // with) or subagent prose `claims` excluded (`agent_texts_excluded`,
    // never eligible for `claims` to begin with). `coverage_incomplete` is
    // the one boolean a reviewer needs to know that gap exists at all.
    //
    // `session_intent` carries the narrower `request_coverage` object
    // instead of this one, deliberately not named `coverage`: it has no
    // claims block, so `agent_texts_excluded` describes an exposure it
    // does not have, and folding it in anyway would make the two payloads'
    // same-shaped fields mean different things. See `session_intent`'s doc
    // comment (Fix 3) for the full reasoning.
    let coverage_incomplete = scope.unattributed_turns > 0 || s.agent_texts_excluded > 0;
    let coverage = json!({
        "coverage_incomplete": coverage_incomplete,
        "unattributed_turns": scope.unattributed_turns,
        "agent_texts_excluded": s.agent_texts_excluded,
        "note": "attributed_intent_via re-derives the same attributed requests shown above, in full; it cannot recover unattributed_turns (unknown-origin turns, never attributed) or agent_texts_excluded (excluded subagent prose, never eligible for claims)"
    });

    shrink_to_fit(CAP_REVIEW_CONTEXT, CONTEXT_LIST_MAX, |k| {
        // Would any string actually rendered at this `k` need eliding? Each
        // check is scoped to the same `.take(k)` the render below uses, so
        // this reflects what the payload is ABOUT to emit, not the full
        // unshown data. Folded into `truncated` below: a single oversized
        // field must flip it exactly like a capped list does.
        let requests_cut = scope
            .requests
            .iter()
            .take(k)
            .any(|r| would_elide(&r.text, QUOTE_MAX));
        let files_cut = scope
            .files_edited
            .iter()
            .take(k)
            .any(|f| would_elide(f, PATH_MAX));
        let decisions_cut = decisions.iter().take(k).any(|d| {
            would_elide(&d.question, QUOTE_MAX)
                || d.chosen
                    .as_deref()
                    .is_some_and(|c| would_elide(c, QUOTE_MAX))
                || d.options_not_chosen.len() > k
                || d.options_not_chosen
                    .iter()
                    .take(k)
                    .any(|o| would_elide(o, QUOTE_MAX))
        });
        let tasks_cut = incomplete.unfinished_tasks.iter().take(k).any(|t| {
            t.subject
                .as_deref()
                .is_some_and(|x| would_elide(x, QUOTE_MAX))
                || would_elide(&t.last_status, QUOTE_MAX)
        });
        let commands_cut = incomplete
            .failing_commands
            .iter()
            .take(k)
            .any(|c| would_elide(&c.command, QUOTE_MAX));
        let claims_cut = claims
            .iter()
            .take(k)
            .any(|c| would_elide(&c.text, QUOTE_MAX));
        let string_cut =
            requests_cut || files_cut || decisions_cut || tasks_cut || commands_cut || claims_cut;

        let mut out = json!({
            "v": 3,
            "session": session,
            "scope": {
                "requests": scope.requests.iter().take(k).map(|r| {
                    let mut v = json!({
                        "text": elide_middle(&r.text, QUOTE_MAX),
                        "line": r.line_no
                    });
                    // Only present for a multi-transcript work unit (see
                    // `item_session`): `line_no` alone is per-transcript and
                    // collides across members once several transcripts are
                    // in scope, so this is what makes the citation resolve.
                    // Absent for single-transcript payloads, which keeps
                    // that shape byte-identical to before this fix.
                    if let Some(sess) = item_session(s, meta, r.session_ix) {
                        v["session"] = json!(sess);
                    }
                    v
                }).collect::<Vec<_>>(),
                "files_edited": scope.files_edited.iter().take(k).map(|f| json!(
                    elide_middle(f, PATH_MAX)
                )).collect::<Vec<_>>(),
                // Disclosure, not a cap victim: a scalar count that is never
                // itself shrunk. Turns whose origin could not be attributed
                // to the human are excluded from `requests` (see
                // `context::Scope::unattributed_turns`); hiding this count
                // would misrepresent how complete the quoted intent is.
                "unattributed_turns": scope.unattributed_turns
            },
            "decisions": decisions.iter().take(k).map(|d| {
                let mut v = json!({
                    "question": elide_middle(&d.question, QUOTE_MAX),
                    "chosen": d.chosen.as_ref().map(|c| elide_middle(c, QUOTE_MAX)),
                    // Capped and elided like every other transcript-derived
                    // list/string pair here: an AskUserQuestion can carry an
                    // arbitrary number of options, each an arbitrary-length
                    // label, and both were unbounded before this fix.
                    "options_not_chosen": d.options_not_chosen.iter().take(k)
                        .map(|o| json!(elide_middle(o, QUOTE_MAX)))
                        .collect::<Vec<_>>(),
                    "options_not_chosen_total": d.options_not_chosen.len(),
                    // idxs, not just a line number: the Global Constraint is that
                    // every item is dereferenceable, and `evidence()` takes Idx.
                    // A bare line number is not something a reviewer can resolve.
                    "idxs": d.idxs.iter().take(FINDING_IDXS_MAX).collect::<Vec<_>>(),
                    "line": d.line_no
                });
                // Alongside `line` for the same reason as `requests` above:
                // `idxs` only dereferences when `resolution` is `Resolved`
                // (see `Resolution`), so `line` (and now `session`) remains
                // the only citation for a `NotFound` or `Ambiguous` decision.
                if let Some(sess) = item_session(s, meta, d.session_ix) {
                    v["session"] = json!(sess);
                }
                v
            }).collect::<Vec<_>>(),
            "incomplete": {
                "unfinished_tasks": incomplete.unfinished_tasks.iter().take(k)
                    .map(|t| {
                        let mut v = json!({
                            "subject": t.subject.as_ref().map(|x| elide_middle(x, QUOTE_MAX)),
                            // Elided like every other transcript-derived string:
                            // this is the harness-confirmed `statusChange` (or
                            // requested status) verbatim from the transcript,
                            // not a value backstory-mcp controls.
                            "last_status": elide_middle(&t.last_status, QUOTE_MAX),
                            "line": t.line_no
                        });
                        if let Some(sess) = item_session(s, meta, t.session_ix) {
                            v["session"] = json!(sess);
                        }
                        v
                    }).collect::<Vec<_>>(),
                "failing_commands": incomplete.failing_commands.iter().take(k)
                    .map(|c| json!({
                        "command": elide_middle(&c.command, QUOTE_MAX),
                        "idxs": [c.idx]
                    })).collect::<Vec<_>>()
            },
            "claims": claims.iter().take(k).map(|c| {
                let mut v = json!({
                    "text": elide_middle(&c.text, QUOTE_MAX),
                    "line": c.line_no,
                    // True when the human turn closing this claim's window was an
                    // interrupt: the text is still real evidence of what the
                    // agent was doing, but it is possible INTENT cut short, not a
                    // completion claim to check against the diff.
                    "window_interrupted": c.window_interrupted
                });
                if let Some(sess) = item_session(s, meta, c.session_ix) {
                    v["session"] = json!(sess);
                }
                v
            }).collect::<Vec<_>>(),
            "totals": totals,
            "list_cap": k,
            "coverage": coverage,
            // Renamed from `full_intent_via`: it returns the same ATTRIBUTED
            // requests already shown in `scope.requests`, in full, not the
            // unattributed ones. It was never "full intent" and calling it
            // that implied a completeness this tool cannot deliver; see
            // `coverage` for what it genuinely cannot recover.
            "attributed_intent_via": "session_intent",
            "truncated": longest > k || id_cut || string_cut
        });
        // Same disclosure `session_overview` carries (Important #2's fix):
        // without this, a multi-transcript `review_context` payload never
        // says that several transcripts were merged at all, even once the
        // per-item `session` keys above start appearing.
        if let Some((block, cut)) = work_unit_block(meta) {
            out["work_unit"] = block;
            if cut {
                out["truncated"] = json!(true);
            }
        }
        out
    })
}

/// The full verbatim human requests for the session (`v: 3`).
///
/// Deliberately a separate tool from `review_context`: supplying a reviewer
/// with requirements up front and asking it to check conformance induces
/// OVERCORRECTION, where the model starts assuming flaws exist and flags
/// correct code, and the effect gets worse the more you ask it to explain
/// and repair (arXiv:2603.00539). So the full intent is deliberately NOT in
/// the pushed `review_context` payload; making a reviewer ask for it here is
/// the entire point of this tool existing separately.
///
/// Request text is NEVER elided, unlike every other transcript-derived
/// string this crate echoes. If a request does not fit, it is DROPPED
/// whole, never cut down: a half-quoted request is worse than a missing one
/// for a tool whose entire premise is exact quoting.
///
/// `max_tokens` lets a caller request a smaller budget than `CAP_INTENT`. A
/// caller-supplied budget can only ever LOWER the cap, never raise it
/// (`.min(CAP_INTENT)`), so this tool can't be pointed at itself to buy back
/// the room `review_context` deliberately withholds.
///
/// # Fix 1: requests are packed INDIVIDUALLY, not via `shrink_to_fit`
///
/// `shrink_to_fit`'s shared-prefix walk (`take(k)`, `k` falling from a
/// start) does not fit this payload: request 0 is a member of every
/// nonempty prefix, so a single oversized first request busts the cap at
/// every `k > 0`, forcing the walk to `k == 0` and withholding EVERY
/// request, including ones far down the list that would have fit easily on
/// their own. A reviewer that gets nothing back cannot tell "intent existed
/// but was withheld" from "there was no intent to show", and `truncated`
/// alone does not distinguish the two.
///
/// Fixed locally to this payload (not in `shrink_to_fit`, which five other
/// payloads share and whose shared-prefix contract is correct for them):
/// walk `scope.requests` in order, add each one to the running payload if
/// doing so keeps it within `cap`, and otherwise skip just that one and
/// keep walking. `requests_included` and `requests_omitted_for_size` are
/// reported unconditionally so a reviewer can always tell how much was
/// withheld and that the reason was size. Every request in `scope.requests`
/// is considered; the token budget is the only bound on how many come back,
/// so `requests_omitted_for_size` accounts for every one that did not make
/// it in.
///
/// # Fix 2: `max_tokens` is honored as a real ceiling, not overshot
///
/// The payload's fixed envelope (session id, totals, coverage, zero
/// requests) has a real minimum size: `min_viable_tokens`, computed once
/// per call and always reported. When `cap` is below it, no amount of
/// packing can help, so this returns the envelope with `budget_too_small:
/// true` rather than silently emitting a payload bigger than the caller
/// asked for.
///
/// # Fix 3: `request_coverage`, not `coverage`
///
/// `review_context`'s `coverage_incomplete` folds in `agent_texts_excluded`
/// (subagent prose that never made it into `agent_texts`, so `claims`
/// cannot show it). This payload has no claims block and never touches
/// `agent_texts`, so `agent_texts_excluded` describes an exposure this tool
/// genuinely does not have; folding it in anyway (option (a), a shared
/// coverage builder) would give `session_intent` a field with no meaning in
/// its own context, purely to LOOK consistent with `review_context`. That
/// is worse than the divergence it would fix: identical field values with
/// different meanings are exactly as unsafe to compare as identically
/// NAMED fields with different meanings. So this is option (b): the object
/// is renamed `request_coverage`, scoped to the one exposure this payload
/// actually has (`unattributed_turns`, the same attributed-only set
/// `review_context` shows), so the name itself says "this is narrower than
/// `review_context`'s `coverage`" and no consumer can read one payload's
/// field expecting the other's semantics. Both derive `unattributed_turns`
/// from the same `context::scope`, so the two can never disagree about
/// THAT count, only about whether `agent_texts_excluded` also applies,
/// which `request_coverage` no longer claims to speak to at all.
pub fn session_intent(s: &Session, meta: &SessionMeta, max_tokens: Option<usize>) -> Value {
    let scope = crate::context::scope(s);
    let (session, id_cut) = session_block(meta);
    let cap = max_tokens.unwrap_or(CAP_INTENT).min(CAP_INTENT);
    let total = scope.requests.len();

    let coverage_incomplete = scope.unattributed_turns > 0;
    let request_coverage = json!({
        "coverage_incomplete": coverage_incomplete,
        "unattributed_turns": scope.unattributed_turns,
        "note": "requests re-derives the same attributed requests scope() produced, in full; it cannot recover unattributed_turns (unknown-origin turns, never attributed to begin with)"
    });

    // `min_viable_tokens` is threaded through `build` as a real field (Fix
    // E / Minor 5), not appended to the returned payload afterward. Before
    // this fix it was added AFTER every `est_tokens(...) > cap` check below
    // had already passed, so a budget sitting right at the envelope's true
    // size could see a payload that measured under `cap` without the field
    // and then grew past it once the field was tacked on, silently
    // overshooting a caller's tight budget while still reporting
    // `budget_too_small: false`.
    let build = |kept: &[Value],
                 omitted_for_size: usize,
                 budget_too_small: bool,
                 min_viable_tokens: usize| {
        json!({
            "v": 3,
            "session": session.clone(),
            "requests": kept,
            "totals": {"requests": total},
            "requests_included": kept.len(),
            "requests_omitted_for_size": omitted_for_size,
            "request_coverage": request_coverage.clone(),
            "budget_too_small": budget_too_small,
            "min_viable_tokens": min_viable_tokens,
            "truncated": omitted_for_size > 0 || id_cut || budget_too_small
        })
    };

    // Fix 2: the zero-request envelope is the floor this payload can ever
    // shrink to. `omitted_for_size` is reported as every request here,
    // which is accurate: if even the empty envelope does not fit `cap`, no
    // single request can fit alongside it either.
    //
    // `min_viable_tokens` is itself part of the envelope it measures, so
    // this is a small fixed point: seed at 0, remeasure with the field
    // included, and repeat until the measured size stops changing. This
    // converges in one or two steps in practice (`est_tokens` only moves
    // when the field's own digit count crosses a `CHARS_PER_TOKEN`
    // boundary), and is bounded here so a pathological estimator can never
    // loop forever.
    let mut min_viable_tokens = 0usize;
    for _ in 0..8 {
        let next = est_tokens(&build(&[], total, false, min_viable_tokens));
        if next == min_viable_tokens {
            break;
        }
        min_viable_tokens = next;
    }
    if min_viable_tokens > cap {
        return build(&[], total, true, min_viable_tokens);
    }

    // Fix 1: pack individually. Push each candidate, keep it if the payload
    // still fits, otherwise pop it back off and count it as omitted for
    // size, then move on to the NEXT request rather than giving up. Every
    // request in `scope.requests` is considered; the token budget is the
    // only bound.
    //
    // Minor 6: `est_tokens(&build(&kept, ...))` re-serializes the WHOLE
    // payload (envelope + every request kept so far) on every iteration, so
    // this loop is quadratic in the number of requests, not linear. Accepted
    // as-is: real sessions measured ~22 requests, where the quadratic factor
    // is noise. If this ever shows up in a profile, the fix is an
    // incremental token accumulator (track the running total's own token
    // estimate and add each candidate's marginal cost, rather than
    // re-measuring the JSON string from scratch) threaded alongside `kept`
    // instead of recomputed here.
    let mut kept: Vec<Value> = Vec::new();
    let mut omitted_for_size = 0usize;
    for r in &scope.requests {
        let mut item = json!({
            // NOT elided: the entire point of this tool is the full text.
            // If a request does not fit, it is dropped whole (below) rather
            // than mutilated.
            "text": r.text,
            "line": r.line_no
        });
        // Same fix as `review_context`'s requests (Important #2): a bare
        // `line` is per-transcript and collides across members once this
        // covers a multi-transcript work unit, which is the default scope.
        // Absent for single-transcript payloads, so that shape is unchanged.
        if let Some(sess) = item_session(s, meta, r.session_ix) {
            item["session"] = json!(sess);
        }
        kept.push(item);
        if est_tokens(&build(&kept, omitted_for_size, false, min_viable_tokens)) > cap {
            kept.pop();
            omitted_for_size += 1;
        }
    }

    build(&kept, omitted_for_size, false, min_viable_tokens)
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
            unit: None,
        }
    }

    /// A `meta()` with a work unit attached. `finding_session` requires
    /// BOTH a multi-transcript session table AND a caller-supplied unit
    /// (item 3): tests that want a `session` key on a finding need this,
    /// not `meta()`.
    fn meta_with_unit() -> SessionMeta {
        SessionMeta {
            id: "abc".into(),
            identified_by: "explicit".into(),
            unit: Some(UnitMeta {
                sessions: 2,
                joined_gaps_min: vec![5.0],
                span_start: "2026-01-01T00:00:00Z".into(),
                span_end: "2026-01-01T01:00:00Z".into(),
                session_ids: vec!["aaaaaaaa".into(), "bbbbbbbb".into()],
                dropped: 0,
                members_unreadable: 0,
                siblings_unplaced: 0,
            }),
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
            (struggle_areas(&s, &r, &m, 10), 1500),
            (file_story(&s, "/a.ts", &m), 1500),
            (blind_spots(&s, &m), 1000),
            (context_health(&s, &m), 1000),
            (evidence(&s, &[Idx(0), Idx(1)], &m), 1500),
        ];
        for (payload, cap) in caps {
            assert_eq!(payload["v"], 2);
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
        let p = struggle_areas(&s, &rank(&s), &meta(), 5);
        assert_eq!(p["v"], 2);
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
        // `churny_session` is a single transcript (`session_ids` has 0
        // entries), so there is nothing to disambiguate: no finding should
        // carry a `session` key. This is what keeps a single-transcript
        // payload byte-identical to v0.1 apart from the version bump.
        assert!(
            p["files"][0]["findings"][0].get("session").is_none(),
            "a single-transcript analysis must not stamp findings with a session"
        );
    }

    /// Builds a two-transcript `Session` by hand (no ingest involved): two
    /// actions, one from each transcript, so `finding_session` has something
    /// real to resolve. `Idx(0)` is transcript "aaaaaaaa", `Idx(1)` is
    /// "bbbbbbbb".
    fn two_transcript_session() -> Session {
        let a0 = Action {
            session_ix: 0,
            ..Default::default()
        };
        let a1 = Action {
            session_ix: 1,
            ..Default::default()
        };
        Session {
            session_ids: vec!["aaaaaaaa".into(), "bbbbbbbb".into()],
            actions: vec![a0, a1],
            ..Session::default()
        }
    }

    #[test]
    fn finding_session_resolves_a_finding_to_the_transcript_its_evidence_came_from() {
        let s = two_transcript_session();
        let m = meta_with_unit();
        assert_eq!(
            finding_session(&s, &m, &[Idx(1)]),
            Some("bbbbbbbb".to_string()),
            "idx 1's action has session_ix 1, which is session_ids[1]"
        );
        assert_eq!(
            finding_session(&s, &m, &[Idx(0)]),
            Some("aaaaaaaa".to_string())
        );
        assert_eq!(
            finding_session(&s, &m, &[]),
            None,
            "no evidence idxs, nothing to resolve"
        );
    }

    #[test]
    fn finding_session_is_none_for_a_single_transcript_analysis() {
        // `session_ids` has 0 or 1 entries for a bare `ingest_str` session:
        // there is only one transcript, so disambiguating it would be noise.
        let s = Session {
            actions: vec![Action::default()],
            ..Session::default()
        };
        assert_eq!(finding_session(&s, &meta_with_unit(), &[Idx(0)]), None);
    }

    #[test]
    fn finding_session_is_none_without_a_work_unit_even_with_two_transcripts() {
        // Item 3 (T7 review): the two conditions are independent. A caller
        // can pass a merged, multi-transcript `Session` (`session_ids.len()
        // >= 2`) while leaving `unit: None`; this is exactly what
        // `backstory-mcp/src/server.rs` does today. Without this check that
        // caller would get `session` keys on findings with no `work_unit`
        // block on the payload to explain them, breaking the schema's
        // guarantee that `session` implies `work_unit`.
        let s = two_transcript_session();
        assert_eq!(
            finding_session(&s, &meta(), &[Idx(1)]),
            None,
            "two transcripts but no work unit supplied: nothing to disclose"
        );
    }

    fn churn_finding_at(idx: Idx, file: &str) -> Finding {
        Finding {
            kind: crate::model::FindingKind::Churn,
            tier: crate::model::Tier::T1,
            exact: true,
            confidence: crate::model::Confidence::High,
            idxs: vec![idx],
            file: Some(file.to_string()),
            note: None,
            nums: Default::default(),
        }
    }

    #[test]
    fn struggle_areas_stamps_a_finding_with_the_transcript_it_came_from() {
        let s = two_transcript_session();
        let file = "/a.ts";
        let ranked = vec![FileScore {
            class: crate::file_class::classify(file),
            edits: 1,
            file: file.into(),
            breakdown: [("churn".to_string(), 1u64)].into_iter().collect(),
            findings: vec![churn_finding_at(Idx(1), file)],
        }];
        let p = struggle_areas(&s, &ranked, &meta_with_unit(), 5);
        assert_eq!(
            p["files"][0]["findings"][0]["session"], "bbbbbbbb",
            "the finding's one proving idx lands on transcript 1"
        );
    }

    #[test]
    fn struggle_areas_omits_session_when_no_work_unit_even_with_two_transcripts() {
        // Item 3 test (as requested): a merged two-transcript `Session` with
        // `unit: None` must produce no `session` key on any finding. Same
        // setup as the test above, `meta()` instead of `meta_with_unit()`.
        let s = two_transcript_session();
        let file = "/a.ts";
        let ranked = vec![FileScore {
            class: crate::file_class::classify(file),
            edits: 1,
            file: file.into(),
            breakdown: [("churn".to_string(), 1u64)].into_iter().collect(),
            findings: vec![churn_finding_at(Idx(1), file)],
        }];
        let p = struggle_areas(&s, &ranked, &meta(), 5);
        assert!(
            p["files"][0]["findings"][0].get("session").is_none(),
            "no work_unit supplied, so no session key may appear: {}",
            p["files"][0]["findings"][0]
        );
    }

    /// `check_payloads.py` only requires the mock fixture's `ranking_rule` to
    /// be non-empty, so editing `score::RANKING_RULE` would leave the fixture
    /// silently stale with CI green. Pin the two together.
    #[test]
    fn mock_fixture_ranking_rule_matches_the_constant() {
        let path: std::path::PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "..",
            "fixtures",
            "mock-payloads",
            "struggle_areas.json",
        ]
        .iter()
        .collect();
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let fixture: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        assert_eq!(
            fixture["ranking_rule"].as_str().unwrap_or_else(|| {
                panic!("{} has no string ranking_rule field", path.display())
            }),
            crate::score::RANKING_RULE,
            "the mock fixture's ranking_rule has drifted from score::RANKING_RULE"
        );
    }

    #[test]
    fn session_overview_top_struggles_carry_class_and_edits() {
        let s = churny_session();
        let ranked = rank(&s);
        let p = session_overview(&s, &ranked, &meta());
        assert_eq!(p["v"], 2);
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
        // No real transcript behind this synthetic `FileScore` data, and a
        // single-transcript `Session::default()` never carries a `session`
        // key on its findings anyway (see `finding_session`).
        let p = struggle_areas(&Session::default(), &ranked, &meta(), usize::MAX);
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
        let p = struggle_areas(&Session::default(), &ranked, &meta(), 5);
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
    fn blind_spots_reports_a_secrets_touch() {
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/repo/.env"}}]}}"#;
        let s = crate::ingest::ingest_str(raw, crate::model::Lane::Main);
        let p = blind_spots(&s, &meta());
        assert_eq!(p["totals"]["secrets_file_touched"], 1);
        assert_eq!(p["secrets_file_touched"][0]["kind"], "secrets_file_touched");
    }

    #[test]
    fn blind_spots_stamps_a_finding_with_the_transcript_it_came_from() {
        // Direct mirror of
        // `struggle_areas_stamps_a_finding_with_the_transcript_it_came_from`
        // (item 5, T7 review): `blind_spots`' stamping went untested even
        // though `blind_spots` is the riskiest judgment call in the change.
        // `blind_spots` computes its own findings via `score::all_findings`
        // rather than taking them from the caller, so this session is built
        // with `ingest_str` (for a real `SecretsFileTouched` finding) and
        // then hand-stamped as transcript 1 of 2, the same way a real
        // work-unit merge would number a second transcript's actions.
        let raw = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"r1","name":"Read","input":{"file_path":"/repo/.env"}}]}}"#;
        let mut s = crate::ingest::ingest_str(raw, crate::model::Lane::Main);
        for a in &mut s.actions {
            a.session_ix = 1;
        }
        s.session_ids = vec!["aaaaaaaa".into(), "bbbbbbbb".into()];
        let p = blind_spots(&s, &meta_with_unit());
        assert_eq!(
            p["secrets_file_touched"][0]["session"], "bbbbbbbb",
            "the finding's one proving idx lands on transcript 1"
        );
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
            unit: None,
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
            unit: None,
        };
        let path = huge_path(0);
        let caps = [
            (session_overview(&s, &ranked, &m), 1000),
            (struggle_areas(&s, &ranked, &m, usize::MAX), 1500),
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

    #[test]
    fn session_overview_emits_a_work_unit_block_and_bumps_the_version() {
        let s = Session::default();
        let meta = SessionMeta {
            id: "bbbbbbbb".into(),
            identified_by: "explicit".into(),
            unit: Some(UnitMeta {
                sessions: 3,
                joined_gaps_min: vec![0.1, -60.0],
                span_start: "2026-01-01T00:00:00Z".into(),
                span_end: "2026-01-01T05:00:00Z".into(),
                session_ids: vec!["aaaaaaaa".into(), "bbbbbbbb".into(), "cccccccc".into()],
                dropped: 0,
                members_unreadable: 0,
                siblings_unplaced: 0,
            }),
        };
        let v = session_overview(&s, &[], &meta);

        assert_eq!(v["v"], 2, "the contract version bumps with the schema");
        let wu = &v["work_unit"];
        assert_eq!(wu["sessions"], 3);
        assert_eq!(wu["joined_gaps_min"][1], -60.0, "overlap reported negative");
        assert_eq!(wu["span_start"], "2026-01-01T00:00:00Z");
        assert!(
            wu["rule"].as_str().unwrap().contains("30 min"),
            "the rule states itself so a reader can audit the grouping"
        );
    }

    #[test]
    fn session_overview_omits_the_work_unit_block_for_one_transcript() {
        let s = Session::default();
        let meta = SessionMeta {
            id: "aaaaaaaa".into(),
            identified_by: "explicit".into(),
            unit: None,
        };
        let v = session_overview(&s, &[], &meta);
        assert_eq!(v["v"], 2, "the version bumps even without a unit block");
        assert!(
            v.get("work_unit").is_none(),
            "a single-transcript analysis has no grouping to disclose"
        );
    }

    #[test]
    fn session_overview_holds_its_cap_with_a_full_work_unit() {
        // Item 4 (T7 review): both adversarial cap tests above pass
        // `unit: None`, so the largest possible `work_unit` block
        // (`MAX_WORK_UNIT_SESSIONS` transcript ids, one fewer gaps) never
        // actually met the cap by construction, only by arithmetic in a
        // reviewer's head. Built with realistic-length uuids and messy,
        // unrounded gap values so item 6's rounding is exercised for real
        // rather than on the fixture's already-tidy numbers.
        let sessions = crate::work_unit::MAX_WORK_UNIT_SESSIONS;
        let s = Session::default();
        let m = SessionMeta {
            id: "abc".into(),
            identified_by: "explicit".into(),
            unit: Some(UnitMeta {
                sessions,
                joined_gaps_min: (0..sessions - 1)
                    .map(|i| (i as f64 + 1.0) * 5.516_666_666_666_667)
                    .collect(),
                span_start: "2026-01-01T00:00:00Z".into(),
                span_end: "2026-01-05T00:00:00Z".into(),
                session_ids: (0..sessions)
                    .map(|i| format!("{i:08x}-aaaa-bbbb-cccc-dddddddddddd"))
                    .collect(),
                dropped: 0,
                members_unreadable: 0,
                siblings_unplaced: 0,
            }),
        };
        let p = session_overview(&s, &[], &m);
        assert!(
            est_tokens(&p) <= CAP_OVERVIEW,
            "over cap: ~{} tokens",
            est_tokens(&p)
        );
        assert_eq!(p["work_unit"]["sessions"], sessions as u64);
        assert_eq!(
            p["work_unit"]["session_ids"].as_array().unwrap().len(),
            sessions
        );
    }

    // ---------------------------------------------------------------------
    // review_context (v3)
    // ---------------------------------------------------------------------

    #[test]
    fn review_context_carries_all_four_blocks_and_true_totals() {
        // "3 shown" must never be mistaken for "3 happened": the same rule
        // blind_spots already holds. Totals are unconditional; lists are a
        // capped sample. Four blocks, not five: the heuristic `constraints`
        // block (Task 9) was cut before implementation.
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache"}}"#;
        let ask = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let ans = r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let task = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Add tests"}}]}}"#;
        // A TaskCreate needs its paired result: the harness-assigned id in
        // `toolUseResult.task.id` is the only source of truth for
        // `TaskEvent::id` (a TaskCreate with no result produces no event at
        // all, per ingest's `a_task_create_with_no_paired_result_produces_no_event`).
        let task_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]},"toolUseResult":{"task":{"id":"1","subject":"Add tests"}}}"#;
        let s = crate::ingest::ingest_str(
            &format!("{human}\n{ask}\n{ans}\n{task}\n{task_result}"),
            crate::model::Lane::Main,
        );
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert_eq!(p["v"], 3, "new contract version");
        assert_eq!(p["scope"]["requests"][0]["text"], "add a cache");
        assert_eq!(p["decisions"][0]["chosen"], "JSONL");
        assert_eq!(
            p["decisions"][0]["options_not_chosen"][0], "SQLite",
            "DecisionOut carries options_not_chosen, not rejected"
        );
        assert!(
            p["decisions"][0]["idxs"].as_array().unwrap().len() == 1,
            "the resolved AskUserQuestion action idx must be citable via evidence()"
        );
        assert_eq!(
            p["incomplete"]["unfinished_tasks"][0]["subject"],
            "Add tests"
        );
        assert_eq!(p["totals"]["decisions"], 1);
        assert_eq!(p["totals"]["unfinished_tasks"], 1);
        // Disclosure fields the brief's snapshot of the types could not know
        // about: unattributed turns (scope) and excluded subagent prose
        // (session-wide), surfaced alongside their respective blocks.
        assert_eq!(p["scope"]["unattributed_turns"], 0);
        assert_eq!(p["totals"]["agent_texts_excluded"], 0);
        // Single-transcript byte-stability guard (Important #2): with no
        // work unit supplied, none of the four blocks may carry a `session`
        // key, and the payload must not carry a `work_unit` block either.
        assert!(p.get("work_unit").is_none());
        assert!(p["scope"]["requests"][0].get("session").is_none());
        assert!(p["decisions"][0].get("session").is_none());
        assert!(
            p["incomplete"]["unfinished_tasks"][0]
                .get("session")
                .is_none()
        );
    }

    #[test]
    fn review_context_discloses_work_unit_and_stamps_every_block_with_its_transcript() {
        // Important #2: both MCP tools and bare `backstory context` cover the
        // WHOLE work unit by default, where `line_no` collides across
        // transcripts. This is the multi-transcript counterpart of
        // `review_context_carries_all_four_blocks_and_true_totals`: same
        // four blocks, but hand-stamped as transcript 1 of 2 (the same
        // pattern `blind_spots_stamps_a_finding_with_the_transcript_it_came_from`
        // uses) so every citation can be checked against `session_ids[1]`.
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache"}}"#;
        let ask = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{"questions":[{"question":"Which store?","options":[{"label":"SQLite"},{"label":"JSONL"}]}]}}]}}"#;
        let ans = r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"JSONL"}}}"#;
        let task = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Add tests"}}]}}"#;
        let task_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]},"toolUseResult":{"task":{"id":"1","subject":"Add tests"}}}"#;
        let claim = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"text","text":"Done, nothing to change."}]}}"#;
        let mut s = crate::ingest::ingest_str(
            &format!("{human}\n{ask}\n{ans}\n{task}\n{task_result}\n{claim}"),
            crate::model::Lane::Main,
        );
        // Hand-stamp as transcript 1 of 2, exactly as a real work-unit merge
        // would number a second transcript's records.
        for a in &mut s.actions {
            a.session_ix = 1;
        }
        for u in &mut s.user_texts {
            u.session_ix = 1;
        }
        for d in &mut s.decisions {
            d.session_ix = 1;
        }
        for t in &mut s.task_events {
            t.session_ix = 1;
        }
        for t in &mut s.agent_texts {
            t.session_ix = 1;
        }
        s.session_ids = vec!["aaaaaaaa".into(), "bbbbbbbb".into()];

        let p = review_context(&s, &meta_with_unit());

        // Same disclosure `session_overview` carries: the payload must say
        // several transcripts were merged, not just stamp individual items.
        assert_eq!(p["work_unit"]["sessions"], 2);
        assert_eq!(p["work_unit"]["session_ids"].as_array().unwrap().len(), 2);

        assert_eq!(
            p["scope"]["requests"][0]["session"], "bbbbbbbb",
            "a bare line collides across transcripts; session disambiguates it"
        );
        assert_eq!(p["decisions"][0]["session"], "bbbbbbbb");
        assert_eq!(
            p["incomplete"]["unfinished_tasks"][0]["session"],
            "bbbbbbbb"
        );
        assert_eq!(p["claims"][0]["session"], "bbbbbbbb");
    }

    #[test]
    fn review_context_tags_a_claim_closed_by_an_interrupt() {
        // A reviewer must be able to tell a completion claim from text cut
        // off mid-turn, or it will try to verify future intent against a
        // diff. `window_interrupted` is the only source of that signal.
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache"}}"#;
        let prose = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"text","text":"Let me check the tests"}]}}"#;
        let interrupt = r#"{"type":"user","timestamp":"2026-01-01T00:00:02Z","message":{"content":"[Request interrupted by user]"}}"#;
        let s = crate::ingest::ingest_str(
            &format!("{human}\n{prose}\n{interrupt}"),
            crate::model::Lane::Main,
        );
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert_eq!(p["claims"][0]["text"], "Let me check the tests");
        assert_eq!(p["claims"][0]["window_interrupted"], true);
    }

    #[test]
    fn review_context_stays_under_its_token_cap_on_a_dense_session() {
        // A real session has 13 human messages and 60 prose blocks. Without
        // shrink_to_fit this payload would blow past every other payload in
        // the crate combined.
        let mut lines = Vec::new();
        for i in 0..40 {
            let text = "z".repeat(400);
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T00:01:{i:02}Z","message":{{"content":[{{"type":"text","text":"{text}"}}]}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert!(
            est_tokens(&p) <= CAP_REVIEW_CONTEXT,
            "payload must fit its cap, got {}",
            est_tokens(&p)
        );
        assert_eq!(p["truncated"], true, "and must say so");
        assert_eq!(p["totals"]["claims"], 40, "true total survives the cap");
    }

    #[test]
    fn review_context_reports_true_totals_for_every_block_when_every_list_is_capped() {
        // Beyond the brief: a session with MORE items than CONTEXT_LIST_MAX
        // in every block at once (requests, decisions, unfinished tasks,
        // failing commands, claims), so a total that silently shrank with
        // its list -- the exact failure mode this discipline exists to
        // prevent -- would be caught here, not just for one block.
        const N: usize = 15; // > CONTEXT_LIST_MAX (12)
        let mut lines = Vec::new();
        let mut ts = 0u32;
        let mut next_ts = || {
            let m = ts / 60;
            let sec = ts % 60;
            ts += 1;
            format!("2026-01-01T00:{m:02}:{sec:02}Z")
        };
        for i in 0..N {
            lines.push(format!(
                r#"{{"type":"user","timestamp":"{}","origin":{{"kind":"human"}},"message":{{"content":"req {i}"}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"text","text":"claim {i}"}}]}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"tool_use","id":"q{i}","name":"AskUserQuestion","input":{{"questions":[{{"question":"Which store {i}?","options":[{{"label":"A"}},{{"label":"B"}}]}}]}}}}]}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"tool_result","tool_use_id":"q{i}"}}]}},"toolUseResult":{{"answers":{{"Which store {i}?":"B"}}}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"tool_use","id":"t{i}","name":"TaskCreate","input":{{"subject":"Task {i}"}}}}]}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"tool_result","tool_use_id":"t{i}"}}]}},"toolUseResult":{{"task":{{"id":"{i}","subject":"Task {i}"}}}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"assistant","timestamp":"{}","message":{{"content":[{{"type":"tool_use","id":"b{i}","name":"Bash","input":{{"command":"cargo test {i}"}}}}]}}}}"#,
                next_ts()
            ));
            lines.push(format!(
                r#"{{"type":"user","timestamp":"{}","message":{{"content":[{{"type":"tool_result","tool_use_id":"b{i}","is_error":true}}]}}}}"#,
                next_ts()
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert_eq!(p["totals"]["requests"], N as u64, "requests total");
        assert_eq!(p["totals"]["decisions"], N as u64, "decisions total");
        assert_eq!(
            p["totals"]["unfinished_tasks"], N as u64,
            "unfinished_tasks total"
        );
        assert_eq!(
            p["totals"]["failing_commands"], N as u64,
            "failing_commands total: distinct validation commands, each failing"
        );
        assert_eq!(p["totals"]["claims"], N as u64, "claims total");
        assert_eq!(p["truncated"], true, "every list above had to be capped");
        assert!(
            p["decisions"].as_array().unwrap().len() < N,
            "the shown list is a sample, strictly smaller than the true total"
        );
        assert!(est_tokens(&p) <= CAP_REVIEW_CONTEXT);
    }

    // ---------------------------------------------------------------------
    // review_context fix wave (adversarial review of b74ca15)
    // ---------------------------------------------------------------------

    #[test]
    fn review_context_survives_an_oversized_option_label_without_starving_other_blocks() {
        // Fix 1: `options_not_chosen` and `last_status` used to be echoed
        // verbatim with no per-string elision. `shrink_to_fit` walks ONE
        // shared `k` down until the payload fits; with an oversized string
        // still present in the payload at every k > 0, the loop had no
        // choice but to walk all the way to k = 0, which empties requests,
        // decisions, incomplete work AND claims together, not just the one
        // oversized decision. This is the property that must survive: a
        // hostile option label or status elsewhere in the payload must not
        // starve the other blocks. RED on the pre-fix code (requests and
        // claims come back empty).
        let huge = "x".repeat(8000);
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache"}}"#;
        let prose = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:01Z","message":{"content":[{"type":"text","text":"Done, added the cache"}]}}"#;
        let ask = format!(
            r#"{{"type":"assistant","timestamp":"2026-01-01T00:00:02Z","message":{{"content":[{{"type":"tool_use","id":"q1","name":"AskUserQuestion","input":{{"questions":[{{"question":"Which store?","options":[{{"label":"safe"}},{{"label":"{huge}"}}]}}]}}}}]}}}}"#
        );
        let ans = r#"{"type":"user","timestamp":"2026-01-01T00:00:03Z","message":{"content":[{"type":"tool_result","tool_use_id":"q1"}]},"toolUseResult":{"answers":{"Which store?":"safe"}}}"#;
        let task = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:04Z","message":{"content":[{"type":"tool_use","id":"t1","name":"TaskCreate","input":{"subject":"Add tests"}}]}}"#;
        let task_result = r#"{"type":"user","timestamp":"2026-01-01T00:00:05Z","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]},"toolUseResult":{"task":{"id":"1","subject":"Add tests"}}}"#;
        let update = r#"{"type":"assistant","timestamp":"2026-01-01T00:00:06Z","message":{"content":[{"type":"tool_use","id":"t2","name":"TaskUpdate","input":{"taskId":"1","status":"blocked"}}]}}"#;
        let update_result = format!(
            r#"{{"type":"user","timestamp":"2026-01-01T00:00:07Z","message":{{"content":[{{"type":"tool_result","tool_use_id":"t2"}}]}},"toolUseResult":{{"success":true,"taskId":"1","statusChange":"{huge}"}}}}"#
        );
        let s = crate::ingest::ingest_str(
            &format!(
                "{human}\n{prose}\n{ask}\n{ans}\n{task}\n{task_result}\n{update}\n{update_result}"
            ),
            crate::model::Lane::Main,
        );
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert!(
            est_tokens(&p) <= CAP_REVIEW_CONTEXT,
            "must hold its cap even with a hostile option label and status, got {}",
            est_tokens(&p)
        );
        assert!(
            !p["scope"]["requests"].as_array().unwrap().is_empty(),
            "a human request must survive an oversized string elsewhere in the payload: {p}"
        );
        assert!(
            !p["claims"].as_array().unwrap().is_empty(),
            "a claim must survive an oversized string elsewhere in the payload: {p}"
        );
        assert_eq!(
            p["totals"]["unfinished_tasks"], 1,
            "the true total is unaffected by the cap either way"
        );
    }

    #[test]
    fn review_context_flags_truncated_when_a_single_field_is_elided_but_no_list_is_capped() {
        // Fix 2: `truncated` used to reflect only list cardinality and the
        // session id, so a single oversized field was elided invisibly. One
        // request, one field over `QUOTE_MAX`, no list anywhere near
        // `CONTEXT_LIST_MAX`.
        let text = "y".repeat(QUOTE_MAX + 1);
        let human = format!(
            r#"{{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
        );
        let s = crate::ingest::ingest_str(&human, crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert_eq!(p["totals"]["requests"], 1, "no list was capped");
        assert!(
            p["scope"]["requests"][0]["text"]
                .as_str()
                .unwrap()
                .contains("chars elided"),
            "the field itself must actually be elided"
        );
        assert_eq!(
            p["truncated"], true,
            "an elided field must flip truncated even when no list hit its cap"
        );
    }

    // ---------------------------------------------------------------------
    // session_intent (v3)
    // ---------------------------------------------------------------------

    #[test]
    fn session_intent_returns_full_requests_and_honours_a_smaller_budget() {
        let mut lines = Vec::new();
        for i in 0..30 {
            let text = "q".repeat(300);
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"{text}"}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let big = session_intent(&s, &meta, None);
        let small = session_intent(&s, &meta, Some(300));

        assert_eq!(big["totals"]["requests"], 30);
        assert_eq!(small["totals"]["requests"], 30, "total never shrinks");
        assert!(
            small["requests"].as_array().unwrap().len() < big["requests"].as_array().unwrap().len(),
            "a smaller budget returns fewer requests"
        );
        assert!(est_tokens(&small) <= 300);
    }

    #[test]
    fn session_intent_stamps_requests_with_their_transcript_only_when_multi_transcript() {
        // Same fix as `review_context`'s requests (Important #2): a bare
        // `line` collides across transcripts once the default whole-work-unit
        // scope covers more than one. Single-transcript call carries no
        // `session` key (byte-stability); the two-transcript call does.
        let human = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"add a cache"}}"#;
        let s = crate::ingest::ingest_str(human, crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };
        let single = session_intent(&s, &meta, None);
        assert!(single["requests"][0].get("session").is_none());

        let mut multi = s;
        for u in &mut multi.user_texts {
            u.session_ix = 1;
        }
        multi.session_ids = vec!["aaaaaaaa".into(), "bbbbbbbb".into()];
        let p = session_intent(&multi, &meta_with_unit(), None);
        assert_eq!(p["requests"][0]["session"], "bbbbbbbb");
    }

    #[test]
    fn session_intent_max_tokens_can_only_lower_the_cap_never_raise_it() {
        // Item 2 (task-11 brief context): a caller-supplied budget larger
        // than CAP_INTENT must be clamped down, never honored as a bigger
        // cap. Verified indirectly: a session whose full text would already
        // fit CAP_INTENT must not swell past it just because the caller
        // asked for more room.
        let mut lines = Vec::new();
        for i in 0..5 {
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"hello {i}"}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let uncapped = session_intent(&s, &meta, None);
        let over_capped = session_intent(&s, &meta, Some(CAP_INTENT * 10));
        assert_eq!(
            uncapped, over_capped,
            "a max_tokens above CAP_INTENT must not raise the effective cap"
        );
    }

    #[test]
    fn session_intent_reports_coverage_incomplete_when_turns_go_unattributed() {
        // The same coverage disclosure review_context carries (item 3,
        // task-11 brief context): session_intent re-derives from the SAME
        // attributed requests scope() already produced, so it cannot recover
        // a turn scope() excluded as unattributed either.
        let unattributed = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"[Request interrupted by user]"}}"#;
        let s = crate::ingest::ingest_str(unattributed, crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = session_intent(&s, &meta, None);
        assert!(
            p["request_coverage"]["unattributed_turns"]
                .as_u64()
                .unwrap()
                > 0,
            "the fixture must actually produce an unattributed turn"
        );
        assert_eq!(p["request_coverage"]["coverage_incomplete"], true);
    }

    #[test]
    fn session_intent_oversized_first_request_does_not_starve_the_rest() {
        // Fix 1 (RED on the pre-fix code): shrink_to_fit's shared-prefix walk
        // (`take(k)`, k falling from a start) means request 0 is a member of
        // EVERY nonempty prefix. If it alone busts the cap, every k > 0 busts
        // the cap too, so the loop is forced all the way to k = 0 and returns
        // NO requests at all, even though requests 1..5 below would fit the
        // budget easily on their own. Individual packing must walk past the
        // oversized one and still keep the small ones.
        let big = "x".repeat(6000);
        let mut lines = vec![format!(
            r#"{{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{{"kind":"human"}},"message":{{"content":"{big}"}}}}"#
        )];
        for i in 1..=5 {
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"hi {i}"}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        // 1000 tokens (~3500 chars) is comfortably bigger than the fixed
        // envelope and the five small requests together, but far smaller
        // than the 6000-char first request alone.
        let p = session_intent(&s, &meta, Some(1000));

        assert_eq!(p["totals"]["requests"], 6, "total never shrinks");
        let kept: Vec<&str> = p["requests"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            kept.len(),
            5,
            "the five small requests must all survive the oversized first one, got {kept:?}"
        );
        for i in 1..=5 {
            assert!(
                kept.contains(&format!("hi {i}").as_str()),
                "request {i} was dropped even though it fits the budget on its own"
            );
        }
        assert!(
            !kept.iter().any(|t| t.starts_with('x')),
            "the oversized request must never be half-quoted into the output"
        );
        assert_eq!(
            p["requests_omitted_for_size"], 1,
            "the omission must be reported unconditionally"
        );
        assert_eq!(p["requests_included"], 5);
        assert_eq!(p["truncated"], true);
    }

    #[test]
    fn session_intent_considers_every_request_no_hidden_cardinality_cap() {
        // Residual 2: a pre-slice to INTENT_LIST_MAX (200) requests used to
        // run BEFORE packing, so a 201st+ request was never considered, not
        // even counted in requests_omitted_for_size, only folded into the
        // generic `truncated` flag. That is a silent cap in a payload whose
        // whole discipline is that nothing shown can be mistaken for
        // everything. With the pre-slice gone, the token budget is the only
        // real bound; a generous budget must return every one of 210 tiny
        // requests, proving no hidden cardinality cap survives.
        let mut lines = Vec::new();
        for i in 0..210 {
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:{:02}:00Z","origin":{{"kind":"human"}},"message":{{"content":"r{i}"}}}}"#,
                i % 60
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = session_intent(&s, &meta, None);

        assert_eq!(p["totals"]["requests"], 210);
        assert_eq!(
            p["requests"].as_array().unwrap().len(),
            210,
            "no hidden cardinality cap: all 210 requests must be considered and fit"
        );
        assert_eq!(p["requests_included"], 210);
        assert_eq!(p["requests_omitted_for_size"], 0);
        assert_eq!(p["truncated"], false);
    }

    #[test]
    fn review_context_reports_coverage_incomplete_when_turns_go_unattributed() {
        // Fix 3: the payload could emit `truncated: false` and point a
        // reviewer at `attributed_intent_via` while `unattributed_turns` or
        // `agent_texts_excluded` was positive, implying a gap that tool
        // could close. It cannot: `attributed_intent_via` re-derives from
        // the SAME attributed requests already shown, never the excluded
        // ones. `coverage` is TOP-LEVEL so this can't be missed a level
        // down.
        let unattributed = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"[Request interrupted by user]"}}"#;
        let s = crate::ingest::ingest_str(unattributed, crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let p = review_context(&s, &meta);
        assert!(
            p["scope"]["unattributed_turns"].as_u64().unwrap() > 0,
            "the fixture must actually produce an unattributed turn"
        );
        assert_eq!(p["coverage"]["coverage_incomplete"], true);
        assert_eq!(
            p["coverage"]["unattributed_turns"],
            p["scope"]["unattributed_turns"]
        );
    }

    #[test]
    fn session_intent_flags_budget_too_small_below_the_zero_request_envelope() {
        // Fix 2: `max_tokens` did not actually bound the output.
        // `shrink_to_fit`'s k = 0 payload (the fixed envelope) was returned
        // unconditionally, so a caller passing a tiny `max_tokens` still
        // got the whole envelope back with no sign the budget was blown.
        // `min_viable_tokens` is the documented floor; below it the payload
        // must say so explicitly rather than silently overshoot.
        let s = crate::ingest::ingest_str(
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","origin":{"kind":"human"},"message":{"content":"hello"}}"#,
            crate::model::Lane::Main,
        );
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        // A generous cap so the normal packing path runs and reports the
        // envelope's real size, unaffected by whether requests fit.
        let baseline = session_intent(&s, &meta, None);
        let envelope = baseline["min_viable_tokens"].as_u64().unwrap();
        assert!(
            envelope > 1,
            "sanity: the envelope must be bigger than the pathological caps below"
        );
        assert_eq!(baseline["budget_too_small"], false);

        for cap in [0u64, 1u64, envelope - 1] {
            let p = session_intent(&s, &meta, Some(cap as usize));
            assert_eq!(
                p["budget_too_small"], true,
                "cap {cap} is below the {envelope}-token envelope and must be flagged"
            );
            assert_eq!(
                p["requests"].as_array().unwrap().len(),
                0,
                "no request can fit alongside an envelope that itself does not fit"
            );
            assert_eq!(p["min_viable_tokens"], envelope);
            assert_eq!(p["truncated"], true);
        }

        let fits = session_intent(&s, &meta, Some(envelope as usize));
        assert_eq!(
            fits["budget_too_small"], false,
            "a cap exactly at the envelope's size must be honored, not flagged"
        );
    }

    #[test]
    fn session_intent_never_overshoots_a_budget_at_the_boundary() {
        // Minor 5 / Fix E: `min_viable_tokens` used to be appended to the
        // payload AFTER the packing loop's `est_tokens(...) > cap` check had
        // already passed, so the field's own tokens were never part of what
        // was measured against `cap`. A budget sitting exactly at (or just
        // above) the true envelope size could come back bigger than the cap
        // it claimed to honor, with `budget_too_small: false`. Now
        // `min_viable_tokens` is threaded through `build` and included in
        // every measurement, so this must hold for every cap at or above the
        // envelope: the returned payload never exceeds the cap it was asked
        // to fit inside.
        let mut lines = Vec::new();
        for i in 0..8 {
            lines.push(format!(
                r#"{{"type":"user","timestamp":"2026-01-01T00:00:{i:02}Z","origin":{{"kind":"human"}},"message":{{"content":"request number {i}, please handle it"}}}}"#
            ));
        }
        let s = crate::ingest::ingest_str(&lines.join("\n"), crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let baseline = session_intent(&s, &meta, None);
        let envelope = baseline["min_viable_tokens"].as_u64().unwrap() as usize;

        for cap in envelope..(envelope + 60) {
            let p = session_intent(&s, &meta, Some(cap));
            assert!(
                est_tokens(&p) <= cap,
                "cap {cap}: payload measured {} tokens, exceeding the budget it \
                 claims to honor (min_viable_tokens field must be part of the \
                 measured envelope, not appended after the fit check)",
                est_tokens(&p)
            );
        }
    }

    #[test]
    fn session_intent_and_review_context_coverage_cannot_contradict() {
        // Fix 3: the two payloads used to fold unattributed_turns (and, for
        // review_context, agent_texts_excluded) into an identically-named
        // `coverage_incomplete` under an identically-named `coverage`
        // object, computed by DIFFERENT rules. The same session could
        // report coverage_incomplete: true from one and false from the
        // other under a field name a consumer would reasonably assume
        // meant the same thing in both. session_intent's object is now
        // `request_coverage`, scoped to the one exposure (unattributed
        // turns) it actually shares with review_context, so the two can
        // never disagree about what they both claim to measure.
        let unattributed = r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"content":"[Request interrupted by user]"}}"#;
        let s = crate::ingest::ingest_str(unattributed, crate::model::Lane::Main);
        let meta = SessionMeta {
            id: "s1".into(),
            identified_by: "explicit".into(),
            unit: None,
        };

        let intent = session_intent(&s, &meta, None);
        let ctx = review_context(&s, &meta);

        assert_eq!(intent["request_coverage"]["coverage_incomplete"], true);
        // The narrower flag can never claim incompleteness the broader one
        // denies: both are driven by the same `unattributed_turns` count,
        // and review_context's flag is a strict superset (it also folds in
        // agent_texts_excluded), so it can never be MORE optimistic.
        if intent["request_coverage"]["coverage_incomplete"] == true {
            assert_eq!(
                ctx["coverage"]["coverage_incomplete"], true,
                "session_intent reports incomplete coverage while review_context reports complete, for the same session"
            );
        }
        assert_eq!(
            intent["request_coverage"]["unattributed_turns"], ctx["coverage"]["unattributed_turns"],
            "both re-derive unattributed_turns from the same context::scope"
        );
    }
}
