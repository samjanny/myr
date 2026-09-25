//! Live Docker confinement acceptance. Ignored by default because it needs a
//! local daemon. Run it explicitly with a pinned local image (never pulled):
//!
//! MYR_LIVE_DOCKER_IMAGE=sha256:<id> cargo test -p myr-runner --test live_docker -- --ignored
//!
//! The image must provide busybox-style `/usr/bin/env`, `/bin/sh`, `/bin/cp`
//! and `wget`. No model is called.
use myr_graph::Graph;
use myr_runner::{
    candidate, docker_sandbox, mission, prose,
    setup::{self, Config},
    views::Tree,
};
use serde_json::json;

/// (argv, expected exit code, expected stdout)
fn probes() -> Vec<(Vec<&'static str>, i64, Option<String>)> {
    vec![
        // Positive: the candidate tree is present in the working copy.
        (vec!["sh", "check.sh"], 0, Some("checked\n".into())),
        // Ordinary nonzero exits are retained, not reclassified.
        (vec!["sh", "-c", "exit 3"], 3, None),
        // Network is disabled.
        (
            vec![
                "sh",
                "-c",
                "wget -q -T 3 -O /dev/null http://1.1.1.1/ 2>/dev/null && exit 0 || exit 9",
            ],
            9,
            None,
        ),
        // The root filesystem and the input mount are read-only.
        (
            vec![
                "sh",
                "-c",
                "touch /etc/myr-probe 2>/dev/null && exit 0 || exit 8",
            ],
            8,
            None,
        ),
        (
            vec![
                "sh",
                "-c",
                "touch /input/myr-probe 2>/dev/null && exit 0 || exit 7",
            ],
            7,
            None,
        ),
        // The bounded working tmpfs is writable.
        (
            vec!["sh", "-c", "echo ok > /work/probe && cat /work/probe"],
            0,
            Some("ok\n".into()),
        ),
        // Every Linux capability is dropped.
        (
            vec![
                "sh",
                "-c",
                "while read k v; do [ \"$k\" = CapEff: ] && echo \"$v\"; done < /proc/self/status; true",
            ],
            0,
            Some("0000000000000000\n".into()),
        ),
        // No privilege escalation.
        (
            vec![
                "sh",
                "-c",
                "while read k v; do [ \"$k\" = NoNewPrivs: ] && echo \"$v\"; done < /proc/self/status; true",
            ],
            0,
            Some("1\n".into()),
        ),
    ]
}

#[test]
#[ignore = "requires a local Docker daemon and MYR_LIVE_DOCKER_IMAGE"]
fn live_docker_sandbox_confinement() {
    let image = std::env::var("MYR_LIVE_DOCKER_IMAGE").expect("MYR_LIVE_DOCKER_IMAGE");
    let docker = std::env::var("MYR_LIVE_DOCKER").unwrap_or_else(|_| "/usr/bin/docker".into());
    let dir = tempfile::tempdir().unwrap();
    let mut graph = Graph::open(
        dir.path().join("graph.sqlite"),
        myr_cas::Store::open(dir.path().join("shared")).unwrap(),
    )
    .unwrap();
    let probes = probes();
    let verify: Vec<_> = probes.iter().map(|(argv, _, _)| argv.clone()).collect();
    let mission = mission::parse(
        serde_yaml_ng::to_string(&json!({"goal":"Live sandbox acceptance","verify":verify}))
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let providers: serde_json::Value =
        serde_json::from_str(include_str!("../../../fixtures/providers-test-only.json")).unwrap();
    let commands: Vec<_> = verify
        .iter()
        .map(|argv| {
            json!({"kind":"command","argv":argv,"cwd":".","environment":{},
                "tool_versions":{"sh":"busybox (image)"},"timeout_ms":30000,"max_output_bytes":65536,
                "sandbox":{"backend":{"docker_linux":{"image":image}},"network":false,
                    "memory_bytes":67108864,"max_processes":32}})
        })
        .collect();
    let config: Config = serde_json::from_value(json!({"providers":providers,"commands":commands,
        "writable":[],"token_budget":1000,"call_budget":1,"time_budget_ms":600000,
        "max_native_output_tokens":1,"max_reference_output_tokens":1,"docker_executable":docker}))
    .unwrap();
    let tree = Tree::from([("check.sh".into(), b"echo checked\n".to_vec())]);
    let prepared =
        setup::prepare(&mut graph, &mission, &tree, &config, &dir.path().join("q")).unwrap();
    let sealed = prose::seal_acceptance(
        &mut graph,
        &mission,
        &prepared.policy,
        &prepared.registry,
        &prepared.command_atoms,
    )
    .unwrap();
    let recorded = candidate::prepare_recorded(&mut graph, sealed.reference(), &[], &[]).unwrap();
    let runtime = docker_sandbox::DockerRuntime {
        executable: docker.into(),
    };
    for (index, (argv, exit, stdout)) in probes.iter().enumerate() {
        let policy = myr_core::ObjectRef::new(
            myr_core::Kind::Artifact,
            myr_wire::artifact_cid(&config.commands[index].encode().unwrap()),
        );
        let execution = docker_sandbox::execute(&graph, &recorded, policy, &runtime)
            .unwrap_or_else(|e| panic!("{argv:?}: {e}"));
        assert_eq!(execution.exit_code, *exit, "{argv:?}");
        if let Some(stdout) = stdout {
            assert_eq!(
                String::from_utf8_lossy(&execution.stdout),
                *stdout,
                "{argv:?}"
            );
        }
    }
}
