//! Runtime-only command evidence, bound to candidate and sealed policy. Raw
//! host/container metadata stays in a separate audit store, outside agent CAS.
use crate::{
    Result,
    candidate::{self, RecordedCandidate},
    docker_sandbox::{self, DockerRuntime, Execution},
    goal,
    verifier::{self, VerifierPolicy},
};
use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("command verification: {0}")]
    Runner(#[from] crate::Error),
    #[error("command execution: {0}")]
    Execution(#[from] docker_sandbox::Error),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub format: String,
    pub candidate: ObjectRef,
    pub policy: ObjectRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measurement: Option<ObjectRef>,
    pub exit_code: i64,
    pub stdout: ObjectRef,
    pub stderr: ObjectRef,
    /// Reference in the private audit CAS, deliberately not a shared graph edge.
    pub private_audit: ObjectRef,
}

#[derive(Debug, Serialize)]
pub struct Verification {
    pub record: ObjectRef,
    pub claim: ObjectRef,
    pub evidence: ObjectRef,
}

struct Target {
    claim: Claim,
    policy_ref: ObjectRef,
    policy: VerifierPolicy,
    measurement: Option<(ObjectRef, crate::measurement::Procedure, Decimal)>,
}

impl Target {
    fn verdict(&self, exit_code: i64, stdout: &[u8]) -> Result<Verdict> {
        let satisfied = if let Some((_, procedure, tolerance)) = &self.measurement {
            if exit_code != 0 {
                return Err(invalid("measurement command did not finish successfully").into());
            }
            crate::measurement::evaluate(procedure, tolerance, stdout)?.1
        } else {
            exit_code == 0
        };
        Ok(if satisfied == self.claim.polarity {
            Verdict::Supports
        } else {
            Verdict::Contradicts
        })
    }
}

fn target(graph: &Graph, candidate: ObjectRef, atom: ObjectRef) -> Result<Target> {
    let manifest = candidate::load_manifest(graph, candidate)?;
    let sealed = goal::load(graph, manifest.scope.goal)?;
    let criterion = sealed
        .ir()
        .criteria
        .iter()
        .find(|c| c.binding && c.atom == atom)
        .ok_or_else(|| invalid("command atom is not a sealed binding criterion"))?;
    let Object::Atom(value) = graph.get(atom)? else {
        return Err(invalid("command target is not ATOM").into());
    };
    let Object::PredicateDef(predicate) = graph.get(value.predicate)? else {
        return Err(invalid("command predicate is absent").into());
    };
    let measurement = if predicate.class == PredicateClass::Empirical {
        let m = criterion
            .measurement
            .as_ref()
            .ok_or_else(|| invalid("empirical criterion lacks a measurement"))?;
        Some((
            m.procedure,
            crate::measurement::load(graph, m.procedure, sealed.policy())?,
            m.tolerance,
        ))
    } else {
        None
    };
    let policy_ref = if let Some((_, procedure, _)) = &measurement {
        procedure.command
    } else {
        if predicate != core_predicates().remove(0) {
            return Err(invalid(
                "command evidence requires command_succeeds or an empirical procedure",
            )
            .into());
        }
        let [Argument::Ref(reference)] = value.arguments.as_slice() else {
            return Err(invalid("command atom requires one policy reference").into());
        };
        *reference
    };
    if !sealed.policy().verifier_policies.contains(&policy_ref) {
        return Err(invalid("command policy is not sealed").into());
    }
    let policy = verifier::load_policy(graph, policy_ref, sealed.policy().time_budget_ms)?;
    if !matches!(policy, VerifierPolicy::Command { .. }) {
        return Err(invalid("command policy required").into());
    }
    Ok(Target {
        claim: Claim {
            atom,
            polarity: criterion.polarity,
            scope: manifest.scope,
        },
        policy_ref,
        policy,
        measurement,
    })
}

/// Only the actual executor can reach production recording. Infrastructure
/// errors return before semantic evidence is inserted; no fake exit is inferred.
pub fn verify(
    graph: &mut Graph,
    audit: &Store,
    candidate: &RecordedCandidate,
    atom: ObjectRef,
    runtime: &DockerRuntime,
) -> std::result::Result<Verification, Error> {
    verify_bounded(
        graph,
        audit,
        candidate,
        atom,
        runtime,
        std::time::Duration::MAX,
    )
}

pub fn verify_bounded(
    graph: &mut Graph,
    audit: &Store,
    candidate: &RecordedCandidate,
    atom: ObjectRef,
    runtime: &DockerRuntime,
    remaining: std::time::Duration,
) -> std::result::Result<Verification, Error> {
    separate_store(graph, audit)?;
    let policy_ref = target(graph, candidate.reference(), atom)?.policy_ref;
    let execution =
        docker_sandbox::execute_bounded(graph, candidate, policy_ref, runtime, remaining)?;
    Ok(record(graph, audit, atom, execution)?)
}

fn separate_store(graph: &Graph, audit: &Store) -> Result<()> {
    if audit.root().starts_with(graph.cas().root()) || graph.cas().root().starts_with(audit.root())
    {
        return Err(invalid("command audit store must be separate from shared CAS").into());
    }
    Ok(())
}

fn record(
    graph: &mut Graph,
    audit: &Store,
    atom: ObjectRef,
    execution: Execution,
) -> Result<Verification> {
    separate_store(graph, audit)?;
    let target = target(graph, execution.candidate, atom)?;
    if execution.policy != target.policy_ref
        || !(0..=255).contains(&execution.exit_code)
        || (125..=127).contains(&execution.exit_code)
    {
        return Err(invalid("execution does not match command target or normal exit").into());
    }
    let private_audit = audit.put_artifact(&serde_json::to_vec(&execution)?)?;
    let verdict = target.verdict(execution.exit_code, &execution.stdout)?;
    let measurement = target.measurement.as_ref().map(|m| m.0);
    let claim = target.claim;
    let VerifierPolicy::Command {
        argv,
        cwd,
        environment,
        tool_versions,
        max_output_bytes,
        ..
    } = target.policy
    else {
        unreachable!()
    };
    if execution.stdout.len() as u64 + execution.stderr.len() as u64 > max_output_bytes {
        return Err(invalid("execution output exceeds sealed limit").into());
    }
    let stdout = graph.register_artifact(&execution.stdout)?;
    let stderr = graph.register_artifact(&execution.stderr)?;
    let run = RunRecord {
        format: "myr-command-run-v0".into(),
        candidate: execution.candidate,
        policy: target.policy_ref,
        measurement,
        exit_code: execution.exit_code,
        stdout,
        stderr,
        private_audit,
    };
    let mut dependencies = vec![run.candidate, run.policy, stdout, stderr];
    dependencies.extend(measurement);
    let record =
        graph.register_artifact_with_dependencies(&serde_json::to_vec(&run)?, &dependencies)?;
    let rationale = graph.register_artifact(
        b"Runtime sandbox command evaluated against the sealed criterion and its measurement, when present.",
    )?;
    let (claim_ref, _) = myr_wire::identify(&Object::Claim(claim.clone()))?;
    let evidence = Object::Evidence(Box::new(Evidence {
        claim: claim_ref,
        scope: claim.scope.clone(),
        verdict,
        mechanism: Mechanism::Deterministic,
        lineage: None,
        command: Some(CommandRecord {
            argv,
            cwd,
            environment: environment.into_iter().collect(),
            tool_versions: tool_versions.into_iter().collect(),
            exit_code: run.exit_code,
            stdout,
            stderr,
            sandbox: record,
        }),
        rationale,
        correlation_domains: vec![],
    }));
    let refs = graph.insert_batch(&[
        (Authority::Runtime, Object::Claim(claim)),
        (Authority::Runtime, evidence),
    ])?;
    Ok(Verification {
        record,
        claim: refs[0],
        evidence: refs[1],
    })
}

/// Check both shared provenance and the private runtime transcript. An artifact
/// with matching JSON in agent CAS alone cannot substitute for runtime execution.
pub fn load_record(graph: &Graph, audit: &Store, reference: ObjectRef) -> Result<RunRecord> {
    separate_store(graph, audit)?;
    reference.require(Kind::Artifact)?;
    if !graph.live(reference)? {
        return Err(invalid("command run record is inactive").into());
    }
    let bytes = graph.cas().get(reference)?;
    let record: RunRecord = serde_json::from_slice(&bytes)?;
    if record.format != "myr-command-run-v0" || serde_json::to_vec(&record)? != bytes {
        return Err(invalid("command run record is not canonical").into());
    }
    candidate::load_manifest(graph, record.candidate)?;
    let dependencies = graph.dependencies(reference)?;
    for dependency in [
        record.candidate,
        record.policy,
        record.stdout,
        record.stderr,
    ]
    .into_iter()
    .chain(record.measurement)
    {
        dependency.require(Kind::Artifact)?;
        if !dependencies.contains(&dependency) || !graph.live(dependency)? {
            return Err(invalid("command run provenance is absent or inactive").into());
        }
    }
    record.private_audit.require(Kind::Artifact)?;
    let raw = audit.get(record.private_audit)?;
    let execution: Execution = serde_json::from_slice(&raw)?;
    if serde_json::to_vec(&execution)? != raw
        || execution.candidate != record.candidate
        || execution.policy != record.policy
        || execution.exit_code != record.exit_code
        || myr_wire::artifact_cid(&execution.stdout) != record.stdout.cid
        || myr_wire::artifact_cid(&execution.stderr) != record.stderr.cid
    {
        return Err(invalid("command transcript differs from shared record").into());
    }
    Ok(record)
}

pub(crate) fn binds(
    graph: &Graph,
    audit: &Store,
    candidate: ObjectRef,
    evidence: &Evidence,
) -> Result<bool> {
    let Some(command) = &evidence.command else {
        return Ok(false);
    };
    let bytes = graph.cas().get(command.sandbox)?;
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(false);
    };
    if value["format"] != "myr-command-run-v0" {
        return Ok(false);
    }
    let run = load_record(graph, audit, command.sandbox)?;
    if run.candidate != candidate {
        return Ok(false);
    }
    let Object::Claim(claim) = graph.get(evidence.claim)? else {
        return Ok(false);
    };
    let target = target(graph, candidate, claim.atom)?;
    let verdict = target.verdict(run.exit_code, &graph.cas().get(run.stdout)?)?;
    if run.measurement != target.measurement.as_ref().map(|m| m.0) {
        return Ok(false);
    }
    let VerifierPolicy::Command {
        argv,
        cwd,
        environment,
        tool_versions,
        ..
    } = target.policy
    else {
        unreachable!()
    };
    let mut normalized = evidence.clone();
    normalized.command = Some(CommandRecord {
        argv,
        cwd,
        environment: environment.into_iter().collect(),
        tool_versions: tool_versions.into_iter().collect(),
        exit_code: run.exit_code,
        stdout: run.stdout,
        stderr: run.stderr,
        sandbox: command.sandbox,
    });
    let Object::Evidence(normalized) =
        myr_wire::normalize(&Object::Evidence(Box::new(normalized)))?
    else {
        unreachable!()
    };
    Ok(claim == target.claim
        && run.policy == target.policy_ref
        && evidence.scope == claim.scope
        && evidence.mechanism == Mechanism::Deterministic
        && evidence.verdict == verdict
        && evidence.command == normalized.command)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        acceptance::{ObligationStatus, assess_candidate},
        goal::{Criterion, GoalIr, SealPolicy},
        views::{Tree, import_worker},
    };
    use std::collections::BTreeMap;

    pub(crate) struct Fixture {
        pub(crate) _dir: tempfile::TempDir,
        pub(crate) graph: Graph,
        pub(crate) audit: Store,
        pub(crate) goal: ObjectRef,
        pub(crate) atom: ObjectRef,
        pub(crate) policy: ObjectRef,
    }
    impl Fixture {
        pub(crate) fn empirical(&mut self, polarity: bool) -> ObjectRef {
            let sealed = goal::load(&self.graph, self.goal).unwrap();
            let predicate = self
                .graph
                .insert(
                    Authority::Compiler,
                    &Object::PredicateDef(PredicateDef {
                        name: "mission.fixture_latency".into(),
                        version: 1,
                        arguments: vec![],
                        semantics: "Fixture measured latency is at most 16 milliseconds.".into(),
                        class: PredicateClass::Empirical,
                        provenance: Some(self.policy),
                    }),
                )
                .unwrap();
            self.atom = self
                .graph
                .insert(
                    Authority::Compiler,
                    &Object::Atom(Atom {
                        predicate,
                        arguments: vec![],
                    }),
                )
                .unwrap();
            let procedure = crate::measurement::Procedure {
                format: "myr-measurement-procedure-v0".into(),
                command: self.policy,
                target: Decimal {
                    mantissa: 16,
                    exponent: 0,
                },
                relation: crate::measurement::Relation::AtMost,
                unit: "milliseconds".into(),
            };
            let procedure_ref = self
                .graph
                .register_artifact(&procedure.encode().unwrap())
                .unwrap();
            self.goal = goal::compile(
                &mut self.graph,
                GoalIr {
                    goal: "Fixture empirical goal".into(),
                    registry: vec![predicate],
                    criteria: vec![Criterion {
                        atom: self.atom,
                        polarity,
                        binding: true,
                        measurement: Some(goal::Measurement {
                            procedure: procedure_ref,
                            tolerance: Decimal {
                                mantissa: 1,
                                exponent: 0,
                            },
                        }),
                    }],
                    assumptions: vec![],
                },
                sealed.policy().clone(),
            )
            .unwrap()
            .reference();
            procedure_ref
        }
        pub(crate) fn new(polarity: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut graph = Graph::open(
                dir.path().join("graph.sqlite"),
                Store::open(dir.path().join("shared")).unwrap(),
            )
            .unwrap();
            let audit = Store::open(dir.path().join("private")).unwrap();
            let (baseline, _) = import_worker(
                &mut graph,
                &Tree::from([("f".into(), b"baseline".to_vec())]),
            )
            .unwrap();
            let policy = VerifierPolicy::Command {
                argv: vec!["check".into()],
                cwd: ".".into(),
                environment: BTreeMap::from([
                    ("Z".into(), "line\r\n".into()),
                    ("A".into(), "e\u{301}".into()),
                ]),
                tool_versions: BTreeMap::from([("check".into(), "fixture".into())]),
                timeout_ms: 1000,
                max_output_bytes: 4096,
                sandbox: verifier::SandboxPolicy {
                    backend: verifier::SandboxBackend::DockerLinux {
                        image: format!("sha256:{}", "a".repeat(64)),
                    },
                    network: false,
                    memory_bytes: 64 * 1024 * 1024,
                    max_processes: 16,
                },
            };
            let policy = graph.register_artifact(&policy.encode().unwrap()).unwrap();
            let predicate = graph
                .insert(
                    Authority::Compiler,
                    &Object::PredicateDef(core_predicates().remove(0)),
                )
                .unwrap();
            let atom = graph
                .insert(
                    Authority::Compiler,
                    &Object::Atom(Atom {
                        predicate,
                        arguments: vec![Argument::Ref(policy)],
                    }),
                )
                .unwrap();
            let sealed = goal::compile(
                &mut graph,
                GoalIr {
                    goal: "Fixture command".into(),
                    registry: vec![predicate],
                    criteria: vec![Criterion {
                        atom,
                        polarity,
                        binding: true,
                        measurement: None,
                    }],
                    assumptions: vec![],
                },
                SealPolicy {
                    providers: serde_json::from_str(include_str!(
                        "../../../fixtures/providers-test-only.json"
                    ))
                    .unwrap(),
                    baseline,
                    protected: vec![],
                    verifier_policies: vec![policy],
                    token_budget: 1000000,
                    call_budget: 10,
                    // Generous: simulated missions share a loaded test machine.
                    time_budget_ms: 600_000,
                },
            )
            .unwrap();
            Self {
                _dir: dir,
                graph,
                audit,
                goal: sealed.reference(),
                atom,
                policy,
            }
        }
        // Synthetic execution exercises persistence only; no Docker process runs.
        pub(crate) fn execution(&self, candidate: ObjectRef, exit_code: i64) -> Execution {
            Execution {
                candidate,
                policy: self.policy,
                exit_code,
                stdout: vec![0, 255, 13, 10],
                stderr: b"fixture".to_vec(),
                image_metadata: serde_json::json!({"fixture":true}),
                daemon_metadata: serde_json::json!({"fixture":true}),
                container_metadata: serde_json::json!({"fixture":true}),
                create_argv: vec!["fixture-only".into()],
            }
        }
    }

    pub(crate) fn record_fixture(
        graph: &mut Graph,
        audit: &Store,
        atom: ObjectRef,
        execution: Execution,
    ) -> Result<Verification> {
        record(graph, audit, atom, execution)
    }

    #[test]
    fn empirical_evidence_uses_observation_tolerance_polarity_and_private_receipt() {
        for (value, polarity, supported) in [
            (17, true, true),
            (18, true, false),
            (18, false, true),
            (17, false, false),
        ] {
            let mut f = Fixture::new(true);
            let procedure = f.empirical(polarity);
            let candidate = candidate::prepare_recorded(&mut f.graph, f.goal, &[], &[]).unwrap();
            let mut execution = f.execution(candidate.reference(), 0);
            execution.stdout = serde_json::to_vec(&crate::measurement::Observation {
                format: "myr-measurement-v0".into(),
                value: Decimal {
                    mantissa: value,
                    exponent: 0,
                },
                unit: "milliseconds".into(),
            })
            .unwrap();
            let verified = record(&mut f.graph, &f.audit, f.atom, execution).unwrap();
            let run = load_record(&f.graph, &f.audit, verified.record).unwrap();
            assert_eq!(run.measurement, Some(procedure));
            assert!(
                f.graph
                    .dependencies(verified.evidence)
                    .unwrap()
                    .contains(&procedure)
            );
            let Object::Evidence(evidence) = f.graph.get(verified.evidence).unwrap() else {
                panic!()
            };
            assert_eq!(
                evidence.verdict,
                if supported {
                    Verdict::Supports
                } else {
                    Verdict::Contradicts
                }
            );
            assert!(binds(&f.graph, &f.audit, candidate.reference(), &evidence).unwrap());
            assert_eq!(
                assess_candidate(&f.graph, &f.audit, candidate.reference())
                    .unwrap()
                    .all_binding_proven(),
                supported
            );
        }
    }

    #[test]
    fn invalid_or_failed_measurements_do_not_produce_semantic_evidence() {
        for mode in 0..3 {
            let mut f = Fixture::new(true);
            f.empirical(true);
            let candidate = candidate::prepare_recorded(&mut f.graph, f.goal, &[], &[]).unwrap();
            let mut execution = f.execution(candidate.reference(), if mode == 2 { 1 } else { 0 });
            execution.stdout = if mode == 0 {
                b"not a measurement".to_vec()
            } else {
                serde_json::to_vec(&crate::measurement::Observation {
                    format: "myr-measurement-v0".into(),
                    value: Decimal {
                        mantissa: 1,
                        exponent: 0,
                    },
                    unit: if mode == 1 { "seconds" } else { "milliseconds" }.into(),
                })
                .unwrap()
            };
            assert!(record(&mut f.graph, &f.audit, f.atom, execution).is_err());
            assert!(
                !assess_candidate(&f.graph, &f.audit, candidate.reference())
                    .unwrap()
                    .all_binding_proven()
            );
            let claim = target(&f.graph, candidate.reference(), f.atom)
                .unwrap()
                .claim;
            let (reference, _) = myr_wire::identify(&Object::Claim(claim)).unwrap();
            assert!(f.graph.resolve(reference.cid).is_err());
        }
    }

    #[test]
    fn command_record_keeps_private_provenance_and_respects_criterion_polarity() {
        for (polarity, code) in [(true, 0), (true, 7), (false, 0), (false, 7)] {
            let mut f = Fixture::new(polarity);
            let candidate = candidate::prepare_recorded(&mut f.graph, f.goal, &[], &[]).unwrap();
            let execution = f.execution(candidate.reference(), code);
            let verification = record(&mut f.graph, &f.audit, f.atom, execution).unwrap();
            let run = load_record(&f.graph, &f.audit, verification.record).unwrap();
            assert_eq!(f.graph.cas().get(run.stdout).unwrap(), [0, 255, 13, 10]);
            assert!(f.graph.resolve(run.private_audit.cid).is_err());
            assert!(
                f.graph
                    .fetch(&[verification.evidence], run.private_audit)
                    .is_err()
            );
            assert!(f.graph.cas().get(run.private_audit).is_err());
            let audit = assess_candidate(&f.graph, &f.audit, candidate.reference()).unwrap();
            assert_eq!(audit.all_binding_proven(), (code == 0) == polarity);
            if (code == 0) != polarity {
                assert_eq!(audit.obligations[0].status, ObligationStatus::Refuted);
                assert_eq!(
                    audit.obligations[0].refuting_evidence,
                    vec![verification.evidence]
                );
                assert!(audit.evidence.contains(&verification.evidence));
                assert!(audit.refuted_by_deterministic_evidence());
            } else {
                assert!(audit.obligations[0].refuting_evidence.is_empty());
            }
            let Object::Evidence(evidence) = f.graph.get(verification.evidence).unwrap() else {
                unreachable!()
            };
            assert!(binds(&f.graph, &f.audit, candidate.reference(), &evidence).unwrap());
            let blank_audit = Store::open(f._dir.path().join("empty-audit")).unwrap();
            assert!(load_record(&f.graph, &blank_audit, verification.record).is_err());
        }
    }

    #[test]
    fn mixed_candidates_and_invalidated_assumptions_cannot_complete() {
        let mut f = Fixture::new(true);
        let original =
            candidate::prepare_recorded(&mut f.graph, f.goal, &[], &["f".into()]).unwrap();
        let execution = f.execution(original.reference(), 0);
        let first = record(&mut f.graph, &f.audit, f.atom, execution).unwrap();
        let manifest = candidate::load_manifest(&f.graph, original.reference()).unwrap();
        let result = f.graph.register_artifact(b"changed").unwrap();
        let mut assumption = Assumption {
            scope: manifest.scope.clone(),
            question: "Choose replacement?".into(),
            chosen: "yes".into(),
            alternatives: vec!["no".into()],
            rationale: result,
            artifacts: vec![],
            invalidates: None,
        };
        let a = f
            .graph
            .insert(Authority::Worker, &Object::Assumption(assumption.clone()))
            .unwrap();
        let delta = f
            .graph
            .insert(
                Authority::Worker,
                &Object::Delta(Delta {
                    scope: manifest.scope,
                    path: "f".into(),
                    base: manifest.files["f"],
                    patch: result,
                    result,
                    codec: DeltaCodec::Replacement,
                    assumptions: vec![a],
                }),
            )
            .unwrap();
        let changed =
            candidate::prepare_recorded(&mut f.graph, f.goal, &[delta], &["f".into()]).unwrap();
        let audit = assess_candidate(&f.graph, &f.audit, changed.reference()).unwrap();
        assert_eq!(
            audit.obligations[0].status,
            ObligationStatus::UnboundEvidence
        );
        let execution = f.execution(changed.reference(), 0);
        let second = record(&mut f.graph, &f.audit, f.atom, execution).unwrap();
        assert!(
            !assess_candidate(&f.graph, &f.audit, changed.reference())
                .unwrap()
                .all_binding_proven()
        );
        assert!(
            !assess_candidate(&f.graph, &f.audit, original.reference())
                .unwrap()
                .all_binding_proven()
        );
        assumption.invalidates = Some(a);
        f.graph
            .insert(Authority::Runtime, &Object::Assumption(assumption))
            .unwrap();
        assert!(!f.graph.live(second.evidence).unwrap());
        assert!(load_record(&f.graph, &f.audit, second.record).is_err());
        assert!(load_record(&f.graph, &f.audit, first.record).is_ok());
        assert!(f.graph.get(second.evidence).is_ok());
        assert!(
            assess_candidate(&f.graph, &f.audit, original.reference())
                .unwrap()
                .all_binding_proven()
        );
        // Contrary evidence recorded for one candidate refutes that candidate
        // only; a different candidate in the same scope is merely unproven.
        let refuting_execution = f.execution(original.reference(), 7);
        record(&mut f.graph, &f.audit, f.atom, refuting_execution).unwrap();
        let audit = assess_candidate(&f.graph, &f.audit, original.reference()).unwrap();
        assert_eq!(audit.obligations[0].status, ObligationStatus::Refuted);
        let third =
            candidate::prepare_recorded(&mut f.graph, f.goal, &[], &["f".into(), "g".into()])
                .unwrap();
        assert_ne!(third.reference(), original.reference());
        let audit = assess_candidate(&f.graph, &f.audit, third.reference()).unwrap();
        assert_eq!(audit.obligations[0].status, ObligationStatus::MissingFact);
        assert!(audit.obligations[0].refuting_evidence.is_empty());
        assert!(!audit.refuted_by_deterministic_evidence());
    }

    #[test]
    fn invalid_execution_does_not_insert_claim_or_evidence() {
        let mut f = Fixture::new(true);
        let candidate = candidate::prepare_recorded(&mut f.graph, f.goal, &[], &[]).unwrap();
        let claim = target(&f.graph, candidate.reference(), f.atom)
            .unwrap()
            .claim;
        let (claim_ref, _) = myr_wire::identify(&Object::Claim(claim)).unwrap();
        for code in [125, 126, 127, -1, 256] {
            let execution = f.execution(candidate.reference(), code);
            assert!(record(&mut f.graph, &f.audit, f.atom, execution).is_err());
            assert!(!f.graph.live(claim_ref).unwrap());
        }
        let shared = f.graph.cas().clone();
        let execution = f.execution(candidate.reference(), 0);
        assert!(record(&mut f.graph, &shared, f.atom, execution).is_err());
        assert!(!f.graph.live(claim_ref).unwrap());
    }
}
