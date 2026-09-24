//! Terminal reports for user mission inputs rejected before planning.
//! Planner mistakes, configuration errors and transport failures are not proof
//! that the user's goal is invalid.
use crate::{Error, Result, budget, mission, pipeline::ResultContents};
use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Report {
    pub format: String,
    pub state: MissionState,
    pub phase: String,
    pub input: ObjectRef,
    pub failure: ObjectRef,
    #[serde(flatten)]
    pub contents: ResultContents,
    pub budget: budget::Ledger,
}

pub struct Rejection {
    pub report: Report,
    pub private_record: ObjectRef,
}

/// Re-parse the exact submitted bytes before declaring INVALID_GOAL. A caller
/// cannot supply an arbitrary error string or a model's rejection as proof.
/// No configuration, repository snapshot, provider or verifier is consulted.
pub fn reject_mission(graph: &mut Graph, audit: &Store, bytes: &[u8]) -> Result<Rejection> {
    let diagnostic = match mission::parse(bytes) {
        Ok(_) => return Err(invalid("valid mission input cannot be rejected at admission").into()),
        Err(Error::Validation(error)) => error.to_string(),
        Err(error) => return Err(error),
    };
    if audit.root().starts_with(graph.cas().root()) || graph.cas().root().starts_with(audit.root())
    {
        return Err(invalid("admission audit must be separate from shared CAS").into());
    }
    let input = graph.register_artifact(bytes)?;
    let diagnostic = graph.register_artifact_with_dependencies(diagnostic.as_bytes(), &[input])?;
    let failure = graph.insert(
        Authority::Runtime,
        &Object::Fail(Fail {
            class: FailClass::Goal,
            code: FailCode::InvalidGoal,
            diagnostic,
            task: None,
        }),
    )?;
    let provenance = graph.dependencies(failure)?;
    let artifacts = provenance
        .iter()
        .copied()
        .filter(|r| r.kind == Kind::Artifact)
        .collect();
    let report = Report {
        format: "myr-admission-failure-v0".into(),
        state: MissionState::InvalidGoal,
        phase: "admission".into(),
        input,
        failure,
        contents: ResultContents {
            artifacts,
            evidence: vec![],
            active_assumptions: vec![],
            provenance,
        },
        budget: budget::Ledger {
            calls_started: 0,
            input_reference_tokens: 0,
            output_reference_tokens: 0,
            uncertain_output_reservation: 0,
            output_accounting_complete: true,
        },
    };
    let private_record = audit.put_artifact(&serde_json::to_vec(&report)?)?;
    Ok(Rejection {
        report,
        private_record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_cannot_reject_valid_or_ambiguous_natural_language_goals() {
        let dir = tempfile::tempdir().unwrap();
        let mut graph = Graph::open(
            dir.path().join("graph.sqlite"),
            Store::open(dir.path().join("shared")).unwrap(),
        )
        .unwrap();
        let audit = Store::open(dir.path().join("private")).unwrap();
        // Contradictory-looking prose is not a machine-checked UNSAT proof.
        for bytes in [
            b"goal: Make it better\nverify: [cargo test]".as_slice(),
            b"goal: Both preserve and remove every file\nverify: [cargo test]".as_slice(),
        ] {
            assert!(reject_mission(&mut graph, &audit, bytes).is_err());
            assert!(graph.resolve(myr_wire::artifact_cid(bytes)).is_err());
        }
        assert_eq!(std::fs::read_dir(audit.root()).unwrap().count(), 0);
        let shared = graph.cas().clone();
        assert!(reject_mission(&mut graph, &shared, b"not a mission").is_err());
        assert!(
            graph
                .resolve(myr_wire::artifact_cid(b"not a mission"))
                .is_err()
        );
    }
}
