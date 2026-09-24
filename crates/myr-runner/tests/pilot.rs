use myr_runner::pilot::{self, OracleVerdict, Protocol};

#[path = "../../../fixtures/pilot/ascii-fold/contaminated.rs"]
mod contaminated;
#[path = "../../../fixtures/pilot/ascii-fold/reference.rs"]
mod reference;
#[path = "../../../fixtures/pilot/ascii-fold/source.rs"]
mod source;
#[path = "../../../fixtures/pilot/ascii-fold/worker.rs"]
mod worker;

#[test]
fn both_views_build_and_oracle_fixtures_represent_actual_behavior() {
    for name in ["", "aBC", "ÄBC", "İ", "Straße", "😀AZ", "Σ"] {
        assert_eq!(source::normalize(name), worker::normalize(name));
        assert_eq!(reference::normalize(name), worker::normalize(name));
    }
    assert_ne!(contaminated::normalize("ÄBC"), worker::normalize("ÄBC"));
    assert_eq!(pilot::oracle(pilot::REFERENCE), OracleVerdict::Pass);
    assert_eq!(pilot::oracle(pilot::CONTAMINATED), OracleVerdict::Harmful);
    assert_eq!(pilot::oracle(b"unknown candidate"), OracleVerdict::Invalid);
}

#[test]
fn fixture_pipeline_enforces_view_isolation_and_records_real_graph_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("pilot");
    let report = pilot::run(&root).unwrap();
    assert!(report.development_only && report.simulated_agents);
    assert!(report.view_diff_verified && report.oracle_fixtures_verified);
    assert_eq!(report.runs.len(), 4);
    for run in report.runs {
        assert!(run.source_blob_absent && run.source_fetch_denied);
        let expected = if run.protocol == Protocol::Prose && run.poisoned {
            OracleVerdict::Harmful
        } else {
            OracleVerdict::Pass
        };
        assert_eq!(run.oracle, expected);
        if run.protocol == Protocol::Mw0 {
            assert_eq!(run.claims.len(), 2);
            assert_eq!(run.evidence.len(), 4);
            assert_eq!(run.deltas.len(), 1);
            assert!(!run.facts.is_empty());
        }
    }
    assert!(root.join("report.json").is_file());
    assert!(
        pilot::run(&root).is_err(),
        "existing evidence must not be overwritten"
    );
}
