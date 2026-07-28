//! A deliberately generous ceiling on work-unit analysis.
//!
//! WHY GENEROUS: this exists to catch an algorithmic regression, such as an
//! accidental O(n^2) merge or a re-read of every member per member. It is not
//! here to police tens of milliseconds, because a timing assertion tight
//! enough to do that is flaky on shared CI runners and would be deleted after
//! the third spurious failure. The real numbers live in the spec's section 9.

use std::time::Instant;

#[test]
fn a_sixteen_member_work_unit_analyzes_well_under_the_ceiling() {
    let td = tempfile::tempdir().unwrap();
    // 16 transcripts, 400 edits each: 6400 actions, comfortably more than the
    // largest real unit's action count. One action per second, back to back
    // across transcripts, so every consecutive pair sits within the 30 minute
    // idle gap and the grouping rule joins all 16 into ONE unit. (Spacing the
    // transcripts an hour apart would silently split them and this test would
    // then measure a 400-action unit while claiming 6400.)
    for s in 0..16u32 {
        let id = format!("{s:08}-1111-2222-3333-444455556666");
        let mut body = String::new();
        for i in 0..400u32 {
            let t = s * 400 + i;
            body.push_str(&format!(
                r#"{{"type":"assistant","timestamp":"2026-01-01T{:02}:{:02}:{:02}Z","sessionId":"{id}","message":{{"content":[{{"type":"tool_use","id":"e{s}_{i}","name":"Edit","input":{{"file_path":"/f{}.rs","old_string":"a","new_string":"b"}}}}]}}}}"#,
                t / 3600,
                (t % 3600) / 60,
                t % 60,
                i % 20
            ));
            body.push('\n');
        }
        std::fs::write(td.path().join(format!("{id}.jsonl")), body).unwrap();
    }
    let last = td.path().join("00000015-1111-2222-3333-444455556666.jsonl");

    let t0 = Instant::now();
    let a = sumcp_core::assemble::load_work_unit(&last, sumcp_core::assemble::MAX_TRANSCRIPT_BYTES)
        .expect("assembles");
    let dt = t0.elapsed();

    assert_eq!(
        a.unit.members.len(),
        16,
        "all 16 transcripts joined one unit"
    );
    assert_eq!(a.session.actions.len(), 6400, "every action merged");
    assert!(
        dt.as_secs_f64() < 10.0,
        "16-member unit took {dt:?}; the ceiling is 10s and exists to catch \
         an algorithmic regression, not to police milliseconds"
    );
}
