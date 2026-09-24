use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};
use myr_runner::{
    goal::*,
    views::{Tree, import_worker},
};

fn policy_bytes(version: &str) -> Vec<u8> {
    use myr_runner::verifier::*;
    VerifierPolicy::Command {
        argv: vec!["cargo".into(), "test".into()],
        cwd: ".".into(),
        environment: std::collections::BTreeMap::new(),
        tool_versions: std::collections::BTreeMap::from([("cargo".into(), version.into())]),
        timeout_ms: 500,
        max_output_bytes: 4096,
        sandbox: SandboxPolicy {
            backend: SandboxBackend::WindowsAppContainer,
            network: false,
            memory_bytes: 1024 * 1024 * 1024,
            max_processes: 32,
        },
    }
    .encode()
    .unwrap()
}

struct Fixture {
    _dir: tempfile::TempDir,
    graph: Graph,
    ir: GoalIr,
    policy: SealPolicy,
    artifact: ObjectRef,
}

#[test]
fn planner_can_construct_registered_atoms_and_pending_assumption_rationale() {
    use myr_runner::{
        mission,
        planner::{Catalog, Step},
    };
    let mut f = Fixture::new();
    let pending = f.predicate("mission.planner_choice", PredicateClass::Unresolved);
    let Object::Atom(pending_atom) = f.graph.get(pending).unwrap() else {
        unreachable!()
    };
    let Object::Atom(command_atom) = f.graph.get(f.ir.criteria[0].atom).unwrap() else {
        unreachable!()
    };
    let mission = mission::parse(b"goal: Preserve behavior\nverify: [cargo test]\n").unwrap();
    let mut catalog = Catalog::new(&f.graph, &f.ir.registry, &[], &[f.artifact]).unwrap();
    let act = |name: &str, args: serde_json::Value| {
        serde_json::to_vec(&serde_json::json!({"action":{"tool":name,"arguments":args}})).unwrap()
    };
    let raw = act(
        "define_atom",
        serde_json::json!({"predicate_ref":command_atom.predicate,"arguments":command_atom.arguments}),
    );
    let Step::Created(command) = catalog
        .handle_response(&mut f.graph, &mission, &f.policy, &raw)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(command, f.ir.criteria[0].atom);
    let raw = act(
        "define_atom",
        serde_json::json!({"predicate_ref":pending_atom.predicate,"arguments":[]}),
    );
    let Step::Created(choice) = catalog
        .handle_response(&mut f.graph, &mission, &f.policy, &raw)
        .unwrap()
    else {
        panic!()
    };
    let text = "Explicit rationale\r\nwith opaque UTF-8: é";
    let raw = act("put_rationale", serde_json::json!({"text":text}));
    let Step::Created(rationale) = catalog
        .handle_response(&mut f.graph, &mission, &f.policy, &raw)
        .unwrap()
    else {
        panic!()
    };
    let raw = act("fetch", serde_json::json!({"reference":rationale}));
    let Step::Fetched { reference, bytes } = catalog
        .handle_response(&mut f.graph, &mission, &f.policy, &raw)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(reference, rationale);
    assert_eq!(bytes, text.as_bytes());
    let mut ir = f.ir.clone();
    ir.assumptions.push(PendingAssumption {
        atom: choice,
        question: "Preserve quirks?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale,
        artifacts: vec![f.artifact],
    });
    let raw = act("submit_goal", serde_json::to_value(ir).unwrap());
    let Step::Sealed(sealed) = catalog
        .handle_response(&mut f.graph, &mission, &f.policy, &raw)
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(sealed.ir().assumptions[0].rationale, rationale);
    assert_eq!(sealed.ir().criteria[0].atom, command);
    assert!(
        catalog
            .response_schema(&mission)
            .to_string()
            .contains(&rationale.cid.to_string())
    );
}

#[test]
fn planner_atom_tools_reject_wrong_types_and_inaccessible_references() {
    use myr_runner::{mission, planner::Catalog};
    let mut f = Fixture::new();
    let mission = mission::parse(b"goal: Preserve behavior\nverify: [cargo test]\n").unwrap();
    let mut catalog = Catalog::new(&f.graph, &f.ir.registry, &[], &[f.artifact]).unwrap();
    let predicate = f.ir.registry[0];
    let wrong = Atom {
        predicate,
        arguments: vec![Argument::Text("not a reference".into())],
    };
    let (reference, _) = myr_wire::identify(&Object::Atom(wrong.clone())).unwrap();
    assert!(
        catalog
            .define_atom(&mut f.graph, predicate, wrong.arguments)
            .is_err()
    );
    assert!(!f.graph.live(reference).unwrap());
    let outside = f.graph.register_artifact(b"outside catalog").unwrap();
    assert!(
        catalog
            .define_atom(&mut f.graph, predicate, vec![Argument::Ref(outside)])
            .is_err()
    );
    let raw = serde_json::to_vec(
        &serde_json::json!({"action":{"tool":"fetch","arguments":{"reference":outside}}}),
    )
    .unwrap();
    assert!(
        catalog
            .handle_response(&mut f.graph, &mission, &f.policy, &raw)
            .is_err()
    );
    for text in [
        String::new(),
        "x".repeat(myr_adapter::MAX_ARTIFACT_BYTES + 1),
    ] {
        let raw = serde_json::to_vec(
            &serde_json::json!({"action":{"tool":"put_rationale","arguments":{"text":text}}}),
        )
        .unwrap();
        assert!(
            catalog
                .handle_response(&mut f.graph, &mission, &f.policy, &raw)
                .is_err()
        );
    }
}

#[test]
fn planner_contract_preserves_user_mission_and_runtime_owned_policy() {
    use myr_runner::{mission, planner::Catalog};
    let mut f = Fixture::new();
    let mission =
        mission::parse(b"goal: Preserve behavior\nverify: [cargo test]\nprotected: [contract/]\n")
            .unwrap();
    let catalog = Catalog::new(
        &f.graph,
        &f.ir.registry,
        &[f.ir.criteria[0].atom],
        &[f.artifact],
    )
    .unwrap();
    let response = serde_json::json!({"action":{"tool":"submit_goal","arguments":f.ir}});
    let schema = catalog.response_schema(&mission);
    let parts = myr_adapter::schema::accounting_parts(
        &schema,
        &myr_adapter::schema::response_schema(myr_adapter::Role::Planner),
    )
    .unwrap();
    assert_eq!(
        parts.iter().map(|p| p.text.as_str()).collect::<String>(),
        schema.to_string()
    );
    assert!(
        parts
            .iter()
            .any(|p| p.protocol_only && p.text.contains("submit_goal"))
    );
    let sealed = catalog
        .compile_response(
            &mut f.graph,
            &mission,
            f.policy.clone(),
            &serde_json::to_vec(&response).unwrap(),
        )
        .unwrap();
    assert_eq!(sealed.ir().goal, mission.goal);
    assert_eq!(sealed.policy().providers.worker, f.policy.providers.worker);
    assert_eq!(sealed.policy().baseline, f.policy.baseline);
    assert!(sealed.policy().protected.contains(&"contract".into()));
    assert_eq!(sealed.policy().token_budget, f.policy.token_budget);
    let preserved = f.graph.cas().get(sealed.reference()).unwrap();
    for variant in 0..7 {
        let mut bad = response.clone();
        let ir = &mut bad["action"]["arguments"];
        match variant {
            0 => ir["goal"] = "Replace the user goal".into(),
            1 => ir["criteria"][0]["binding"] = false.into(),
            2 => ir["criteria"][0]["polarity"] = false.into(),
            3 => ir["providers"] = serde_json::json!({"worker":"paid-api"}),
            4 => {
                ir["criteria"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("measurement");
            }
            5 => ir["criteria"] = serde_json::json!([]),
            6 => bad["action"]["tool"] = "finish".into(),
            _ => unreachable!(),
        }
        assert!(
            catalog
                .compile_response(
                    &mut f.graph,
                    &mission,
                    f.policy.clone(),
                    &serde_json::to_vec(&bad).unwrap()
                )
                .is_err()
        );
    }
    assert_eq!(f.graph.cas().get(sealed.reference()).unwrap(), preserved);
}

#[test]
fn planner_catalog_restricts_even_live_objects_and_rejects_duplicate_fields() {
    use myr_runner::{mission, planner::Catalog};
    let mut f = Fixture::new();
    let mission = mission::parse(b"goal: Preserve behavior\nverify: [cargo test]\n").unwrap();
    let catalog = Catalog::new(
        &f.graph,
        &f.ir.registry,
        &[f.ir.criteria[0].atom],
        &[f.artifact],
    )
    .unwrap();
    let outside = f.predicate("mission.outside_catalog", PredicateClass::Decidable);
    let mut ir = f.ir.clone();
    ir.criteria.push(Criterion {
        atom: outside,
        polarity: true,
        binding: true,
        measurement: None,
    });
    let raw =
        serde_json::to_vec(&serde_json::json!({"action":{"tool":"submit_goal","arguments":ir}}))
            .unwrap();
    assert!(
        catalog
            .compile_response(&mut f.graph, &mission, f.policy.clone(), &raw)
            .is_err()
    );
    let raw = br#"{"action":{"tool":"submit_goal","tool":"submit_goal","arguments":{}}}"#;
    assert!(
        catalog
            .compile_response(&mut f.graph, &mission, f.policy.clone(), raw)
            .is_err()
    );
    assert!(
        catalog
            .compile_response(
                &mut f.graph,
                &mission,
                f.policy.clone(),
                &vec![b' '; 1024 * 1024 + 1]
            )
            .is_err()
    );
    assert!(Catalog::new(&f.graph, &f.ir.registry, &[outside, outside], &[]).is_err());
}

#[test]
fn docker_execution_rejects_unsealed_policy_and_changed_candidate_before_launch() {
    use myr_runner::{
        candidate::prepare_recorded,
        docker_sandbox::{self, DockerRuntime, Error},
        verifier::{SandboxBackend, VerifierPolicy},
    };
    let mut f = Fixture::new();
    let mut policy: VerifierPolicy = serde_json::from_slice(&policy_bytes("fixture")).unwrap();
    let VerifierPolicy::Command { sandbox, .. } = &mut policy else {
        unreachable!()
    };
    sandbox.backend = SandboxBackend::DockerLinux {
        image: format!("sha256:{}", "a".repeat(64)),
    };
    let docker_policy = f
        .graph
        .register_artifact(&policy.encode().unwrap())
        .unwrap();
    f.policy.verifier_policies.push(docker_policy);
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let candidate = prepare_recorded(&mut f.graph, sealed.reference(), &[], &[]).unwrap();
    let runtime = DockerRuntime {
        executable: f._dir.path().join("must-not-be-launched.exe"),
    };
    let unsealed = f.graph.register_artifact(b"unsealed policy").unwrap();
    assert!(
        matches!(docker_sandbox::execute(&f.graph, &candidate, unsealed, &runtime),
        Err(Error::Rejected(message)) if message == "verifier policy is not sealed")
    );
    assert!(matches!(
        docker_sandbox::execute_bounded(
            &f.graph,
            &candidate,
            docker_policy,
            &runtime,
            std::time::Duration::ZERO
        ),
        Err(Error::Process(myr_adapter::process::Error::Timeout))
    ));
    std::fs::write(
        candidate.files().path().join("src/lib.rs"),
        b"changed outside runtime",
    )
    .unwrap();
    assert!(
        matches!(docker_sandbox::execute(&f.graph, &candidate, docker_policy, &runtime),
        Err(Error::Rejected(message)) if message == "candidate files changed after preparation")
    );
}

#[test]
fn candidate_manifest_commits_reconstruction_and_stales_with_its_assumptions() {
    use myr_runner::candidate::{load_manifest, prepare_recorded};
    let mut f = Fixture::new();
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let first = prepare_recorded(&mut f.graph, sealed.reference(), &[], &["src/".into()]).unwrap();
    let again = prepare_recorded(&mut f.graph, sealed.reference(), &[], &["src".into()]).unwrap();
    assert_eq!(first.reference(), again.reference());
    assert_ne!(first.files().path(), again.files().path());
    let manifest = load_manifest(&f.graph, first.reference()).unwrap();
    let base = manifest.files["src/lib.rs"];
    let result = f.graph.register_artifact(b"replacement").unwrap();
    let mut a = Assumption {
        scope: sealed.scope(),
        question: "Choose replacement?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale: f.artifact,
        artifacts: vec![],
        invalidates: None,
    };
    let assumption = f
        .graph
        .insert(Authority::Worker, &Object::Assumption(a.clone()))
        .unwrap();
    let delta = f
        .graph
        .insert(
            Authority::Worker,
            &Object::Delta(Delta {
                scope: sealed.scope(),
                path: "src/lib.rs".into(),
                base,
                patch: result,
                result,
                codec: DeltaCodec::Replacement,
                assumptions: vec![assumption],
            }),
        )
        .unwrap();
    let changed =
        prepare_recorded(&mut f.graph, sealed.reference(), &[delta], &["src".into()]).unwrap();
    assert_ne!(first.reference(), changed.reference());
    let changed_manifest = load_manifest(&f.graph, changed.reference()).unwrap();
    assert_eq!(changed_manifest.files["src/lib.rs"], result);
    assert_eq!(
        std::fs::read(changed.files().path().join("src/lib.rs")).unwrap(),
        b"replacement"
    );
    let mut forged = changed_manifest;
    forged.files.insert("src/lib.rs".into(), base);
    let forged_ref = f
        .graph
        .register_artifact(&serde_json::to_vec(&forged).unwrap())
        .unwrap();
    assert!(load_manifest(&f.graph, forged_ref).is_err());
    assert!(
        prepare_recorded(
            &mut f.graph,
            sealed.reference(),
            &[delta, delta],
            &["src".into()]
        )
        .is_err()
    );
    a.invalidates = Some(assumption);
    f.graph
        .insert(Authority::Runtime, &Object::Assumption(a))
        .unwrap();
    assert!(!f.graph.live(changed.reference()).unwrap());
    assert!(load_manifest(&f.graph, changed.reference()).is_err());
    assert!(load_manifest(&f.graph, first.reference()).is_ok());
    assert!(f.graph.cas().get(changed.reference()).is_ok());
}

#[test]
fn acceptance_requires_exact_claim_and_live_fact_dependencies() {
    use myr_runner::acceptance::{ObligationStatus, assess};
    let mut f = Fixture::new();
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let initial = assess(&f.graph, sealed.reference()).unwrap();
    assert!(!initial.all_binding_proven());
    assert_eq!(initial.obligations[0].status, ObligationStatus::MissingFact);
    assert!(
        !f.graph.live(initial.obligations[0].claim).unwrap(),
        "audit must not create claims"
    );
    let mut claim = Claim {
        atom: f.ir.criteria[0].atom,
        polarity: false,
        scope: sealed.scope(),
    };
    f.graph
        .insert(Authority::Worker, &Object::Claim(claim.clone()))
        .unwrap();
    assert!(
        !assess(&f.graph, sealed.reference())
            .unwrap()
            .all_binding_proven()
    );
    claim.polarity = true;
    let claim_ref = f
        .graph
        .insert(Authority::Worker, &Object::Claim(claim))
        .unwrap();
    assert_eq!(claim_ref, initial.obligations[0].claim);
    // Synthetic runtime evidence exercises the audit; no command is executed.
    let evidence = f
        .graph
        .insert(
            Authority::Runtime,
            &Object::Evidence(Box::new(Evidence {
                claim: claim_ref,
                scope: sealed.scope(),
                verdict: Verdict::Supports,
                mechanism: Mechanism::Deterministic,
                lineage: None,
                command: Some(CommandRecord {
                    argv: vec!["cargo".into(), "test".into()],
                    cwd: ".".into(),
                    environment: vec![],
                    tool_versions: vec![("cargo".into(), "fixture".into())],
                    exit_code: 0,
                    stdout: f.artifact,
                    stderr: f.artifact,
                    sandbox: f.artifact,
                }),
                rationale: f.artifact,
                correlation_domains: vec![],
            })),
        )
        .unwrap();
    let supported = assess(&f.graph, sealed.reference()).unwrap();
    assert!(supported.all_binding_proven());
    assert!(supported.evidence.contains(&evidence));
    assert!(supported.active_assumptions.is_empty());
    let mut other_ir = f.ir.clone();
    other_ir.goal = "A separately sealed mission".into();
    let other_goal = compile(&mut f.graph, other_ir, f.policy.clone()).unwrap();
    assert!(
        !assess(&f.graph, other_goal.reference())
            .unwrap()
            .all_binding_proven(),
        "facts cannot cross sealed goal scopes"
    );
    let mut assumption = Assumption {
        scope: sealed.scope(),
        question: "Preserve fixture behavior?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale: f.artifact,
        artifacts: vec![],
        invalidates: None,
    };
    let a = f
        .graph
        .insert(Authority::Worker, &Object::Assumption(assumption.clone()))
        .unwrap();
    f.graph.depend(evidence, a).unwrap();
    let conditional = assess(&f.graph, sealed.reference()).unwrap();
    assert!(conditional.all_binding_proven());
    assert_eq!(conditional.active_assumptions, vec![a]);
    assumption.invalidates = Some(a);
    f.graph
        .insert(Authority::Runtime, &Object::Assumption(assumption))
        .unwrap();
    let stale = assess(&f.graph, sealed.reference()).unwrap();
    assert!(!stale.all_binding_proven());
    assert!(stale.evidence.is_empty());
    assert!(stale.active_assumptions.is_empty());
    assert!(
        f.graph.get(evidence).is_ok(),
        "historical evidence remains readable"
    );
}

#[test]
fn candidate_preparation_uses_compiler_seal_and_protected_paths() {
    use myr_runner::candidate::prepare;
    let mut f = Fixture::new();
    f.policy.protected = vec!["src".into()];
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let candidate = prepare(&f.graph, sealed.reference(), &[], &[]).unwrap();
    assert_eq!(
        std::fs::read(candidate.path().join("src/lib.rs")).unwrap(),
        b"source"
    );
    let base = f.graph.register_artifact(b"source").unwrap();
    let result = f.graph.register_artifact(b"changed").unwrap();
    let delta = f
        .graph
        .insert(
            Authority::Worker,
            &Object::Delta(Delta {
                scope: sealed.scope(),
                path: "src/lib.rs".into(),
                base,
                patch: result,
                result,
                codec: DeltaCodec::Replacement,
                assumptions: vec![],
            }),
        )
        .unwrap();
    assert!(prepare(&f.graph, sealed.reference(), &[delta], &["src".into()]).is_err());
    assert_eq!(
        std::fs::read(candidate.path().join("src/lib.rs")).unwrap(),
        b"source"
    );
    let forged = f.graph.register_goal(b"uncompiled goal").unwrap();
    assert!(prepare(&f.graph, forged, &[], &[]).is_err());
}

#[test]
fn yaml_binding_preserves_goal_required_verifiers_and_protected_paths() {
    let mut f = Fixture::new();
    let mission = myr_runner::mission::parse(
        b"goal: Preserve behavior\nverify: [cargo test]\nprotected: [contract/]\n",
    )
    .unwrap();
    let sealed =
        myr_runner::mission::compile(&mut f.graph, &mission, f.ir.clone(), f.policy.clone())
            .unwrap();
    assert!(sealed.policy().protected.contains(&"contract".to_owned()));
    assert!(sealed.policy().protected.contains(&"tests".to_owned()));
    let mut ir = f.ir.clone();
    ir.goal = "Different goal".into();
    assert!(myr_runner::mission::compile(&mut f.graph, &mission, ir, f.policy.clone()).is_err());
    let mut ir = f.ir.clone();
    ir.criteria[0].binding = false;
    assert!(myr_runner::mission::compile(&mut f.graph, &mission, ir, f.policy.clone()).is_err());
    let mut ir = f.ir.clone();
    ir.criteria[0].polarity = false;
    assert!(myr_runner::mission::compile(&mut f.graph, &mission, ir, f.policy.clone()).is_err());
    let mut required = mission;
    required.verify.push(vec!["cargo".into(), "clippy".into()]);
    assert!(
        myr_runner::mission::compile(&mut f.graph, &required, f.ir.clone(), f.policy.clone())
            .is_err()
    );
}

#[test]
fn task_materializes_only_selected_assumptions_and_never_revives_invalidated_ones() {
    let mut f = Fixture::new();
    let decision = f.predicate("mission.compatibility", PredicateClass::Unresolved);
    f.ir.assumptions.push(PendingAssumption {
        atom: decision,
        question: "Keep quirks?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale: f.artifact,
        artifacts: vec![],
    });
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let draft = |decisions| TaskDraft {
        inputs: vec![],
        capabilities: vec!["write:src/".into()],
        obligations: vec![f.ir.criteria[0].atom],
        decisions,
        instruction: f.artifact,
    };
    let without = issue_task(&mut f.graph, sealed.reference(), draft(vec![])).unwrap();
    let Object::Task(task) = f.graph.get(without).unwrap() else {
        panic!()
    };
    assert!(task.assumptions.is_empty());
    let with = issue_task(&mut f.graph, sealed.reference(), draft(vec![decision])).unwrap();
    let Object::Task(task) = f.graph.get(with).unwrap() else {
        panic!()
    };
    assert_eq!(task.assumptions.len(), 1);
    assert_eq!(task.scope, sealed.scope());
    assert!(task.inputs.contains(&decision));
    assert_eq!(task.token_budget, sealed.policy().token_budget);
    let a = task.assumptions[0];
    let Object::Assumption(mut invalidation) = f.graph.get(a).unwrap() else {
        panic!()
    };
    invalidation.invalidates = Some(a);
    f.graph
        .insert(Authority::Runtime, &Object::Assumption(invalidation))
        .unwrap();
    assert!(!f.graph.live(with).unwrap());
    assert!(f.graph.live(without).unwrap());
    assert!(issue_task(&mut f.graph, sealed.reference(), draft(vec![decision])).is_err());
    assert!(!f.graph.live(a).unwrap());
    assert!(
        issue_task(
            &mut f.graph,
            sealed.reference(),
            draft(vec![f.ir.criteria[0].atom])
        )
        .is_err()
    );
    let mut unauthorized = draft(vec![]);
    unauthorized.obligations = vec![decision];
    assert!(issue_task(&mut f.graph, sealed.reference(), unauthorized).is_err());
}

#[test]
fn reopening_revalidates_format_encoding_and_current_dependencies() {
    let mut f = Fixture::new();
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let reference = sealed.reference();
    let bytes = f.graph.cas().get(reference).unwrap();
    assert_eq!(load(&f.graph, reference).unwrap().scope(), sealed.scope());
    let mut padded = bytes.clone();
    padded.push(b'\n');
    let noncanonical = f.graph.register_goal(&padded).unwrap();
    assert!(load(&f.graph, noncanonical).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["format"] = "myr-goal-ir-v999".into();
    let unknown = f
        .graph
        .register_goal(&serde_json::to_vec(&value).unwrap())
        .unwrap();
    assert!(load(&f.graph, unknown).is_err());
    let rationale = f.graph.register_artifact(b"independent rationale").unwrap();
    let a = Assumption {
        scope: sealed.scope(),
        question: "Use procedure?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale,
        artifacts: vec![],
        invalidates: None,
    };
    let assumption = f
        .graph
        .insert(Authority::Worker, &Object::Assumption(a.clone()))
        .unwrap();
    // A verifier policy can become stale through a dependency without rewriting
    // the immutable GOAL. Loading must check that policy, not only the GOAL node.
    f.graph.depend(f.artifact, assumption).unwrap();
    let mut event = a;
    event.invalidates = Some(assumption);
    f.graph
        .insert(Authority::Runtime, &Object::Assumption(event))
        .unwrap();
    assert!(f.graph.live(reference).unwrap());
    assert!(load(&f.graph, reference).is_err());
    assert_eq!(f.graph.cas().get(reference).unwrap(), bytes);
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut graph = Graph::open(
            dir.path().join("graph.sqlite"),
            Store::open(dir.path().join("cas")).unwrap(),
        )
        .unwrap();
        let (baseline, _) = import_worker(
            &mut graph,
            &Tree::from([("src/lib.rs".into(), b"source".to_vec())]),
        )
        .unwrap();
        let artifact = graph.register_artifact(&policy_bytes("1.95.0")).unwrap();
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
                    arguments: vec![Argument::Ref(artifact)],
                }),
            )
            .unwrap();
        Self {
            _dir: dir,
            graph,
            artifact,
            ir: GoalIr {
                goal: "Preserve behavior".into(),
                registry: vec![predicate],
                criteria: vec![Criterion {
                    atom,
                    polarity: true,
                    binding: true,
                    measurement: None,
                }],
                assumptions: vec![],
            },
            policy: SealPolicy {
                providers: serde_json::from_str(include_str!(
                    "../../../fixtures/providers-test-only.json"
                ))
                .unwrap(),
                baseline,
                protected: vec!["tests/".into()],
                verifier_policies: vec![artifact],
                token_budget: 100,
                call_budget: 10,
                time_budget_ms: 1000,
            },
        }
    }
    fn predicate(&mut self, name: &str, class: PredicateClass) -> ObjectRef {
        let p = self
            .graph
            .insert(
                Authority::Compiler,
                &Object::PredicateDef(PredicateDef {
                    name: name.into(),
                    version: 1,
                    arguments: vec![],
                    semantics: "Fixture semantics".into(),
                    class,
                    provenance: Some(self.artifact),
                }),
            )
            .unwrap();
        self.ir.registry.push(p);
        self.graph
            .insert(
                Authority::Compiler,
                &Object::Atom(Atom {
                    predicate: p,
                    arguments: vec![],
                }),
            )
            .unwrap()
    }
}

#[test]
fn seal_commits_goal_acceptance_baseline_protection_verifiers_and_budgets() {
    let mut f = Fixture::new();
    let original = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    let reference = original.reference();
    assert_eq!(original.scope().snapshot, f.policy.baseline);
    assert_eq!(original.policy().protected, vec!["tests"]);
    assert_eq!(
        compile(&mut f.graph, f.ir.clone(), f.policy.clone())
            .unwrap()
            .reference(),
        reference
    );
    let mut changed = f.ir.clone();
    changed.goal.push('!');
    assert_ne!(
        compile(&mut f.graph, changed, f.policy.clone())
            .unwrap()
            .reference(),
        reference
    );
    let mut changed = f.ir.clone();
    changed.criteria[0].polarity = false;
    assert_ne!(
        compile(&mut f.graph, changed, f.policy.clone())
            .unwrap()
            .reference(),
        reference
    );
    for field in 0..7 {
        let mut policy = f.policy.clone();
        match field {
            0 => policy.protected.push("myr.yaml".into()),
            1 => policy.token_budget += 1,
            2 => policy.call_budget += 1,
            3 => policy.time_budget_ms += 1,
            6 => policy.providers.worker.model.push('!'),
            4 => policy
                .verifier_policies
                .push(f.graph.register_artifact(&policy_bytes("1.95.1")).unwrap()),
            _ => {
                policy.baseline = import_worker(
                    &mut f.graph,
                    &Tree::from([("src/lib.rs".into(), b"different".to_vec())]),
                )
                .unwrap()
                .0
            }
        }
        assert_ne!(
            compile(&mut f.graph, f.ir.clone(), policy)
                .unwrap()
                .reference(),
            reference
        );
    }
    assert!(f.graph.live(reference).unwrap());
}

#[test]
fn predicate_classes_and_registry_are_enforced_before_sealing() {
    let mut f = Fixture::new();
    let atom = f.predicate("mission.style", PredicateClass::Heuristic);
    f.ir.criteria.push(Criterion {
        atom,
        polarity: true,
        binding: true,
        measurement: None,
    });
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
    f.ir.criteria.last_mut().unwrap().binding = false;
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_ok());
    let empirical = f.predicate("mission.latency", PredicateClass::Empirical);
    f.ir.criteria.push(Criterion {
        atom: empirical,
        polarity: true,
        binding: true,
        measurement: None,
    });
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
    f.ir.criteria.last_mut().unwrap().measurement = Some(Measurement {
        procedure: f.artifact,
        tolerance: Decimal {
            mantissa: 1,
            exponent: -2,
        },
    });
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
    let mut procedure = myr_runner::measurement::Procedure {
        format: "myr-measurement-procedure-v0".into(),
        command: f
            .graph
            .register_artifact(&policy_bytes("unsealed-version"))
            .unwrap(),
        target: Decimal {
            mantissa: 16,
            exponent: 0,
        },
        relation: myr_runner::measurement::Relation::AtMost,
        unit: "milliseconds".into(),
    };
    let unsealed = f
        .graph
        .register_artifact(&procedure.encode().unwrap())
        .unwrap();
    f.ir.criteria
        .last_mut()
        .unwrap()
        .measurement
        .as_mut()
        .unwrap()
        .procedure = unsealed;
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
    procedure.command = f.policy.verifier_policies[0];
    let registered = f
        .graph
        .register_artifact(&procedure.encode().unwrap())
        .unwrap();
    f.ir.criteria
        .last_mut()
        .unwrap()
        .measurement
        .as_mut()
        .unwrap()
        .procedure = registered;
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_ok());
    f.ir.registry.clear();
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
}

#[test]
fn unresolved_decisions_are_explicit_pending_assumptions_and_conflicts_rejected() {
    let mut f = Fixture::new();
    let atom = f.predicate("mission.preserve_bugs", PredicateClass::Unresolved);
    f.ir.criteria.push(Criterion {
        atom,
        polarity: true,
        binding: false,
        measurement: None,
    });
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
    f.ir.criteria.pop();
    f.ir.assumptions.push(PendingAssumption {
        atom,
        question: "Preserve quirks?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale: f.artifact,
        artifacts: vec![],
    });
    let sealed = compile(&mut f.graph, f.ir.clone(), f.policy.clone()).unwrap();
    assert_eq!(sealed.ir().assumptions.len(), 1);
    let mut opposite = f.ir.criteria[0].clone();
    opposite.polarity = false;
    f.ir.criteria.push(opposite);
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
    f.ir.criteria.pop();
    f.ir.assumptions[0].alternatives.clear();
    assert!(compile(&mut f.graph, f.ir.clone(), f.policy.clone()).is_err());
}
