use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};

struct Fixture {
    _dir: tempfile::TempDir,
    graph: Graph,
    scope: Scope,
    claim: ObjectRef,
    artifact: ObjectRef,
}

#[test]
fn artifact_equality_rejects_non_artifact_refs_atomically() {
    let mut f = Fixture::new();
    let predicate = f
        .graph
        .insert(
            Authority::Compiler,
            &Object::PredicateDef(core_predicates().remove(1)),
        )
        .unwrap();
    let other = f
        .graph
        .register_artifact(b"different opaque bytes\0\xff")
        .unwrap();
    let valid = Object::Atom(Atom {
        predicate,
        arguments: vec![Argument::Ref(f.artifact), Argument::Ref(other)],
    });
    let (valid_ref, _) = myr_wire::identify(&valid).unwrap();
    for non_artifact in [f.claim, f.scope.goal, predicate] {
        for position in [0, 1] {
            let mut arguments = vec![Argument::Ref(f.artifact); 2];
            arguments[position] = Argument::Ref(non_artifact);
            let invalid = Object::Atom(Atom {
                predicate,
                arguments,
            });
            let (invalid_ref, _) = myr_wire::identify(&invalid).unwrap();
            assert!(
                f.graph
                    .insert_batch(&[
                        (Authority::Worker, valid.clone()),
                        (Authority::Worker, invalid),
                    ])
                    .is_err()
            );
            assert!(f.graph.resolve(valid_ref.cid).is_err());
            assert!(f.graph.resolve(invalid_ref.cid).is_err());
        }
    }
    assert_eq!(
        f.graph.insert(Authority::Worker, &valid).unwrap(),
        valid_ref
    );
    // An ATOM merely names the proposition; unequal bytes are valid arguments.
    assert!(f.graph.live(valid_ref).unwrap());
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("objects")).unwrap();
        let mut graph = Graph::open(dir.path().join("graph.sqlite"), store).unwrap();
        let artifact = graph.register_artifact(b"source\r\n").unwrap();
        let goal = graph.register_goal(b"test-only sealed goal").unwrap();
        let scope = Scope {
            goal,
            snapshot: artifact,
        };
        let p = graph
            .insert(
                Authority::Compiler,
                &Object::PredicateDef(core_predicates().remove(0)),
            )
            .unwrap();
        let atom = graph
            .insert(
                Authority::Worker,
                &Object::Atom(Atom {
                    predicate: p,
                    arguments: vec![Argument::Ref(artifact)],
                }),
            )
            .unwrap();
        let claim = graph
            .insert(
                Authority::Worker,
                &Object::Claim(Claim {
                    atom,
                    polarity: true,
                    scope: scope.clone(),
                }),
            )
            .unwrap();
        Self {
            _dir: dir,
            graph,
            scope,
            claim,
            artifact,
        }
    }
    fn evidence(
        &self,
        claim: ObjectRef,
        verdict: Verdict,
        reviewer: Option<(&str, &str)>,
    ) -> Object {
        let lineage = reviewer.map(|(p, f)| Lineage {
            provider_id: p.into(),
            family_id: f.into(),
            checkpoint_id: "test-v1".into(),
            procedure_id: "review-v0".into(),
        });
        let command = if reviewer.is_none() {
            Some(CommandRecord {
                argv: vec!["test-fixture".into()],
                cwd: ".".into(),
                environment: vec![],
                tool_versions: vec![("fixture".into(), "1".into())],
                exit_code: if verdict == Verdict::Supports { 0 } else { 1 },
                stdout: self.artifact,
                stderr: self.artifact,
                sandbox: self.artifact,
            })
        } else {
            None
        };
        Object::Evidence(Box::new(Evidence {
            claim,
            scope: self.scope.clone(),
            verdict,
            mechanism: if reviewer.is_some() {
                Mechanism::LlmReview
            } else {
                Mechanism::Deterministic
            },
            lineage,
            command,
            rationale: self.artifact,
            correlation_domains: vec![],
        }))
    }
    fn review(&mut self, verdict: Verdict, reviewer: Option<(&str, &str)>) -> ObjectRef {
        self.graph
            .insert(
                Authority::Runtime,
                &self.evidence(self.claim, verdict, reviewer),
            )
            .unwrap()
    }
    fn confidence(&self) -> Option<u32> {
        self.graph.active_fact(self.claim).unwrap().map(|r| {
            let Object::Fact(f) = self.graph.get(r).unwrap() else {
                panic!("expected fact")
            };
            f.confidence_ppm
        })
    }
}

#[test]
fn artifact_dependency_publication_is_atomic_and_rejects_cycles() {
    let mut f = Fixture::new();
    let orphan = f.graph.cas().put_artifact(b"unregistered").unwrap();
    let bytes = b"runtime manifest";
    let reference = f.graph.cas().put_artifact(bytes).unwrap();
    assert!(
        f.graph
            .register_artifact_with_dependencies(bytes, &[f.artifact, orphan])
            .is_err()
    );
    assert!(!f.graph.live(reference).unwrap());
    assert!(
        f.graph
            .register_artifact_with_dependencies(bytes, &[reference])
            .is_err()
    );
    assert!(!f.graph.live(reference).unwrap());
    assert_eq!(
        f.graph
            .register_artifact_with_dependencies(bytes, &[f.artifact])
            .unwrap(),
        reference
    );
    assert!(
        f.graph
            .dependencies(reference)
            .unwrap()
            .contains(&f.artifact)
    );
    let artifact_bytes = f.graph.cas().get(f.artifact).unwrap();
    assert!(
        f.graph
            .register_artifact_with_dependencies(&artifact_bytes, &[reference])
            .is_err()
    );
    assert!(
        !f.graph
            .dependencies(f.artifact)
            .unwrap()
            .contains(&reference)
    );
}

#[test]
fn promotion_retraction_and_immutable_history_survive_reopen() {
    let mut f = Fixture::new();
    f.review(Verdict::Supports, Some(("a", "alpha")));
    assert_eq!(f.confidence(), None);
    f.review(Verdict::Supports, Some(("b", "beta")));
    assert_eq!(f.confidence(), Some(800000));
    let old = f.graph.active_fact(f.claim).unwrap().unwrap();
    f.review(Verdict::Contradicts, Some(("c", "gamma")));
    assert_eq!(f.confidence(), None);
    assert!(!f.graph.live(old).unwrap());
    assert!(matches!(f.graph.get(old).unwrap(), Object::Fact(_)));
    f.review(Verdict::Supports, None);
    assert_eq!(f.confidence(), Some(900000));
    f.review(Verdict::Contradicts, None);
    assert_eq!(f.confidence(), None);
    let cas = f.graph.cas().clone();
    drop(f.graph);
    let reopened = Graph::open(f._dir.path().join("graph.sqlite"), cas).unwrap();
    assert!(reopened.active_fact(f.claim).unwrap().is_none());
    assert!(!reopened.live(old).unwrap());
}

#[test]
fn opposite_claims_block_llm_and_deterministic_support_blocks_other_polarity() {
    let mut f = Fixture::new();
    f.review(Verdict::Supports, Some(("a", "alpha")));
    f.review(Verdict::Supports, Some(("b", "beta")));
    let Object::Claim(mut opposite) = f.graph.get(f.claim).unwrap() else {
        panic!()
    };
    opposite.polarity = false;
    let negative = f
        .graph
        .insert(Authority::Worker, &Object::Claim(opposite))
        .unwrap();
    assert!(f.graph.unresolved_conflict(f.claim).unwrap());
    assert_eq!(f.confidence(), None);
    f.review(Verdict::Supports, None);
    assert_eq!(f.confidence(), Some(970000));
    assert!(!f.graph.unresolved_conflict(f.claim).unwrap());
    assert!(f.graph.active_fact(negative).unwrap().is_none());
    let contrary = f.evidence(negative, Verdict::Supports, None);
    f.graph.insert(Authority::Runtime, &contrary).unwrap();
    assert_eq!(f.confidence(), None);
    assert!(f.graph.unresolved_conflict(f.claim).unwrap());
}

#[test]
fn assumptions_invalidate_facts_deltas_and_transitive_artifacts() {
    let mut f = Fixture::new();
    let assumption = Assumption {
        scope: f.scope.clone(),
        question: "Preserve quirks?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale: f.artifact,
        artifacts: vec![f.artifact],
        invalidates: None,
    };
    let a = f
        .graph
        .insert(Authority::Worker, &Object::Assumption(assumption.clone()))
        .unwrap();
    f.graph.depend(f.claim, a).unwrap();
    f.review(Verdict::Supports, None);
    let fact = f.graph.active_fact(f.claim).unwrap().unwrap();
    let Object::Fact(value) = f.graph.get(fact).unwrap() else {
        panic!()
    };
    assert_eq!(value.assumptions, vec![a]);
    let output = f.graph.register_artifact(b"produced artifact").unwrap();
    let delta = f
        .graph
        .insert(
            Authority::Worker,
            &Object::Delta(Delta {
                scope: f.scope.clone(),
                path: "src/lib.rs".into(),
                base: f.artifact,
                patch: output,
                result: output,
                codec: DeltaCodec::Replacement,
                assumptions: vec![a],
            }),
        )
        .unwrap();
    f.graph.depend(output, fact).unwrap();
    let mut invalidation = assumption;
    invalidation.invalidates = Some(a);
    assert!(
        f.graph
            .insert(Authority::Worker, &Object::Assumption(invalidation.clone()))
            .is_err()
    );
    let event = f
        .graph
        .insert(Authority::Runtime, &Object::Assumption(invalidation))
        .unwrap();
    assert!(f.graph.live(event).unwrap());
    for r in [a, f.claim, fact, output, delta] {
        assert!(!f.graph.live(r).unwrap(), "{r:?}");
    }
    assert_eq!(f.confidence(), None);
    assert!(f.graph.get(fact).is_ok());
}

#[test]
fn new_evidence_retires_fact_even_when_confidence_stays_equal() {
    let mut f = Fixture::new();
    f.review(Verdict::Supports, None);
    let old = f.graph.active_fact(f.claim).unwrap().unwrap();
    let produced = f.graph.register_artifact(b"downstream").unwrap();
    f.graph.depend(produced, old).unwrap();
    f.review(Verdict::Supports, Some(("a", "alpha")));
    let new = f.graph.active_fact(f.claim).unwrap().unwrap();
    assert_ne!(old, new);
    assert_eq!(f.confidence(), Some(950000));
    assert!(!f.graph.live(old).unwrap());
    assert!(!f.graph.live(produced).unwrap());
}

#[test]
fn graph_enforces_authority_types_scope_references_and_fetch_closure() {
    let mut f = Fixture::new();
    let evidence = f.evidence(f.claim, Verdict::Supports, None);
    assert!(f.graph.insert(Authority::Worker, &evidence).is_err());
    let mut wrong = evidence;
    if let Object::Evidence(e) = &mut wrong {
        e.scope.snapshot = f.graph.register_artifact(b"other snapshot").unwrap();
    }
    assert!(f.graph.insert(Authority::Runtime, &wrong).is_err());
    let Object::Claim(claim) = f.graph.get(f.claim).unwrap() else {
        panic!()
    };
    let Object::Atom(mut atom) = f.graph.get(claim.atom).unwrap() else {
        panic!()
    };
    atom.arguments = vec![Argument::Boolean(true)];
    assert!(
        f.graph
            .insert(Authority::Worker, &Object::Atom(atom))
            .is_err()
    );
    let secret = f.graph.register_artifact(b"not in task closure").unwrap();
    assert!(f.graph.fetch(&[f.claim], secret).is_err());
    assert_eq!(
        f.graph.fetch(&[f.claim], f.artifact).unwrap(),
        b"source\r\n"
    );
    assert!(f.graph.depend(f.artifact, f.claim).is_err());
    let fake = ObjectRef::new(Kind::Atom, Cid([99; 32]));
    assert!(
        f.graph
            .insert(
                Authority::Worker,
                &Object::Claim(Claim {
                    atom: fake,
                    ..claim
                })
            )
            .is_err()
    );
}
