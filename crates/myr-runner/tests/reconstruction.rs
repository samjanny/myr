use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};
use myr_runner::{
    delta::reconstruct_live,
    views::{Tree, import_worker},
};

#[test]
fn sealed_snapshot_and_live_delta_required_after_assumption_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let cas = Store::open(dir.path().join("cas")).unwrap();
    let mut graph = Graph::open(dir.path().join("graph.sqlite"), cas).unwrap();
    let baseline = Tree::from([
        ("src/lib.rs".into(), b"old\n".to_vec()),
        ("tests/check.rs".into(), b"protected\n".to_vec()),
    ]);
    let (snapshot, files) = import_worker(&mut graph, &baseline).unwrap();
    let goal = graph.register_goal(b"sealed goal").unwrap();
    let scope = Scope { goal, snapshot };
    let rationale = graph.register_artifact(b"Preserve public API").unwrap();
    let mut assumption = Assumption {
        scope: scope.clone(),
        question: "Keep API?".into(),
        chosen: "yes".into(),
        alternatives: vec!["no".into()],
        rationale,
        artifacts: vec![],
        invalidates: None,
    };
    let a = graph
        .insert(Authority::Worker, &Object::Assumption(assumption.clone()))
        .unwrap();
    let output = graph.register_artifact(b"new\n").unwrap();
    let delta = graph
        .insert(
            Authority::Worker,
            &Object::Delta(Delta {
                scope: scope.clone(),
                path: "src/lib.rs".into(),
                base: files["src/lib.rs"],
                patch: output,
                result: output,
                codec: DeltaCodec::Replacement,
                assumptions: vec![a],
            }),
        )
        .unwrap();
    let writable = vec!["src".into()];
    let protected = vec!["tests".into()];
    let candidate = reconstruct_live(&graph, &scope, &[delta], &writable, &protected).unwrap();
    assert_eq!(candidate["src/lib.rs"], b"new\n");
    assert_eq!(candidate["tests/check.rs"], b"protected\n");
    assert_eq!(
        reconstruct_live(&graph, &scope, &[], &writable, &protected).unwrap(),
        baseline
    );
    let alternate = Tree::from([("src/lib.rs".into(), b"other\n".to_vec())]);
    let (snapshot, _) = import_worker(&mut graph, &alternate).unwrap();
    assert!(
        reconstruct_live(
            &graph,
            &Scope { goal, snapshot },
            &[delta],
            &writable,
            &protected
        )
        .is_err()
    );
    assumption.invalidates = Some(a);
    graph
        .insert(Authority::Runtime, &Object::Assumption(assumption))
        .unwrap();
    assert!(
        graph.get(delta).is_ok(),
        "historical object remains readable"
    );
    assert!(reconstruct_live(&graph, &scope, &[delta], &writable, &protected).is_err());
    assert_eq!(
        reconstruct_live(&graph, &scope, &[], &writable, &protected).unwrap(),
        baseline
    );
}

#[test]
fn snapshot_rejects_unregistered_files_and_duplicate_keys() {
    let dir = tempfile::tempdir().unwrap();
    let cas = Store::open(dir.path().join("cas")).unwrap();
    let mut graph = Graph::open(dir.path().join("graph.sqlite"), cas).unwrap();
    let goal = graph.register_goal(b"goal").unwrap();
    let orphan = graph.cas().put_artifact(b"not registered").unwrap();
    let map = std::collections::BTreeMap::from([("f", orphan)]);
    let snapshot = graph
        .register_artifact(&serde_json::to_vec(&map).unwrap())
        .unwrap();
    assert!(reconstruct_live(&graph, &Scope { goal, snapshot }, &[], &[], &[]).is_err());
    let file = graph.register_artifact(b"registered").unwrap();
    let reference = serde_json::to_string(&file).unwrap();
    let malformed = format!("{{\"f\":{reference},\"f\":{reference}}}");
    let snapshot = graph.register_artifact(malformed.as_bytes()).unwrap();
    assert!(reconstruct_live(&graph, &Scope { goal, snapshot }, &[], &[], &[]).is_err());
}
