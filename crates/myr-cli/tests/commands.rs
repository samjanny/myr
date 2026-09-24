use myr_core::*;
use myr_graph::{Authority, Graph};
use std::process::Command;

fn myr() -> Command {
    Command::new(env!("CARGO_BIN_EXE_myr"))
}

#[test]
fn benchmark_plan_is_reproducible_and_does_not_execute_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedule-input.json");
    let cases:Vec<_> = (0..8).map(|i| serde_json::json!({"id":format!("case-{i}"),"poison_type":format!("type-{}",i%3),"case_manifest":Cid([i;32])})).collect();
    let input = serde_json::json!({"cases":cases,"configuration":Cid([99;32]),"seed":42});
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let first = myr().arg("plan-benchmark").arg(&path).output().unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let plan: myr_runner::schedule::Schedule = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(plan.jobs.len(), 160);
    assert!(!plan.executed && !plan.admission_checked);
    myr_runner::schedule::verify(&plan).unwrap();
    let second = myr().arg("plan-benchmark").arg(&path).output().unwrap();
    assert!(second.status.success());
    assert_eq!(first.stdout, second.stdout);
    let collection = serde_json::json!({"schedule":plan,"receipts":[],"bootstrap_samples":1000,"tail_ppm":50000,"bootstrap_seed":42});
    std::fs::write(&path, serde_json::to_vec(&collection).unwrap()).unwrap();
    let collected = myr().arg("collect-benchmark").arg(&path).output().unwrap();
    assert!(
        collected.status.success(),
        "{}",
        String::from_utf8_lossy(&collected.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&collected.stdout).unwrap();
    assert_eq!(report["coverage"]["pending_pairs"], 80);
    assert_eq!(
        report["coverage"]["unavailable_pairs_exceed_ten_percent"],
        false
    );
    assert!(report["analysis"].is_null());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn benchmark_analysis_reports_numerical_bounds_without_official_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let run = |oracle, tokens| serde_json::json!({"oracle":oracle,"communication_tokens":tokens,"structured_output_failed":false});
    let repetition = serde_json::json!({
        "prose":{"clean":run("PASS",100),"poisoned":run("HARMFUL",100)},
        "myr":{"clean":run("PASS",50),"poisoned":run("PASS",50)}
    });
    let cases: Vec<_> = (0..8)
        .map(|i| {
            serde_json::json!({
                "id":format!("fixture-{i}"),"poison_type":format!("type-{}",i%3),
                "repetitions":vec![repetition.clone();5]
            })
        })
        .collect();
    let path = dir.path().join("complete.json");
    let mut input =
        serde_json::json!({"cases":cases,"bootstrap_samples":1000,"tail_ppm":50000,"seed":42});
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let output = myr().arg("analyze-benchmark").arg(&path).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["official_acceptance_checked"], false);
    assert_eq!(report["numerical_thresholds_met"], true);
    assert_eq!(report["point"]["absolute_pcr_reduction"], 1.0);
    assert_eq!(report["by_poison_type"]["type-2"]["cases"], 2);
    assert_eq!(
        report["by_poison_type"]["type-2"]["point"]["prose_propagated_pairs"],
        10
    );
    input["cases"][0]["repetitions"][0]["myr"]["clean"]["oracle"] = "INVALID".into();
    std::fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let output = myr().arg("analyze-benchmark").arg(&path).output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

fn runtime_config(dir: &std::path::Path) -> serde_json::Value {
    let mut providers: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/providers-test-only.json")).unwrap();
    for role in ["planner", "worker", "reviewer_a", "reviewer_b"] {
        providers[role]["backend"]["executable"] =
            serde_json::to_value(dir.join(format!("absent-{role}.exe"))).unwrap();
    }
    serde_json::json!({"providers":providers,"commands":[{
        "kind":"command","argv":["cargo","test"],"cwd":".","environment":{},"tool_versions":{"cargo":"fixture"},
        "timeout_ms":1000,"max_output_bytes":4096,
        "sandbox":{"backend":{"docker_linux":{"image":format!("sha256:{}","a".repeat(64))}},"network":false,"memory_bytes":67108864,"max_processes":16}
    }],"measurements":[{"command_index":0,"target":{"mantissa":160,"exponent":-1},"relation":"at_most","unit":"milliseconds"}],
    "writable":["file.bin"],"token_budget":1000000,"call_budget":10,"time_budget_ms":30000,
    "max_native_output_tokens":1024,"max_reference_output_tokens":4096,"docker_executable":dir.join("absent-docker.exe")})
}

#[test]
fn run_preparation_preserves_bytes_and_failed_execution_records_partial() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("file.bin"), [0, 255, 13, 10]).unwrap();
    let mission = dir.path().join("myr.yaml");
    std::fs::write(
        &mission,
        "goal: Preserve behavior\nverify: [cargo test]\nprotected: [tests/]\n",
    )
    .unwrap();
    let config = dir.path().join("runtime.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&runtime_config(dir.path())).unwrap(),
    )
    .unwrap();
    let root = dir.path().join("prepared");
    let output = myr()
        .arg("run")
        .arg(&mission)
        .arg("--config")
        .arg(&config)
        .arg("--repository")
        .arg(&repo)
        .arg("--root")
        .arg(&root)
        .arg("--prepare-only")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["executed"], false);
    assert!(!root.join("private-audit").exists());
    let prepared: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("prepared.json")).unwrap()).unwrap();
    assert_eq!(
        prepared["policy"]["protected"],
        serde_json::json!(["tests"])
    );
    let cas = myr_cas::Store::open(root.join("objects")).unwrap();
    let measurement_ref: ObjectRef =
        serde_json::from_value(prepared["measurement_procedures"][0].clone()).unwrap();
    let procedure: myr_runner::measurement::Procedure =
        serde_json::from_slice(&cas.get(measurement_ref).unwrap()).unwrap();
    assert_eq!(
        procedure.target,
        Decimal {
            mantissa: 16,
            exponent: 0
        }
    );
    assert_eq!(procedure.unit, "milliseconds");
    let seal_policy: myr_runner::goal::SealPolicy =
        serde_json::from_value(prepared["policy"].clone()).unwrap();
    assert!(seal_policy.verifier_policies.contains(&procedure.command));
    assert!(
        prepared["planner_schema"]
            .to_string()
            .contains(&measurement_ref.cid.to_string())
    );
    let snapshot: ObjectRef = serde_json::from_value(result["baseline"].clone()).unwrap();
    let files: std::collections::BTreeMap<String, ObjectRef> =
        serde_json::from_slice(&cas.get(snapshot).unwrap()).unwrap();
    assert_eq!(cas.get(files["file.bin"]).unwrap(), [0, 255, 13, 10]);
    let run_root = dir.path().join("failed-run");
    // All configured executables are nonexistent absolute paths. No model or
    // Docker can run; this exercises actual CLI failure reporting without quota.
    let output = myr()
        .arg("run")
        .arg(&mission)
        .arg("--config")
        .arg(&config)
        .arg("--repository")
        .arg(&repo)
        .arg("--root")
        .arg(&run_root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["state"], "PARTIAL");
    assert_eq!(result["phase"], "planning");
    assert_eq!(result["budget"]["calls_started"], 1);
    assert_eq!(result["evidence"], serde_json::json!([]));
    assert_eq!(result["active_assumptions"], serde_json::json!([]));
    let graph = Graph::open(
        run_root.join("graph.sqlite"),
        myr_cas::Store::open(run_root.join("objects")).unwrap(),
    )
    .unwrap();
    let failure: ObjectRef = serde_json::from_value(result["failure"].clone()).unwrap();
    let Object::Fail(fail) = graph.get(failure).unwrap() else {
        panic!("missing failure")
    };
    let provenance: Vec<ObjectRef> = serde_json::from_value(result["provenance"].clone()).unwrap();
    let artifacts: Vec<ObjectRef> = serde_json::from_value(result["artifacts"].clone()).unwrap();
    assert!(provenance.contains(&failure));
    assert!(artifacts.contains(&fail.diagnostic));
    assert!(artifacts.contains(&snapshot));
    assert!(artifacts.contains(&files["file.bin"]));
    for reference in &provenance {
        assert!(graph.live(*reference).unwrap());
    }
    assert!(!graph.cas().get(fail.diagnostic).unwrap().is_empty());
    let private_record: ObjectRef =
        serde_json::from_value(result["private_record"].clone()).unwrap();
    let audit = myr_cas::Store::open(run_root.join("private-audit")).unwrap();
    let immutable: serde_json::Value =
        serde_json::from_slice(&audit.get(private_record).unwrap()).unwrap();
    let mut public = result.clone();
    public.as_object_mut().unwrap().remove("private_record");
    assert_eq!(immutable, public);
    assert!(run_root.join("result.json").is_file());
    assert_eq!(
        std::fs::read(repo.join("file.bin")).unwrap(),
        [0, 255, 13, 10]
    );
}

#[test]
fn run_rejects_invalid_measurement_configuration_before_creating_store() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    let mission = dir.path().join("mission.yaml");
    std::fs::write(&mission, "goal: Measure latency\nverify: [cargo test]\n").unwrap();
    let path = dir.path().join("runtime.json");
    for mode in 0..4 {
        let mut config = runtime_config(dir.path());
        match mode {
            0 => config["measurements"][0]["command_index"] = 99.into(),
            1 => config["measurements"][0]["unit"] = " ".into(),
            2 => {
                let duplicate = config["measurements"][0].clone();
                config["measurements"]
                    .as_array_mut()
                    .unwrap()
                    .push(duplicate);
            }
            _ => config["measurements"][0]["target"]["exponent"] = i64::MAX.into(),
        }
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let root = dir.path().join(format!("invalid-{mode}"));
        let result = myr()
            .arg("run")
            .arg(&mission)
            .arg("--config")
            .arg(&path)
            .arg("--repository")
            .arg(&repo)
            .arg("--root")
            .arg(&root)
            .arg("--prepare-only")
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!root.exists());
    }
}

#[test]
fn run_rejects_missing_verifier_and_existing_output_before_preparation() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repository");
    std::fs::create_dir(&repo).unwrap();
    let mission = dir.path().join("myr.yaml");
    std::fs::write(
        &mission,
        "goal: Preserve behavior\nverify: [cargo clippy]\n",
    )
    .unwrap();
    let config = dir.path().join("runtime.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&runtime_config(dir.path())).unwrap(),
    )
    .unwrap();
    let root = dir.path().join("store");
    let run = || {
        myr()
            .arg("run")
            .arg(&mission)
            .arg("--config")
            .arg(&config)
            .arg("--repository")
            .arg(&repo)
            .arg("--root")
            .arg(&root)
            .arg("--prepare-only")
            .output()
            .unwrap()
    };
    assert!(!run().status.success());
    assert!(!root.exists());
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("sentinel"), b"keep").unwrap();
    assert!(!run().status.success());
    assert_eq!(std::fs::read(root.join("sentinel")).unwrap(), b"keep");
    assert!(!root.join("graph.sqlite").exists());
}

#[test]
fn snapshot_imports_repository_bytes_into_new_store_without_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repository");
    let root = dir.path().join("store");
    std::fs::create_dir(&repo).unwrap();
    std::fs::write(repo.join("file.bin"), b"\xff\0\r\n").unwrap();
    let output = myr()
        .arg("snapshot")
        .arg(&repo)
        .arg("--root")
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let reference: ObjectRef = serde_json::from_value(result["files"]["file.bin"].clone()).unwrap();
    let graph = Graph::open(
        root.join("graph.sqlite"),
        myr_cas::Store::open(root.join("objects")).unwrap(),
    )
    .unwrap();
    assert_eq!(graph.cas().get(reference).unwrap(), b"\xff\0\r\n");
    assert!(
        graph
            .live(serde_json::from_value(result["snapshot"].clone()).unwrap())
            .unwrap()
    );
    assert!(
        !myr()
            .arg("snapshot")
            .arg(&repo)
            .arg("--root")
            .arg(&root)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(std::fs::read(repo.join("file.bin")).unwrap(), b"\xff\0\r\n");
}

#[test]
fn mission_yaml_check_does_not_execute_or_create_a_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("myr.yaml");
    std::fs::write(
        &path,
        "goal: Preserve behavior\nverify:\n  - cargo test\nprotected: [tests/, myr.yaml]\n",
    )
    .unwrap();
    let output = myr()
        .arg("check-mission")
        .arg(&path)
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(parsed["verify"][0], serde_json::json!(["cargo", "test"]));
    assert!(!dir.path().join(".myr").exists());
    std::fs::write(&path, "goal: x\nverify: ['cargo test && echo wrong']\n").unwrap();
    assert!(
        !myr()
            .arg("check-mission")
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn seal_and_inspect_validate_real_store_and_reject_unknown_ir_fields() {
    use myr_runner::{goal::*, verifier::*, views::*};
    use std::collections::BTreeMap;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut graph = Graph::open(
        root.join("graph.sqlite"),
        myr_cas::Store::open(root.join("objects")).unwrap(),
    )
    .unwrap();
    let (baseline, _) = import_worker(
        &mut graph,
        &Tree::from([("main.rs".into(), b"fn main() {}".to_vec())]),
    )
    .unwrap();
    let command = VerifierPolicy::Command {
        argv: vec!["rustc".into(), "main.rs".into()],
        cwd: ".".into(),
        environment: BTreeMap::new(),
        tool_versions: BTreeMap::from([("rustc".into(), "1.95.0".into())]),
        timeout_ms: 1000,
        max_output_bytes: 4096,
        sandbox: SandboxPolicy {
            backend: SandboxBackend::WindowsAppContainer,
            network: false,
            memory_bytes: 1024 * 1024 * 1024,
            max_processes: 16,
        },
    };
    let policy_ref = graph.register_artifact(&command.encode().unwrap()).unwrap();
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
                arguments: vec![Argument::Ref(policy_ref)],
            }),
        )
        .unwrap();
    let ir = GoalIr {
        goal: "Compile the program".into(),
        registry: vec![predicate],
        criteria: vec![Criterion {
            atom,
            polarity: true,
            binding: true,
            measurement: None,
        }],
        assumptions: vec![],
    };
    let policy = SealPolicy {
        providers: serde_json::from_str(include_str!("../../../fixtures/providers-test-only.json"))
            .unwrap(),
        baseline,
        protected: vec!["myr.yaml".into()],
        verifier_policies: vec![policy_ref],
        token_budget: 1000,
        call_budget: 10,
        time_budget_ms: 10000,
    };
    drop(graph);
    let ir_path = root.join("ir.json");
    let policy_path = root.join("policy.json");
    std::fs::write(&ir_path, serde_json::to_vec(&ir).unwrap()).unwrap();
    std::fs::write(&policy_path, serde_json::to_vec(&policy).unwrap()).unwrap();
    let seal = || {
        myr()
            .arg("seal")
            .arg("--ir")
            .arg(&ir_path)
            .arg("--policy")
            .arg(&policy_path)
            .arg("--root")
            .arg(root)
            .output()
            .unwrap()
    };
    let output = seal();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let sealed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut graph = Graph::open(
        root.join("graph.sqlite"),
        myr_cas::Store::open(root.join("objects")).unwrap(),
    )
    .unwrap();
    let goal_ref: ObjectRef = serde_json::from_value(sealed["reference"].clone()).unwrap();
    let candidate =
        myr_runner::candidate::prepare_recorded(&mut graph, goal_ref, &[], &[]).unwrap();
    let candidate_cid = candidate.reference().cid.to_string();
    drop(graph);
    let export_parent = tempfile::tempdir().unwrap();
    let destination = export_parent.path().join("candidate");
    let exported = myr()
        .args(["export-candidate", &candidate_cid])
        .arg(&destination)
        .arg("--root")
        .arg(root)
        .output()
        .unwrap();
    assert!(
        exported.status.success(),
        "{}",
        String::from_utf8_lossy(&exported.stderr)
    );
    let receipt: serde_json::Value = serde_json::from_slice(&exported.stdout).unwrap();
    assert_eq!(receipt["mission_completion_checked"], false);
    assert_eq!(
        std::fs::read(destination.join("main.rs")).unwrap(),
        b"fn main() {}"
    );
    assert!(
        !myr()
            .args(["export-candidate", &candidate_cid])
            .arg(&destination)
            .arg("--root")
            .arg(root)
            .output()
            .unwrap()
            .status
            .success()
    );
    let inside = root.join("export-forbidden");
    assert!(
        !myr()
            .args(["export-candidate", &candidate_cid])
            .arg(&inside)
            .arg("--root")
            .arg(root)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!inside.exists());
    let cid = sealed["reference"]["cid"].as_str().unwrap();
    let inspected = myr()
        .args(["inspect-goal", cid, "--root"])
        .arg(root)
        .output()
        .unwrap();
    assert!(inspected.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&inspected.stdout).unwrap(),
        sealed
    );
    let mission_path = root.join("myr.yaml");
    std::fs::write(
        &mission_path,
        "goal: Compile the program\nverify: [rustc main.rs]\nprotected: [tests/]\n",
    )
    .unwrap();
    let bound = || {
        myr()
            .arg("seal")
            .arg("--ir")
            .arg(&ir_path)
            .arg("--policy")
            .arg(&policy_path)
            .arg("--mission")
            .arg(&mission_path)
            .arg("--root")
            .arg(root)
            .output()
            .unwrap()
    };
    let output = bound();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["policy"]["protected"],
        serde_json::json!(["myr.yaml", "tests"])
    );
    std::fs::write(
        &mission_path,
        "goal: A different goal\nverify: [rustc main.rs]\n",
    )
    .unwrap();
    assert!(!bound().status.success());
    let mut invalid = serde_json::to_value(ir).unwrap();
    invalid["silently_ignored"] = true.into();
    std::fs::write(&ir_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    assert!(!seal().status.success());
    let missing = root.join("absent");
    assert!(
        !myr()
            .args(["inspect-goal", cid, "--root"])
            .arg(&missing)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!missing.exists());
}

#[test]
fn pilot_and_show_work_through_the_public_binary() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("pilot");
    let output = myr().arg("pilot").arg(&root).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["development_only"], true);
    let cid = report["runs"][3]["facts"][0]["cid"].as_str().unwrap();
    let shown = myr()
        .args(["show", cid, "--root"])
        .arg(root.join("mw0-poisoned"))
        .output()
        .unwrap();
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let object: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(object["live"], true);
    assert_eq!(object["object"]["kind"], "FACT");
    let repeat = myr().arg("pilot").arg(&root).output().unwrap();
    assert!(!repeat.status.success());
}

#[test]
fn invalidate_records_reason_and_keeps_historical_assumption_readable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let mut graph = Graph::open(
        root.join("graph.sqlite"),
        myr_cas::Store::open(root.join("objects")).unwrap(),
    )
    .unwrap();
    let blob = graph.register_artifact(b"fixture").unwrap();
    let goal = graph.register_goal(b"fixture goal").unwrap();
    let assumption = graph
        .insert(
            Authority::Worker,
            &Object::Assumption(Assumption {
                scope: Scope {
                    goal,
                    snapshot: blob,
                },
                question: "Preserve quirks?".into(),
                chosen: "yes".into(),
                alternatives: vec!["no".into()],
                rationale: blob,
                artifacts: vec![blob],
                invalidates: None,
            }),
        )
        .unwrap();
    drop(graph);
    let cid = assumption.cid.to_string();
    let output = myr()
        .args(["invalidate", &cid, "--root"])
        .arg(root)
        .args([
            "--reason",
            "The contract explicitly rejects the old behavior.",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["live"], false);
    let shown = myr()
        .args(["show", &cid, "--root"])
        .arg(root)
        .output()
        .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(result["live"], false);
    assert_eq!(result["object"]["kind"], "ASSUMPTION");
}

#[test]
fn read_command_does_not_create_a_missing_store() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("absent");
    let output = myr()
        .args(["show", &Cid([0; 32]).to_string(), "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.exists());
}
