//! The session model: ordered actions with first-class ordering.
//!
//! Ordering contract (SPEC decision 2 + amendment 5): actions are sorted by
//! `(effective_timestamp, agent lane [main first], source line number)`.
//! `effective_timestamp` is the line's own timestamp or the last-seen one
//! carried forward — because ~20% of real lines have none. Source line number
//! is always present and monotonic, so the order is *total and deterministic*
//! even under missing or tied timestamps. `Idx` is the stable handle every
//! finding cites as evidence and every payload exposes for `evidence()`.

use serde::{Deserialize, Serialize};

/// Stable index of an action in the session's total order.
///
/// Monotonic within a session; findings cite `Idx` values as evidence and
/// the `evidence()` MCP tool dereferences them back to raw actions.
// `derive` auto-generates trait impls: Ord/PartialOrd give us `<`/sorting,
// Serialize/Deserialize give JSON conversion, Copy makes it cheap to pass by
// value (it's just a u32 under the hood).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)] // serializes as the bare number `102`, not `{"0":102}`
pub struct Idx(pub u32);

/// Which lane an action came from: the main agent or a named subagent.
///
/// The derived `Ord` orders variants by declaration order, so `Main` sorts
/// before every `Sub(..)` — exactly the "main first" tie-break the ordering
/// contract wants. `Sub`s then order by their id string.
// `Default` (via `#[default]` on `Main`) is what lets `#[serde(default)]` on
// `TaskEvent::lane` fall back to something when deserializing a disk cache
// written before that field existed. `Main` is the right fallback: those old
// cache entries only ever came from a main-lane ingest (no subagent
// transcript has ever created a task in the live corpus).
#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Lane {
    /// The primary session transcript.
    #[default]
    Main,
    /// A subagent transcript, identified by its agent id.
    Sub(String),
}

/// One subagent spawn recorded in the MAIN transcript (an `Agent`/`Task`
/// tool call). We keep the spawn's `agentId` because the legacy on-disk
/// layout names the child transcript `agent-<agentId>.jsonl` — that id is
/// the only link from a spawn to its file. `None` when the spawn's result
/// had not come back yet (subagent still running) or carried no agentId.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spawn {
    /// The child agent's id, from the spawn's `toolUseResult.agentId`.
    pub agent_id: Option<String>,
}

/// The kind of thing an action is — kept coarse for v0.1's overview.
///
/// `Other` keeps the original tool name so nothing is silently dropped when a
/// new tool appears (schema drift is expected, not exceptional).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionKind {
    /// A `Read` tool call.
    Read,
    /// An `Edit` tool call.
    Edit,
    /// A `Write` tool call.
    Write,
    /// A `Bash` tool call.
    Bash,
    /// Any other tool, preserving its reported name.
    Other(String),
}

impl ActionKind {
    /// Classify a raw tool name into an [`ActionKind`].
    pub fn from_tool(name: &str) -> Self {
        // `match` is exhaustive — the compiler checks every path returns a
        // value, and the final `other` arm binds the leftover name.
        match name {
            "Read" => ActionKind::Read,
            "Edit" => ActionKind::Edit,
            "Write" => ActionKind::Write,
            "Bash" => ActionKind::Bash,
            other => ActionKind::Other(other.to_string()),
        }
    }
}

/// One agent action (a single tool call) placed in the session's total order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    /// Position in the session's total order.
    pub idx: Idx,
    /// Timestamp used for ordering (own, or carried forward if the line had none).
    pub effective_ts: String,
    /// Whether this line carried its own timestamp (vs an inherited one).
    pub ts_inherited: bool,
    /// Main or subagent lane.
    pub lane: Lane,
    /// Which transcript of the work unit this action came from: an index into
    /// [`Session::session_ids`].
    ///
    /// WHY AN INDEX AND NOT A STRING: a work unit holds at most 16 sessions
    /// but tens of thousands of actions. A `String` here would mean one heap
    /// allocation and about 24 bytes per action; a `u16` is 2 bytes and no
    /// allocation. Payloads look the real id up in the table when they need
    /// to print it. `0` for a single-transcript analysis.
    #[serde(default)]
    pub session_ix: u16,
    /// Original 0-based line number in its source transcript (the total-order tiebreak).
    pub line_no: usize,
    /// What the action did.
    pub kind: ActionKind,
    /// The file the tool acted on, if any (`file_path` input).
    pub file_path: Option<String>,
    /// Whether the tool result was an error (`is_error: true`), if known.
    pub is_error: Option<bool>,
    /// Size in chars of a Write/Edit's new content, if known (large-write signal).
    pub write_len: Option<usize>,
    /// Line count of a Write/Edit's FULL new content, counted at ingest
    /// *before* `edit_new` gets capped — so it stays accurate for huge writes.
    /// Consumers: review-burden (#27), relative churn (#7).
    pub write_lines: Option<usize>,
    /// For Read actions: the file's total line count as the harness reported
    /// it (`toolUseResult.file.totalLines`, a T2 field). This is the file's
    /// real size even when the Read itself was partial (offset/limit).
    /// Consumer: relative-churn denominator (#7).
    pub read_total_lines: Option<usize>,
    /// Hash of (tool name + raw serialized input JSON). Byte-identical calls
    /// hash equal — that's all the loop detector (#21) needs. Not a content
    /// fingerprint; never surfaced in payloads.
    pub input_hash: Option<u64>,
    /// Error text from the tool result, if it errored (drives fumble detection).
    pub error: Option<String>,
    /// Edited line ranges `(start, end)` from `structuredPatch` (rework signal).
    pub hunks: Vec<(u32, u32)>,
    /// The Bash command string, if this is a Bash action (failure attribution).
    pub command: Option<String>,
    /// Whether the user hand-modified this tool's result (`userModified: true`).
    pub user_modified: bool,
    /// Normalized+capped `old_string` of an Edit (revert detection).
    pub edit_old: Option<String>,
    /// Normalized+capped `new_string` of an Edit (revert detection).
    pub edit_new: Option<String>,
    /// Seconds from proposing this Edit/Write to its result — the approval
    /// latency heuristic (execution ≈ instant, so this ≈ human decision time).
    pub approval_latency_s: Option<f64>,
    /// Whether an auto-accept permission mode was in force when THIS action
    /// ran. The mode changes within a session (Claude Code emits a `mode`
    /// event each time), so latency suppression is per action; the
    /// session-level [`Session::auto_accept`] keeps meaning "ever seen".
    /// `#[serde(default)]` (false) so transcript caches written before this
    /// field existed still deserialize.
    #[serde(default)]
    pub auto_accept_here: bool,
}

impl Action {
    /// The identity an adjacency comparison must use.
    ///
    /// Returns the pair (originating transcript, lane within it). Two actions
    /// belong to the same lane only when BOTH match. Every comparison that
    /// asks "were these two actions produced by the same agent in sequence"
    /// must use this rather than `.lane`, or a work unit will let a finding
    /// span two different transcripts.
    pub fn lane_key(&self) -> (u16, &Lane) {
        (self.session_ix, &self.lane)
    }
}

// This `Default` impl is `#[cfg(test)]`-only on purpose: it exists so unit
// tests can build a throwaway `Action` and only override the two or three
// fields the test actually cares about, instead of spelling out all twenty
// fields every time. It is NOT part of the public API a library consumer
// sees, because "give me a plausible-looking blank action" is a testing
// convenience, not a real operation any caller outside this crate needs.
// Gating it like this keeps the crate's real surface smaller than it would
// be if every helper we wrote for our own tests leaked out to consumers.
#[cfg(test)]
impl Default for Action {
    fn default() -> Self {
        Action {
            idx: Idx(0),
            effective_ts: String::new(),
            ts_inherited: false,
            lane: Lane::Main,
            session_ix: 0,
            line_no: 0,
            // `ActionKind` has no meaningful "default" action kind, so this
            // is a placeholder every real test overwrites explicitly before
            // asserting anything that depends on what kind of action it is.
            kind: ActionKind::Other(String::new()),
            file_path: None,
            is_error: None,
            write_len: None,
            write_lines: None,
            read_total_lines: None,
            input_hash: None,
            error: None,
            hunks: vec![],
            command: None,
            user_modified: false,
            edit_old: None,
            edit_new: None,
            approval_latency_s: None,
            auto_accept_here: false,
        }
    }
}

/// Token accounting, summed once per `message.id` (dedup layer a).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Tokens {
    /// Uncached input tokens.
    pub input: u64,
    /// Generated output tokens.
    pub output: u64,
    /// Tokens served from cache.
    pub cache_read: u64,
    /// Tokens written to cache.
    pub cache_creation: u64,
}

impl Tokens {
    /// Cache-hit ratio: cache reads over all input-side tokens. `None` when
    /// there is no input-side traffic to divide by (avoids 0/0).
    pub fn cache_hit_ratio(&self) -> Option<f64> {
        let denom = self.input + self.cache_read + self.cache_creation;
        // `?`-free guard: return None rather than divide by zero.
        if denom == 0 {
            None
        } else {
            Some(self.cache_read as f64 / denom as f64)
        }
    }
}

/// Field-reliability tier (metrics-spec parser rules): T1 stable, T2 needs
/// edge handling, T3 unstable. Every finding declares the tier of the data it
/// rests on, so a schema break is triaged by blast radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    /// Stable fields.
    #[serde(rename = "T1")]
    T1,
    /// Fields needing edge handling.
    #[serde(rename = "T2")]
    T2,
    /// Unstable/undocumented fields.
    #[serde(rename = "T3")]
    T3,
}

/// Confidence in a finding; low-confidence findings count ×0.5 in ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    /// Directly evidenced.
    High,
    /// Reasonable inference.
    Medium,
    /// Weak attribution.
    Low,
}

/// The kind of finding — serializes to the exact strings in the payload enum
/// (`docs/payload-schema.md`), so Rust output matches the frozen contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Repeat edits to one file.
    Churn,
    /// A later edit overlapping an earlier edit's lines.
    Rework,
    /// A file re-read many times. Renamed from `Thrash` (2026-07-18) so our
    /// corpus-grounded re-read count never masquerades as the literature's
    /// stuck-in-loop metric (which is `ActionLoop`). Serializes as `re_read`.
    ReRead,
    /// An edit attempted before the file was read (harness-blocked).
    BlindWriteAttempt,
    /// Repeated failing commands attributed to a file.
    FailureLoop,
    /// A later edit restores content an earlier edit had removed.
    TrueRevert,
    /// A revert right after user pushback (a capitulation flip).
    Flip,
    /// The user hand-corrected the agent's edit.
    UserCorrected,
    /// The session's opening move (read-first vs patch-first).
    OpeningMove,
    /// A large write accepted almost instantly (comprehension debt).
    LargeWriteInstantAccept,
    /// ≥3 consecutive byte-identical tool calls in one lane (metrics-spec
    /// #21, SEAlign definition). Always advisory: emitted with
    /// `Confidence::Low` so ranking applies `low_confidence_factor`.
    ActionLoop,
    /// More agent-written lines between two human turns than the 200–400 LOC
    /// human review band (metrics-spec #27, the comprehension-layer anchor).
    /// Frames risk ("plausibly could not have been reviewed"), never verdict.
    ReviewBurden,
    /// A credentials or key file was read, edited, or written. Zero-tolerance
    /// by design: one occurrence is the entire signal, so it solo-qualifies
    /// for review rather than needing a second finding. Surfaced through
    /// `blind_spots`, not through the ranking, because the ranking puts
    /// `Config` last and burying this would defeat the point.
    SecretsFileTouched,
}

/// One evidence-backed observation about the session.
///
/// Every finding carries the action `idxs` proving it — the honesty invariant.
/// `note` explains heuristics; `file` scopes file-level findings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// What was observed.
    pub kind: FindingKind,
    /// Reliability tier of the underlying fields.
    pub tier: Tier,
    /// True = deterministic count; false = heuristic (requires a `note`).
    pub exact: bool,
    /// Confidence in the finding.
    pub confidence: Confidence,
    /// Action indices proving it (dereferenceable via `evidence()`).
    pub idxs: Vec<Idx>,
    /// The file this finding is about, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Human-readable explanation (required when `exact` is false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Numeric operationalizations backing the finding (e.g.
    /// `edit_fraction_first10`, `first_edit_index`, `relative_churn`, `loc`).
    /// One map instead of bespoke fields per kind — payloads stay uniform.
    /// `skip_serializing_if` keeps existing findings' JSON unchanged (an
    /// empty map serializes to nothing); `default` lets old JSON deserialize.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty", default)]
    pub nums: std::collections::BTreeMap<String, f64>,
}

/// A user text message, placed in time so signals can ask "did the user push
/// back between these two edits?".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserText {
    /// Effective timestamp (own or carried forward).
    pub effective_ts: String,
    /// Source line number (total-order tiebreak, same key as actions).
    pub line_no: usize,
    /// The message text.
    pub text: String,
    /// Which transcript of the work unit this message came from: an index
    /// into [`Session::session_ids`], exactly like [`Action::session_ix`]
    /// (see that field's doc for the full reasoning: a `u16` index instead
    /// of a `String` id, `0` for a single-transcript analysis).
    ///
    /// `ingest_str` does not know its own transcript id, so it always
    /// leaves this at the default `0`; the work-unit merge step (a later
    /// task) stamps the real index once several transcripts are assembled
    /// together, the same way it does for `Action::session_ix`.
    #[serde(default)]
    pub session_ix: u16,
    /// Whether this turn came from the human, per the `origin.kind` field.
    /// A harness-injected turn (e.g. a task notification) is recorded but is
    /// not a human turn, so it must not reset the review-burden window.
    /// Absent `origin` means human: the field is newer than the transcripts
    /// we must keep reading, and defaulting an unknown turn to human keeps
    /// the pre-existing (wider) windowing on old data. The serde default is
    /// `true` for the same reason.
    #[serde(default = "default_true")]
    pub is_human: bool,
}

/// Serde default helper: see [`UserText::is_human`].
fn default_true() -> bool {
    true
}

/// One question the agent put to the human, the options it offered, and what
/// the human picked.
///
/// WHY THIS IS ITS OWN VEC AND NOT A FIELD ON `Action`: a session has tens of
/// thousands of actions and a handful of decisions. Hanging the question and
/// option text off every action would cost memory on every one of them to
/// serve a few. This follows the same shape as `Session::spawns`, which is
/// also a small paired-with-its-result list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Decision {
    /// The question text, verbatim.
    pub question: String,
    /// The option labels offered, in the order presented.
    pub options: Vec<String>,
    /// What the human chose. `None` when the session ended before answering.
    /// This is NOT constrained to be one of `options`: the "Other" escape
    /// hatch lets the human answer in free text, and that text is the most
    /// informative answer of all, so it is kept exactly as written.
    pub answer: Option<String>,
    /// Source line of the asking call (total-order tiebreak, same key as
    /// actions and user texts).
    pub line_no: usize,
    /// Which transcript of the work unit this came from. See
    /// [`Action::session_ix`] for why this is an index and not a `String`.
    #[serde(default)]
    pub session_ix: u16,
    // Deliberately no `Idx` field here. Both `merge_sessions` and
    // `merge_work_unit` renumber every action's `Idx` globally, so an index
    // assigned at ingest time would go stale the moment a merge ran, and a
    // stale citation is worse than an absent one. `line_no` + `session_ix`
    // above are stable across merges; a later task resolves the real,
    // current `Idx` from the MERGED session by matching on that pair. Do
    // not re-add `idx` here without solving the staleness problem first.
}

/// One transition in a task's lifecycle: its creation, or a status change.
///
/// Kept as an event LIST rather than a final-state map because the payload
/// needs to cite the action that left a task unfinished, and a map would
/// have thrown that index away.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskEvent {
    /// The task id, exactly as the harness reported it: `toolUseResult.task.id`
    /// on a create, `taskId` on an update. Each transcript's harness numbers
    /// ids from 1 independently, so this id is only unique WITHIN one
    /// (session_ix, lane) pair, never across a merged work unit; see `lane`
    /// below.
    pub id: String,
    /// The subject, present on creates and on renames, absent on plain
    /// status updates.
    pub subject: Option<String>,
    /// `"pending"` for a create, otherwise the confirmed status an update's
    /// result reported.
    pub status: String,
    /// Source line (total-order tiebreak).
    pub line_no: usize,
    /// Which transcript of the work unit this came from.
    #[serde(default)]
    pub session_ix: u16,
    /// Main or subagent lane, stamped from the ingest call's `default_lane`
    /// exactly like `Action::lane`. `merge_sessions` concatenates a
    /// subagent's task events into the main lane's without renumbering
    /// anything, and each transcript's harness numbers its own task ids from
    /// 1, so a subagent's task "1" and the main lane's task "1" would
    /// otherwise share an identity. Task identity is therefore
    /// `(session_ix, lane, id)`, not `id` alone.
    #[serde(default)]
    pub lane: Lane,
    // Deliberately no `Idx` field here, same reasoning as `Decision` above:
    // both merge functions renumber every action's `Idx` globally, so an
    // index captured at ingest time would go stale the moment a merge ran.
    // `line_no` + `session_ix` are stable across merges; a later task
    // resolves the real, current `Idx` from the MERGED session by matching
    // on that pair.
}

/// One block of agent prose: what it said it did.
///
/// The review payload hands these to the reviewer as CLAIMS to verify against
/// the diff. suMCP never checks them itself, because checking a natural
/// language assertion against code requires understanding both, which is the
/// consuming agent's job and not this tool's.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentText {
    /// The prose, capped at `AGENT_TEXT_CAP` characters.
    pub text: String,
    /// Source line (total-order tiebreak).
    pub line_no: usize,
    /// Which transcript of the work unit this came from.
    #[serde(default)]
    pub session_ix: u16,
}

/// A fully parsed session: ordered actions plus parse-health counters.
///
/// `Default` is `#[cfg(test)]`-only, same reasoning as `Action`'s: it lets a
/// unit test build a throwaway `Session` and set only the two or three
/// fields it cares about (usually `actions` and `session_ids`), instead of
/// spelling out this whole struct every time. Unlike `Action`, every field
/// here already implements `Default` on its own, so the derive does the
/// work instead of a hand-written impl.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(Default))]
pub struct Session {
    /// All actions, in the total order (so `actions[i].idx == Idx(i)`).
    pub actions: Vec<Action>,
    /// User text messages, in source order (pushback/flip detection).
    pub user_texts: Vec<UserText>,
    /// The working directory recorded in the transcript (first `cwd` field
    /// seen). Used by the HTML report header; `None` for synthetic sessions.
    #[serde(default)]
    pub cwd: Option<String>,
    /// Token totals (deduped per `message.id`).
    pub tokens: Tokens,
    /// Histogram of every event `type` seen (including ones we don't model).
    pub type_counts: std::collections::BTreeMap<String, u64>,
    /// Lines that were not valid JSON (counted, never fatal).
    pub parse_errors: u64,
    /// How many lines carried no timestamp (amendment 5 visibility).
    pub untimestamped_lines: u64,
    /// User interruptions (`[Request interrupted by user`).
    pub interrupts: u64,
    /// Whether an auto-accept permission mode was ever seen. When true,
    /// approval-latency signals are suppressed (the delta means nothing).
    pub auto_accept: bool,
    /// This session's direct subagent spawns (`Agent`/`Task` calls in the
    /// MAIN transcript), post-dedup. Used by assembly to find and merge the
    /// child transcripts; carried through the merge as provenance.
    pub spawns: Vec<Spawn>,
    /// Questions the agent put to the human, with the options offered and the
    /// answer given. The review-context payload's highest-value block: it is
    /// the only place a deliberate human choice is recorded, so a reviewer
    /// can stop flagging it as a mistake.
    ///
    /// `#[serde(default)]` so transcript caches written before this field
    /// existed still deserialize.
    #[serde(default)]
    pub decisions: Vec<Decision>,
    /// Task lifecycle transitions, in source order. Replayed by the context
    /// module into a final state per task, so the payload can report work
    /// that was planned and never finished.
    #[serde(default)]
    pub task_events: Vec<TaskEvent>,
    /// Agent prose blocks, in source order: what the agent said it did. The
    /// payload presents these as claims for the reviewer to check against
    /// the diff.
    #[serde(default)]
    pub agent_texts: Vec<AgentText>,
    /// The transcript ids making up this session, oldest first. An action's
    /// `session_ix` indexes into this. A single-transcript analysis has
    /// exactly one entry, so `session_ids[0]` is always the id being reported.
    ///
    /// NOTE: a bare `ingest_str` call does not know its own transcript id, so
    /// it leaves this empty (see `ingest.rs`); the merge step (a later task)
    /// fills it in once several transcripts are assembled into one work unit.
    /// Anything reading `session_ids` must tolerate an empty table, and
    /// `Action::lane_key` deliberately never indexes into it (it just returns
    /// the raw `u16`), so an empty table can never cause an out-of-bounds
    /// lookup.
    #[serde(default)]
    pub session_ids: Vec<String>,
    /// Subagent spawns whose transcript could not be turned into analyzed
    /// actions (file not found / unreadable / oversized / parsed to zero
    /// actions / over the file-count cap). Honest scope disclosure, surfaced
    /// as `subagent_files_missing` in `session_overview`. `0` from a bare
    /// `ingest_str`; set by `merge_sessions` from the assembly's count.
    pub subagent_files_missing: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idx_is_ordered_and_serializes_transparently() {
        assert!(Idx(2) > Idx(1));
        assert_eq!(serde_json::to_string(&Idx(102)).unwrap(), "102");
        assert_eq!(serde_json::from_str::<Idx>("102").unwrap(), Idx(102));
    }

    #[test]
    fn main_lane_sorts_before_subagents() {
        let mut lanes = vec![Lane::Sub("b".into()), Lane::Main, Lane::Sub("a".into())];
        lanes.sort();
        assert_eq!(
            lanes,
            vec![Lane::Main, Lane::Sub("a".into()), Lane::Sub("b".into())]
        );
    }

    #[test]
    fn cache_hit_ratio_guards_against_zero() {
        assert_eq!(Tokens::default().cache_hit_ratio(), None);
        let t = Tokens {
            input: 10,
            cache_read: 90,
            ..Default::default()
        };
        assert_eq!(t.cache_hit_ratio(), Some(0.9));
    }

    // Both tests below build an `Action` with `::default()` and then
    // overwrite a couple of fields by hand, one at a time, rather than
    // spelling out a full struct literal. Clippy's `field_reassign_with_default`
    // lint would rather we write `Action { lane: ..., session_ix: ...,
    // ..Default::default() }` in one go. Here that would bury the exact two
    // fields the test cares about inside a longer literal, so the
    // field-by-field version is kept for readability and the lint is
    // silenced explicitly instead.
    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn lane_key_separates_the_main_lanes_of_two_sessions() {
        // WHY THIS EXISTS: `Lane::Main == Lane::Main` is true across two
        // different transcripts. Every adjacency-based finding (true_revert,
        // flip, failure proximity) compares lanes, so without the session tag
        // an edit in session 0 would read as the same lane as one in session 1
        // and a revert could fire across a boundary it must never cross.
        let mut a = Action::default();
        a.lane = Lane::Main;
        a.session_ix = 0;
        let mut b = Action::default();
        b.lane = Lane::Main;
        b.session_ix = 1;

        assert_eq!(a.lane, b.lane, "the lanes alone are equal");
        assert_ne!(a.lane_key(), b.lane_key(), "the lane keys must differ");
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn lane_key_matches_within_one_session() {
        let mut a = Action::default();
        a.lane = Lane::Sub("x".into());
        a.session_ix = 2;
        let mut b = Action::default();
        b.lane = Lane::Sub("x".into());
        b.session_ix = 2;
        assert_eq!(a.lane_key(), b.lane_key());
    }
}
