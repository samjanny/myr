use base64::{Engine, engine::general_purpose::STANDARD};
use myr_adapter::{Action, Identity, Outcome, Response, Role, Session, SessionConfig};
use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};
use std::collections::BTreeMap;

struct Fixture {
    dir: tempfile::TempDir,
    graph: Graph,
    task: ObjectRef,
    predicate: ObjectRef,
    base: ObjectRef,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut graph = Graph::open(
            dir.path().join("graph.sqlite"),
            Store::open(dir.path().join("objects")).unwrap(),
        )
        .unwrap();
        let base = graph.register_artifact(b"original\r\n").unwrap();
        let goal = graph.register_goal(b"fixture sealed goal").unwrap();
        let predicate = graph
            .insert(
                Authority::Compiler,
                &Object::PredicateDef(core_predicates().remove(0)),
            )
            .unwrap();
        let task = graph
            .insert(
                Authority::Runtime,
                &Object::Task(Task {
                    scope: Scope {
                        goal,
                        snapshot: base,
                    },
                    inputs: vec![base, predicate],
                    capabilities: vec!["write:src/".into(), "write:tests/".into()],
                    assumptions: vec![],
                    obligations: vec![],
                    instruction: base,
                    token_budget: 2000,
                    call_budget: 10,
                    time_budget_ms: 30000,
                }),
            )
            .unwrap();
        Self {
            dir,
            graph,
            task,
            predicate,
            base,
        }
    }
    fn session(&self, role: Role, provider: &str) -> Session {
        Session::new(
            &self.graph,
            SessionConfig {
                task: self.task,
                role,
                identity: Identity {
                    agent: provider.into(),
                    lineage: Lineage {
                        provider_id: provider.into(),
                        family_id: provider.into(),
                        checkpoint_id: "fixture-v0".into(),
                        procedure_id: "boundary-test".into(),
                    },
                },
                baseline: BTreeMap::from([
                    ("src/lib.rs".into(), self.base),
                    ("tests/test.rs".into(), self.base),
                ]),
                protected: vec!["tests/".into()],
                quarantine: self.dir.path().join("quarantine"),
            },
        )
        .unwrap()
    }
    fn claim_action(&self) -> Action {
        Action::EmitClaim {
            predicate_ref: self.predicate,
            arguments: vec![Argument::Ref(self.base)],
            polarity: true,
        }
    }
}
fn handle(session: &mut Session, graph: &mut Graph, action: Action) -> Outcome {
    session
        .handle(graph, &serde_json::to_vec(&Response { action }).unwrap())
        .unwrap()
}
fn produced(outcome: Outcome) -> Vec<ObjectRef> {
    let Outcome::Applied { objects, .. } = outcome else {
        panic!("expected applied")
    };
    objects
}

#[test]
fn distinct_provider_identities_produce_same_claim_and_different_attestations() {
    let mut f = Fixture::new();
    let mut a = f.session(Role::Worker, "openai");
    let mut b = f.session(Role::Worker, "anthropic");
    let action = f.claim_action();
    let ar = produced(handle(&mut a, &mut f.graph, action.clone()));
    let br = produced(handle(&mut b, &mut f.graph, action));
    assert_eq!(ar[0], br[0]);
    assert_eq!(ar[1], br[1]);
    assert_ne!(ar[2], br[2]);
    let Object::Attest(attest) = f.graph.get(ar[2]).unwrap() else {
        panic!()
    };
    assert_eq!(attest.agent, "openai");
    assert_eq!(attest.lineage.provider_id, "openai");
    assert!(f.graph.active_fact(ar[1]).unwrap().is_none());
}

#[test]
fn three_invalid_outputs_are_quarantined_then_runtime_emits_fail() {
    let mut f = Fixture::new();
    let mut session = f.session(Role::Worker, "fixture");
    let raw = br#"{"action":{"tool":"emit_evidence","arguments":{"confidence":1000000}}}"#;
    for attempt in 1..=2 {
        assert!(
            matches!(session.handle(&mut f.graph,raw).unwrap(),Outcome::Repair {attempt:a,..} if a==attempt)
        );
    }
    let Outcome::Failed { failure } = session.handle(&mut f.graph, raw).unwrap() else {
        panic!()
    };
    let Object::Fail(fail) = f.graph.get(failure).unwrap() else {
        panic!()
    };
    assert_eq!(fail.code, FailCode::InvalidAgentOutput);
    assert_eq!(fail.class, FailClass::Validation);
    let path = f
        .dir
        .path()
        .join("quarantine")
        .join(hex::encode(myr_wire::artifact_cid(raw).0));
    assert_eq!(std::fs::read(path).unwrap(), raw);
    assert!(session.handle(&mut f.graph, raw).is_err());
}

#[test]
fn semantic_failure_rolls_back_entire_atom_claim_attestation_batch() {
    let mut f = Fixture::new();
    let mut s = f.session(Role::Worker, "fixture");
    let atom = Object::Atom(Atom {
        predicate: f.predicate,
        arguments: vec![Argument::Boolean(true)],
    });
    let r = myr_wire::identify(&atom).unwrap().0;
    let invalid = Action::EmitClaim {
        predicate_ref: f.predicate,
        arguments: vec![Argument::Boolean(true)],
        polarity: true,
    };
    assert!(matches!(
        handle(&mut s, &mut f.graph, invalid),
        Outcome::Repair { attempt: 1, .. }
    ));
    assert!(f.graph.get(r).is_err());
    let valid = f.claim_action();
    assert_eq!(produced(handle(&mut s, &mut f.graph, valid)).len(), 3);
    assert!(matches!(
        s.handle(&mut f.graph, b"not json").unwrap(),
        Outcome::Repair { attempt: 1, .. }
    ));
}

#[test]
fn failed_later_member_does_not_leave_earlier_objects_in_graph() {
    let mut f = Fixture::new();
    let atom = Object::Atom(Atom {
        predicate: f.predicate,
        arguments: vec![Argument::Ref(f.base)],
    });
    let ar = myr_wire::identify(&atom).unwrap().0;
    let missing = ObjectRef::new(Kind::Claim, Cid([99; 32]));
    let attest = Object::Attest(Attest {
        claim: missing,
        task: f.task,
        agent: "runtime".into(),
        lineage: Lineage {
            provider_id: "x".into(),
            family_id: "x".into(),
            checkpoint_id: "1".into(),
            procedure_id: "1".into(),
        },
    });
    assert!(
        f.graph
            .insert_batch(&[(Authority::Worker, atom), (Authority::Runtime, attest)])
            .is_err()
    );
    assert!(f.graph.get(ar).is_err());
}

#[test]
fn protected_paths_fail_immediately_even_with_write_capability() {
    let mut f = Fixture::new();
    let mut s = f.session(Role::Worker, "fixture");
    let base = f.base;
    let outcome = handle(
        &mut s,
        &mut f.graph,
        Action::EmitDelta {
            path: "tests/test.rs".into(),
            base_ref: base,
            patch_ref: base,
            codec: DeltaCodec::Replacement,
            result_ref: base,
            assumptions: vec![],
        },
    );
    let Outcome::Failed { failure } = outcome else {
        panic!()
    };
    let Object::Fail(fail) = f.graph.get(failure).unwrap() else {
        panic!()
    };
    assert_eq!(fail.code, FailCode::CapabilityDenied);
}

#[test]
fn reference_access_is_task_local_and_opaque_bytes_are_preserved() {
    let mut f = Fixture::new();
    let mut s = f.session(Role::Worker, "fixture");
    let secret = f.graph.register_artifact(b"source-view-secret").unwrap();
    assert!(matches!(
        handle(&mut s, &mut f.graph, Action::Fetch { reference: secret }),
        Outcome::Repair { .. }
    ));
    let bytes = b"\0\xff\r\n";
    let objects = produced(handle(
        &mut s,
        &mut f.graph,
        Action::PutArtifact {
            content_base64: STANDARD.encode(bytes),
        },
    ));
    assert_eq!(f.graph.cas().get(objects[0]).unwrap(), bytes);
    let fetched = handle(
        &mut s,
        &mut f.graph,
        Action::Fetch {
            reference: objects[0],
        },
    );
    assert!(
        serde_json::to_value(&fetched)
            .unwrap()
            .get("fetched_raw_bytes")
            .is_none()
    );
    let Outcome::Applied {
        result,
        fetched_raw_bytes,
        ..
    } = fetched
    else {
        panic!()
    };
    assert_eq!(fetched_raw_bytes, Some(bytes.len() as u64));
    assert_eq!(result["content_base64"], STANDARD.encode(bytes));
    let base = f.base;
    let delta = produced(handle(
        &mut s,
        &mut f.graph,
        Action::EmitDelta {
            path: "src/lib.rs".into(),
            base_ref: base,
            patch_ref: objects[0],
            codec: DeltaCodec::Replacement,
            result_ref: objects[0],
            assumptions: vec![],
        },
    ));
    assert_eq!(delta[0].kind, Kind::Delta);
}

#[test]
fn reviewer_cannot_emit_claim_and_worker_cannot_review() {
    let mut f = Fixture::new();
    let mut reviewer = f.session(Role::Reviewer, "fixture");
    let action = f.claim_action();
    assert!(matches!(
        handle(&mut reviewer, &mut f.graph, action),
        Outcome::Failed { .. }
    ));
    let mut worker = f.session(Role::Worker, "fixture");
    let base = f.base;
    assert!(matches!(
        handle(
            &mut worker,
            &mut f.graph,
            Action::ReviewClaim {
                claim_ref: ObjectRef::new(Kind::Claim, Cid([0; 32])),
                verdict: Verdict::Supports,
                rationale_ref: base
            }
        ),
        Outcome::Failed { .. }
    ));
}

#[test]
fn unknown_fields_and_untrusted_lineage_are_rejected_by_schema() {
    let mut f = Fixture::new();
    let mut s = f.session(Role::Worker, "fixture");
    let mut value = serde_json::to_value(Response {
        action: f.claim_action(),
    })
    .unwrap();
    value["action"]["arguments"]["lineage"] = serde_json::json!({"provider_id":"forged"});
    assert!(matches!(
        s.handle(&mut f.graph, &serde_json::to_vec(&value).unwrap())
            .unwrap(),
        Outcome::Repair { .. }
    ));
    let worker_schema = myr_adapter::schema::response_schema(Role::Worker).to_string();
    let reviewer_schema = myr_adapter::schema::response_schema(Role::Reviewer).to_string();
    assert!(worker_schema.contains("emit_claim"));
    assert!(!worker_schema.contains("review_claim"));
    assert!(reviewer_schema.contains("review_claim"));
    assert!(!reviewer_schema.contains("emit_claim"));
    for schema in [&worker_schema, &reviewer_schema] {
        assert!(!schema.contains("emit_evidence"));
        assert!(!schema.contains("provider_id"));
    }
}

#[test]
fn unified_diff_keeps_separate_patch_and_result_and_requires_patch_access() {
    let mut f = Fixture::new();
    let mut session = f.session(Role::Worker, "fixture");
    let secret_patch = f.graph.register_artifact(b"private patch").unwrap();
    let base = f.base;
    let action = Action::EmitDelta {
        path: "src/lib.rs".into(),
        base_ref: base,
        patch_ref: secret_patch,
        result_ref: base,
        codec: DeltaCodec::UnifiedDiff,
        assumptions: vec![],
    };
    assert!(matches!(
        handle(&mut session, &mut f.graph, action),
        Outcome::Repair { .. }
    ));
    let patch = produced(handle(
        &mut session,
        &mut f.graph,
        Action::PutArtifact {
            content_base64: STANDARD.encode(
                b"--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-original\r\n+changed\n",
            ),
        },
    ))[0];
    let result = produced(handle(
        &mut session,
        &mut f.graph,
        Action::PutArtifact {
            content_base64: STANDARD.encode(b"changed\n"),
        },
    ))[0];
    let action = Action::EmitDelta {
        path: "src/lib.rs".into(),
        base_ref: base,
        patch_ref: patch,
        result_ref: result,
        codec: DeltaCodec::UnifiedDiff,
        assumptions: vec![],
    };
    let delta = produced(handle(&mut session, &mut f.graph, action))[0];
    let Object::Delta(d) = f.graph.get(delta).unwrap() else {
        panic!()
    };
    assert_eq!(d.codec, DeltaCodec::UnifiedDiff);
    assert_eq!(d.patch, patch);
    assert_eq!(d.result, result);
    let replacement = Action::EmitDelta {
        path: "src/lib.rs".into(),
        base_ref: base,
        patch_ref: patch,
        result_ref: result,
        codec: DeltaCodec::Replacement,
        assumptions: vec![],
    };
    assert!(matches!(
        handle(&mut session, &mut f.graph, replacement),
        Outcome::Repair { .. }
    ));
}

#[test]
fn reviewer_output_is_wrapped_by_runtime_and_two_lineages_promote() {
    let mut f = Fixture::new();
    let mut worker = f.session(Role::Worker, "worker");
    let action = f.claim_action();
    let claim = produced(handle(&mut worker, &mut f.graph, action))[1];
    let Object::Task(mut review_task) = f.graph.get(f.task).unwrap() else {
        panic!()
    };
    review_task.inputs.push(claim);
    f.task = f
        .graph
        .insert(Authority::Runtime, &Object::Task(review_task))
        .unwrap();
    let mut a = f.session(Role::Reviewer, "reviewer-a");
    let mut b = f.session(Role::Reviewer, "reviewer-b");
    let review = Action::ReviewClaim {
        claim_ref: claim,
        verdict: Verdict::Supports,
        rationale_ref: f.base,
    };
    let evidence_a = produced(handle(&mut a, &mut f.graph, review.clone()))[0];
    assert!(f.graph.active_fact(claim).unwrap().is_none());
    let evidence_b = produced(handle(&mut b, &mut f.graph, review))[0];
    assert_ne!(evidence_a, evidence_b);
    let Object::Evidence(e) = f.graph.get(evidence_a).unwrap() else {
        panic!()
    };
    assert_eq!(e.mechanism, Mechanism::LlmReview);
    assert_eq!(e.lineage.unwrap().provider_id, "reviewer-a");
    assert!(e.command.is_none());
    let fact = f.graph.active_fact(claim).unwrap().unwrap();
    let Object::Fact(fact) = f.graph.get(fact).unwrap() else {
        panic!()
    };
    assert_eq!(fact.confidence_ppm, 800000);
}
