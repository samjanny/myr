//! Explicit local Docker execution. No image pulls, backend fallback, or FACT
//! promotion. Live confinement acceptance is separate from configuration checks.
use crate::{
    candidate::{self, RecordedCandidate},
    goal, snapshot,
    verifier::{self, SandboxBackend, VerifierPolicy},
};
use myr_adapter::process::{self, Output, OutputLimits};
use myr_core::ObjectRef;
use myr_graph::Graph;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const BOOTSTRAP: &str = "/bin/cp -R /input/. /work/ || exit 125; cd \"$1\" || exit 125; shift; exec /usr/bin/env -i -- \"$@\"";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sandbox preparation: {0}")]
    Runner(#[from] crate::Error),
    #[error("sandbox I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("sandbox subprocess: {0}")]
    Process(#[from] process::Error),
    #[error("sandbox metadata: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sandbox unavailable or configuration rejected: {0}")]
    Rejected(String),
    #[error("sandbox cleanup failed for {container}; manual cleanup required")]
    Cleanup { container: String },
}
type Result<T> = std::result::Result<T, Error>;
fn reject(message: &str) -> Error {
    Error::Rejected(message.into())
}

pub struct DockerRuntime {
    pub executable: PathBuf,
}

/// Raw execution receipt. It is not EVIDENCE and cannot establish a FACT by itself.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub candidate: ObjectRef,
    pub policy: ObjectRef,
    pub exit_code: i64,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub image_metadata: Value,
    pub daemon_metadata: Value,
    pub container_metadata: Value,
    pub create_argv: Vec<String>,
}

struct Cli<'a> {
    runtime: &'a DockerRuntime,
    config: &'a Path,
}
impl Cli<'_> {
    fn call(&self, args: &[String], timeout: Duration, cap: usize) -> Result<Output> {
        let mut command = Command::new(&self.runtime.executable);
        command.env_clear().current_dir(self.config);
        for name in ["SystemRoot", "WINDIR", "TEMP", "TMP", "TMPDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let host = if cfg!(windows) {
            "npipe:////./pipe/docker_engine"
        } else {
            "unix:///var/run/docker.sock"
        };
        command
            .arg("--config")
            .arg(self.config)
            .args(["--host", host])
            .args(args);
        Ok(process::run_bounded(
            command,
            vec![],
            timeout,
            OutputLimits {
                stdout_bytes: cap,
                stderr_bytes: cap,
                combined_bytes: cap,
            },
        )?)
    }
}

fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| process::Error::Timeout.into())
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}
fn metadata(output: Output) -> Result<Value> {
    if !output.success {
        return Err(reject("Docker metadata command failed"));
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn check_daemon(value: &Value) -> Result<()> {
    if value["OSType"] != "linux"
        || value["MemoryLimit"] != true
        || value["SwapLimit"] != true
        || value["PidsLimit"] != true
        || !matches!(value["CgroupDriver"].as_str(), Some("systemd" | "cgroupfs"))
    {
        return Err(reject(
            "Linux daemon with memory, swap and PID cgroup enforcement required",
        ));
    }
    if !value["SecurityOptions"].as_array().is_some_and(|options| {
        options.iter().any(|v| {
            v.as_str()
                .is_some_and(|s| s.starts_with("name=seccomp,") && s.contains("profile=builtin"))
        })
    }) {
        return Err(reject("default seccomp enforcement required"));
    }
    Ok(())
}
fn one(value: &Value) -> Result<&Value> {
    let array = value
        .as_array()
        .filter(|a| a.len() == 1)
        .ok_or_else(|| reject("expected one Docker object"))?;
    Ok(&array[0])
}
fn check_image(value: &Value, image: &str) -> Result<()> {
    let value = one(value)?;
    if value["Id"] != image
        || value["Os"] != "linux"
        || !(value["Config"]["Volumes"].is_null()
            || value["Config"]["Volumes"]
                .as_object()
                .is_some_and(|m| m.is_empty()))
    {
        return Err(reject("image identity, OS or declared volumes rejected"));
    }
    Ok(())
}

struct Plan {
    args: Vec<String>,
    image: String,
    memory: u64,
    pids: u32,
    user: String,
    input: String,
    command: Vec<String>,
    owner: String,
}
fn plan(policy: &VerifierPolicy, candidate: &Path, name: &str) -> Result<Plan> {
    policy.validate()?;
    let VerifierPolicy::Command {
        argv,
        cwd,
        environment,
        sandbox,
        ..
    } = policy
    else {
        return Err(reject("command policy required"));
    };
    let SandboxBackend::DockerLinux { image } = &sandbox.backend else {
        return Err(reject("Docker backend was not explicitly selected"));
    };
    let input = candidate
        .to_str()
        .ok_or_else(|| reject("non-UTF-8 candidate path"))?
        .to_owned();
    if input.contains([',', '"', '\n', '\r']) || !candidate.is_absolute() {
        return Err(reject(
            "candidate path cannot be represented by a Docker bind mount",
        ));
    }
    #[cfg(unix)]
    let user = {
        use std::os::unix::fs::MetadataExt;
        let m = candidate.metadata()?;
        format!("{}:{}", m.uid(), m.gid())
    };
    #[cfg(not(unix))]
    let user = "0:0".to_owned();
    let owner_dir = tempfile::Builder::new().prefix("myr-owner-").tempdir()?;
    let owner = owner_dir
        .path()
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| reject("invalid ownership token"))?
        .to_owned();
    let mut args = strings(&[
        "create",
        "--name",
        name,
        "--pull=never",
        "--network=none",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges=true",
        "--cgroupns=private",
        "--ipc=none",
        "--no-healthcheck",
        "--restart=no",
        "--log-driver=none",
        "--entrypoint=/usr/bin/env",
    ]);
    args.extend([
        format!("--label=myr.execution={owner}"),
        format!("--memory={}", sandbox.memory_bytes),
        format!("--memory-swap={}", sandbox.memory_bytes),
        format!("--pids-limit={}", sandbox.max_processes),
        format!("--user={user}"),
        format!("--mount=type=bind,src={input},dst=/input,readonly,bind-propagation=rprivate"),
        format!(
            "--tmpfs=/work:rw,nosuid,nodev,mode=1777,size={}",
            sandbox.memory_bytes
        ),
        format!(
            "--tmpfs=/tmp:rw,nosuid,nodev,noexec,mode=1777,size={}",
            sandbox.memory_bytes
        ),
        image.clone(),
        "-i".into(),
        "--".into(),
        "/bin/sh".into(),
        "-c".into(),
        BOOTSTRAP.into(),
        "myr-verifier".into(),
        if cwd == "." {
            "/work".into()
        } else {
            format!("/work/{cwd}")
        },
    ]);
    args.extend(environment.iter().map(|(k, v)| format!("{k}={v}")));
    args.extend(argv.iter().cloned());
    let image_index = args
        .iter()
        .position(|arg| arg == image)
        .expect("image argument");
    let command = args[image_index + 1..].to_vec();
    Ok(Plan {
        args,
        image: image.clone(),
        memory: sandbox.memory_bytes,
        pids: sandbox.max_processes,
        user,
        input,
        command,
        owner,
    })
}

fn check_container(value: &Value, plan: &Plan, id: &str) -> Result<()> {
    let v = one(value)?;
    let h = &v["HostConfig"];
    if v["Id"] != id
        || v["Image"] != plan.image
        || v["Config"]["User"] != plan.user
        || h["NetworkMode"] != "none"
        || h["ReadonlyRootfs"] != true
        || h["Privileged"] != false
        || h["Memory"] != plan.memory
        || h["MemorySwap"] != plan.memory
        || h["PidsLimit"] != plan.pids
        || h["CapDrop"] != json!(["ALL"])
        || h["SecurityOpt"] != json!(["no-new-privileges=true"])
        || h["CgroupnsMode"] != "private"
        || h["IpcMode"] != "none"
        || h["PidMode"] != ""
        || h["LogConfig"]["Type"] != "none"
        || h["RestartPolicy"]["Name"] != "no"
        || v["Config"]["Entrypoint"] != json!(["/usr/bin/env"])
        || v["Config"]["Cmd"] != json!(plan.command)
        || v["Config"]["Labels"]["myr.execution"] != plan.owner
        || v["Config"]["Healthcheck"]["Test"] != json!(["NONE"])
        || ![
            "CapAdd",
            "Devices",
            "DeviceRequests",
            "VolumesFrom",
            "Binds",
        ]
        .iter()
        .all(|key| h[*key].is_null() || h[*key].as_array().is_some_and(Vec::is_empty))
        || h["UTSMode"] != ""
    {
        return Err(reject("created container does not match isolation policy"));
    }
    let mounts = v["Mounts"]
        .as_array()
        .ok_or_else(|| reject("missing mount inspection"))?;
    if mounts.len() != 1
        || mounts[0]["Type"] != "bind"
        || mounts[0]["Destination"] != "/input"
        || mounts[0]["RW"] != false
        || mounts[0]["Propagation"] != "rprivate"
    {
        return Err(reject("unexpected container mount"));
    }
    // Windows Docker Desktop may translate the bind source to its VM path.
    let requested = h["Mounts"]
        .as_array()
        .ok_or_else(|| reject("missing requested mounts"))?;
    if requested.len() != 1
        || requested[0]["Source"] != plan.input
        || requested[0]["Target"] != "/input"
        || requested[0]["ReadOnly"] != true
        || requested[0]["Type"] != "bind"
    {
        return Err(reject("candidate mount source differs"));
    }
    let tmpfs = h["Tmpfs"]
        .as_object()
        .ok_or_else(|| reject("missing tmpfs limits"))?;
    if tmpfs.len() != 2
        || tmpfs.get("/work")
            != Some(&json!(format!(
                "rw,nosuid,nodev,mode=1777,size={}",
                plan.memory
            )))
        || tmpfs.get("/tmp")
            != Some(&json!(format!(
                "rw,nosuid,nodev,noexec,mode=1777,size={}",
                plan.memory
            )))
    {
        return Err(reject("unexpected writable tmpfs"));
    }
    Ok(())
}

/// Execute only a sealed policy on a recorded candidate. Cleanup is required for
/// success; timeout and infrastructure failures never masquerade as exit zero.
pub fn execute(
    graph: &Graph,
    candidate: &RecordedCandidate,
    policy_ref: ObjectRef,
    runtime: &DockerRuntime,
) -> Result<Execution> {
    execute_bounded(graph, candidate, policy_ref, runtime, Duration::MAX)
}

pub fn execute_bounded(
    graph: &Graph,
    candidate: &RecordedCandidate,
    policy_ref: ObjectRef,
    runtime: &DockerRuntime,
    remaining_mission: Duration,
) -> Result<Execution> {
    let start = Instant::now();
    if !runtime.executable.is_absolute() {
        return Err(reject("absolute Docker executable required"));
    }
    let manifest = candidate::load_manifest(graph, candidate.reference())?;
    let sealed = goal::load(graph, manifest.scope.goal)?;
    if !sealed.policy().verifier_policies.contains(&policy_ref) {
        return Err(reject("verifier policy is not sealed"));
    }
    let policy = verifier::load_policy(graph, policy_ref, sealed.policy().time_budget_ms)?;
    let VerifierPolicy::Command {
        timeout_ms,
        max_output_bytes,
        ..
    } = &policy
    else {
        return Err(reject("command policy required"));
    };
    let config = tempfile::Builder::new().prefix("myr-docker-").tempdir()?;
    let name = config
        .path()
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| reject("invalid container name"))?;
    let plan = plan(&policy, candidate.files().path(), name)?;
    let tree = snapshot::read(
        candidate.files().path(),
        &snapshot::ReadPolicy {
            excluded: vec![],
            ..Default::default()
        },
    )?;
    if crate::views::hashes(&tree) != *candidate.files().input_hashes() {
        return Err(reject("candidate files changed after preparation"));
    }
    let deadline = start
        .checked_add(Duration::from_millis(*timeout_ms).min(remaining_mission))
        .ok_or_else(|| reject("deadline overflow"))?;
    let cli = Cli {
        runtime,
        config: config.path(),
    };
    let daemon = metadata(cli.call(
        &strings(&["info", "--format", "{{json .}}"]),
        remaining(deadline)?,
        1024 * 1024,
    )?)?;
    check_daemon(&daemon)?;
    let image = metadata(cli.call(
        &strings(&["image", "inspect", &plan.image]),
        remaining(deadline)?,
        1024 * 1024,
    )?)?;
    check_image(&image, &plan.image)?;
    let (output, after) = run_container(
        &plan,
        name,
        deadline,
        *max_output_bytes as usize,
        &mut |args, timeout, cap| cli.call(args, timeout, cap),
    )?;
    candidate::load_manifest(graph, candidate.reference())?;
    Ok(Execution {
        candidate: candidate.reference(),
        policy: policy_ref,
        exit_code: i64::from(output.exit_code.expect("validated normal exit")),
        stdout: output.stdout,
        stderr: output.stderr,
        image_metadata: image,
        daemon_metadata: daemon,
        container_metadata: after,
        create_argv: plan.args,
    })
}

fn run_container(
    plan: &Plan,
    name: &str,
    deadline: Instant,
    cap: usize,
    call: &mut impl FnMut(&[String], Duration, usize) -> Result<Output>,
) -> Result<(Output, Value)> {
    // From this point, even a failed create may have reached the daemon.
    let result = (|| {
        let created = call(&plan.args, remaining(deadline)?, 1024 * 1024)?;
        if !created.success {
            return Err(reject("container creation failed"));
        }
        let id = std::str::from_utf8(&created.stdout)
            .map_err(|_| reject("invalid container ID"))?
            .trim();
        if id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(reject("invalid container ID"));
        }
        let before = metadata(call(
            &strings(&["container", "inspect", id]),
            remaining(deadline)?,
            1024 * 1024,
        )?)?;
        check_container(&before, plan, id)?;
        if one(&before)?["State"]["Status"] != "created" {
            return Err(reject("container already started"));
        }
        let output = call(
            &strings(&["start", "--attach", id]),
            remaining(deadline)?,
            cap,
        )?;
        let after = metadata(call(
            &strings(&["container", "inspect", id]),
            remaining(deadline)?,
            1024 * 1024,
        )?)?;
        check_container(&after, plan, id)?;
        let state = &one(&after)?["State"];
        if state["Status"] != "exited"
            || state["Running"] != false
            || state["OOMKilled"] != false
            || state["Error"] != ""
        {
            return Err(reject("container did not finish normally"));
        }
        let exit_code = state["ExitCode"]
            .as_i64()
            .ok_or_else(|| reject("missing container exit code"))?;
        if output.exit_code.map(i64::from) != Some(exit_code) {
            return Err(reject("attached CLI and container exit codes differ"));
        }
        if (125..=127).contains(&exit_code) {
            return Err(reject("bootstrap or executable startup failure"));
        }
        Ok((output, after))
    })();
    // Emergency cleanup has a separate bounded allowance after the work deadline.
    let cleanup = (|| {
        let cleanup_deadline = Instant::now() + Duration::from_secs(10);
        let inspected = metadata(call(
            &strings(&["container", "inspect", name]),
            remaining(cleanup_deadline)?,
            1024 * 1024,
        )?)?;
        let value = one(&inspected)?;
        if value["Config"]["Labels"]["myr.execution"] != plan.owner {
            return Err(reject(
                "refusing cleanup of a container owned by another execution",
            ));
        }
        let id = value["Id"]
            .as_str()
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or_else(|| reject("invalid cleanup container ID"))?;
        call(
            &strings(&["rm", "--force", "--volumes", id]),
            remaining(cleanup_deadline)?,
            1024 * 1024,
        )
    })();
    if !cleanup.is_ok_and(|output| output.success) {
        return Err(Error::Cleanup {
            container: name.into(),
        });
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{candidate::Candidate, verifier::SandboxPolicy, views::Tree};
    use std::collections::BTreeMap;

    fn policy() -> VerifierPolicy {
        VerifierPolicy::Command {
            argv: vec![
                "/usr/bin/check".into(),
                "literal; $(no shell expansion)".into(),
            ],
            cwd: ".".into(),
            environment: BTreeMap::from([("ONLY".into(), "this value".into())]),
            tool_versions: BTreeMap::from([("/usr/bin/check".into(), "fixture".into())]),
            timeout_ms: 1000,
            max_output_bytes: 4096,
            sandbox: SandboxPolicy {
                backend: SandboxBackend::DockerLinux {
                    image: format!("sha256:{}", "a".repeat(64)),
                },
                network: false,
                memory_bytes: 64 * 1024 * 1024,
                max_processes: 16,
            },
        }
    }
    fn inspected(p: &Plan, id: &str) -> Value {
        json!([{
            "Id": id, "Image": p.image,
            "Config": { "User": p.user, "Entrypoint": ["/usr/bin/env"], "Cmd": p.command,
                "Labels": { "myr.execution": p.owner }, "Healthcheck": { "Test": ["NONE"] } },
            "HostConfig": {
                "NetworkMode": "none", "ReadonlyRootfs": true, "Privileged": false,
                "Memory": p.memory, "MemorySwap": p.memory, "PidsLimit": p.pids,
                "CapDrop": ["ALL"], "SecurityOpt": ["no-new-privileges=true"],
                "CgroupnsMode": "private", "IpcMode": "none", "PidMode": "", "UTSMode": "",
                "LogConfig": { "Type": "none" }, "RestartPolicy": { "Name": "no" },
                "Mounts": [{ "Type": "bind", "Source": p.input, "Target": "/input", "ReadOnly": true }],
                "Tmpfs": {
                    "/work": format!("rw,nosuid,nodev,mode=1777,size={}", p.memory),
                    "/tmp": format!("rw,nosuid,nodev,noexec,mode=1777,size={}", p.memory)
                }
            },
            "Mounts": [{ "Type": "bind", "Destination": "/input", "RW": false, "Propagation": "rprivate" }],
            "State": { "Status": "created", "Running": false, "OOMKilled": false, "Error": "", "ExitCode": 0 }
        }])
    }
    fn output(stdout: Vec<u8>, code: i32) -> Output {
        Output {
            success: code == 0,
            exit_code: Some(code),
            stdout,
            stderr: vec![],
        }
    }

    #[test]
    fn explicit_pinned_policy_and_literal_argv_are_required() {
        let candidate = Candidate::from_tree(&Tree::new()).unwrap();
        let p = plan(&policy(), candidate.path(), "myr-fixture").unwrap();
        assert!(p.args.contains(&"--pull=never".into()));
        assert!(p.args.contains(&"--network=none".into()));
        assert!(p.args.contains(&"--read-only".into()));
        assert_eq!(p.command.last().unwrap(), "literal; $(no shell expansion)");
        assert!(p.command.contains(&"ONLY=this value".into()));
        assert!(!BOOTSTRAP.contains("eval"));
        for image in ["latest", "alpine:latest", "sha256:short"] {
            let mut bad = policy();
            let VerifierPolicy::Command { sandbox, .. } = &mut bad else {
                unreachable!()
            };
            sandbox.backend = SandboxBackend::DockerLinux {
                image: image.into(),
            };
            assert!(bad.validate().is_err());
        }
        let mut other = policy();
        let VerifierPolicy::Command { sandbox, .. } = &mut other else {
            unreachable!()
        };
        sandbox.backend = SandboxBackend::WindowsAppContainer;
        assert!(plan(&other, candidate.path(), "myr-fixture").is_err());
    }

    #[test]
    fn inspected_isolation_changes_are_rejected_before_start() {
        let candidate = Candidate::from_tree(&Tree::new()).unwrap();
        let p = plan(&policy(), candidate.path(), "myr-fixture").unwrap();
        let good = inspected(&p, "id");
        check_container(&good, &p, "id").unwrap();
        for (pointer, replacement) in [
            ("/0/HostConfig/NetworkMode", json!("host")),
            ("/0/HostConfig/ReadonlyRootfs", json!(false)),
            ("/0/HostConfig/Privileged", json!(true)),
            ("/0/HostConfig/Memory", json!(0)),
            ("/0/HostConfig/MemorySwap", json!(-1)),
            ("/0/HostConfig/PidsLimit", json!(0)),
            ("/0/HostConfig/PidMode", json!("host")),
            ("/0/HostConfig/CapAdd", json!(["SYS_ADMIN"])),
            ("/0/Config/Cmd", json!(["unexpected"])),
            ("/0/Mounts/0/RW", json!(true)),
            ("/0/HostConfig/Mounts/0/Source", json!("/host")),
            ("/0/HostConfig/Tmpfs", json!({})),
        ] {
            let mut bad = good.clone();
            if pointer.ends_with("CapAdd") {
                bad[0]["HostConfig"]["CapAdd"] = replacement;
            } else {
                *bad.pointer_mut(pointer).unwrap() = replacement;
            }
            assert!(check_container(&bad, &p, "id").is_err(), "{pointer}");
        }
        assert!(check_daemon(&json!({"OSType":"linux"})).is_err());
        let daemon = json!({"OSType":"linux", "MemoryLimit":true, "SwapLimit":true, "PidsLimit":true,
            "CgroupDriver":"systemd", "SecurityOptions":["name=seccomp,profile=builtin"]});
        check_daemon(&daemon).unwrap();
        let mut image = json!([{"Id":p.image,"Os":"linux","Config":{"Volumes":null}}]);
        check_image(&image, &p.image).unwrap();
        image[0]["Config"]["Volumes"] = json!({"/unexpected":{}});
        assert!(check_image(&image, &p.image).is_err());
    }

    #[test]
    fn lifecycle_cleans_after_success_timeout_rejection_and_cleanup_failure() {
        let candidate = Candidate::from_tree(&Tree::new()).unwrap();
        let p = plan(&policy(), candidate.path(), "myr-fixture").unwrap();
        let id = "b".repeat(64);
        for scenario in [
            "success",
            "command_failure",
            "timeout",
            "bad_config",
            "cleanup_failure",
            "oom",
            "foreign",
        ] {
            let mut calls = vec![];
            let mut inspections = 0;
            let result = run_container(
                &p,
                "myr-fixture",
                Instant::now() + Duration::from_secs(5),
                4096,
                &mut |args, _timeout, cap| {
                    calls.push(args[0].clone());
                    match args[0].as_str() {
                        "create" => Ok(output(id.as_bytes().to_vec(), 0)),
                        "container" => {
                            inspections += 1;
                            let mut value = inspected(&p, &id);
                            if scenario == "foreign" {
                                value[0]["Config"]["Labels"]["myr.execution"] =
                                    json!("another execution");
                            }
                            if scenario == "bad_config" {
                                value[0]["HostConfig"]["Privileged"] = json!(true);
                            }
                            if inspections == 2 {
                                value[0]["State"]["Status"] = json!("exited");
                                value[0]["State"]["ExitCode"] =
                                    json!(if scenario == "command_failure" { 7 } else { 0 });
                                if scenario == "oom" {
                                    value[0]["State"]["OOMKilled"] = json!(true);
                                }
                            }
                            Ok(output(serde_json::to_vec(&value).unwrap(), 0))
                        }
                        "start" => {
                            assert_eq!(cap, 4096);
                            if scenario == "timeout" {
                                Err(process::Error::Timeout.into())
                            } else {
                                Ok(output(
                                    b"verifier bytes".to_vec(),
                                    if scenario == "command_failure" { 7 } else { 0 },
                                ))
                            }
                        }
                        "rm" => Ok(output(
                            vec![],
                            if scenario == "cleanup_failure" { 1 } else { 0 },
                        )),
                        _ => panic!("unexpected command"),
                    }
                },
            );
            if scenario == "foreign" {
                assert!(!calls.contains(&"rm".into()));
                assert!(!calls.contains(&"start".into()));
            } else {
                assert_eq!(calls.last().unwrap(), "rm");
            }
            match scenario {
                "success" | "command_failure" => {
                    let (out, _) = result.unwrap();
                    assert_eq!(
                        out.exit_code,
                        Some(if scenario == "command_failure" { 7 } else { 0 })
                    );
                }
                "cleanup_failure" | "foreign" => {
                    assert!(matches!(result, Err(Error::Cleanup { .. })))
                }
                "bad_config" => {
                    assert!(result.is_err());
                    assert!(!calls.contains(&"start".into()));
                }
                _ => assert!(result.is_err()),
            }
        }
    }
}
