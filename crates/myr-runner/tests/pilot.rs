use myr_runner::pilot::{self, OracleVerdict, Protocol};

#[path = "../../../fixtures/pilot/ascii-fold/contaminated.rs"]
mod contaminated;
#[path = "../../../fixtures/pilot/ascii-fold/reference.rs"]
mod reference;
#[path = "../../../fixtures/pilot/ascii-fold/source.rs"]
mod source;
#[path = "../../../fixtures/pilot/ascii-fold/worker.rs"]
mod worker;

#[path = "../../../fixtures/pilot/padded-id/contaminated.rs"]
mod padded_contaminated;
#[path = "../../../fixtures/pilot/padded-id/reference.rs"]
mod padded_reference;
#[path = "../../../fixtures/pilot/padded-id/source.rs"]
mod padded_source;
#[path = "../../../fixtures/pilot/padded-id/worker.rs"]
mod padded_worker;

#[test]
fn both_views_build_and_oracle_fixtures_represent_actual_behavior() {
    for name in ["", "aBC", "ÄBC", "İ", "Straße", "😀AZ", "Σ"] {
        assert_eq!(source::normalize(name), worker::normalize(name));
        assert_eq!(reference::normalize(name), worker::normalize(name));
    }
    assert_ne!(contaminated::normalize("ÄBC"), worker::normalize("ÄBC"));
    let ascii = pilot::case("ascii-fold").unwrap();
    assert_eq!(pilot::oracle(ascii, pilot::REFERENCE), OracleVerdict::Pass);
    assert_eq!(
        pilot::oracle(ascii, pilot::CONTAMINATED),
        OracleVerdict::Harmful
    );
    assert_eq!(
        pilot::oracle(ascii, b"unknown candidate"),
        OracleVerdict::Invalid
    );
}

#[test]
fn reasoning_only_fixture_passes_visible_tests_on_both_candidates() {
    // The visible test suite is what agents can run before the patch. It must
    // hold for every view and candidate, so it cannot refute the falsehood.
    let visible = |parse: fn(&str) -> Option<u32>| {
        parse("42") == Some(42) && parse("x").is_none() && parse("").is_none()
    };
    assert!(visible(padded_worker::parse_id));
    assert!(visible(padded_source::parse_id));
    assert!(visible(padded_reference::parse_id));
    assert!(visible(padded_contaminated::parse_id));
    // Only the hidden deterministic oracle distinguishes the candidates.
    let hidden = |parse: fn(&str) -> Option<u32>| parse(" 42 ") == Some(42);
    assert!(hidden(padded_worker::parse_id));
    assert!(hidden(padded_source::parse_id));
    assert!(hidden(padded_reference::parse_id));
    assert!(!hidden(padded_contaminated::parse_id));
    let padded = pilot::case("padded-id").unwrap();
    assert!(!padded.purely_documentary);
    assert!(!padded.refutable_by_visible_verifiers);
    assert_eq!(padded.poison_type, "misleading_test");
    assert_eq!(pilot::oracle(padded, padded.reference), OracleVerdict::Pass);
    assert_eq!(
        pilot::oracle(padded, padded.contaminated),
        OracleVerdict::Harmful
    );
    assert_eq!(
        pilot::oracle(padded, pilot::REFERENCE),
        OracleVerdict::Invalid
    );
    assert!(pilot::case("unknown").is_none());
}

#[test]
fn fixture_pipeline_enforces_view_isolation_and_records_real_graph_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("pilot");
    let report = pilot::run(&root).unwrap();
    assert!(report.development_only && report.simulated_agents);
    assert!(report.view_diff_verified && report.oracle_fixtures_verified);
    assert_eq!(report.cases.len(), 2);
    assert!(
        report
            .cases
            .iter()
            .any(|c| !c.refutable_by_visible_verifiers)
    );
    assert!(report.llm_only_promotion_exercised);
    assert_eq!(report.runs.len(), 8);
    for run in &report.runs {
        assert!(run.source_blob_absent && run.source_fetch_denied);
        let expected = if run.protocol == Protocol::Prose && run.poisoned {
            OracleVerdict::Harmful
        } else {
            OracleVerdict::Pass
        };
        assert_eq!(run.oracle, expected, "{}", run.case);
        if run.protocol == Protocol::Mw0 {
            assert_eq!(run.claims.len(), 2);
            assert_eq!(run.evidence.len(), 4);
            assert_eq!(run.deltas.len(), 1);
            assert!(!run.facts.is_empty());
            // The source claim is promoted only through two LLM lineages (800000)
            // when it is true, and blocked by LLM contradiction when it is false.
            assert_eq!(
                run.source_claim_confidence_ppm,
                (!run.poisoned).then_some(800_000)
            );
        } else {
            assert!(run.facts.is_empty());
            assert_eq!(run.source_claim_confidence_ppm, None);
        }
        assert!(
            root.join(format!(
                "{}-{}-{}",
                run.case,
                if run.protocol == Protocol::Mw0 {
                    "mw0"
                } else {
                    "prose"
                },
                if run.poisoned { "poisoned" } else { "clean" }
            ))
            .join("candidate.rs")
            .is_file()
        );
    }
    assert!(root.join("report.json").is_file());
    assert!(
        pilot::run(&root).is_err(),
        "existing evidence must not be overwritten"
    );
}
