//! Sealed verifier configuration. Validation does not execute a command or
//! certify that an OS sandbox is installed; execution must prove confinement.
use crate::Result;
use myr_core::*;
use myr_graph::Graph;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum SandboxBackend {
    LinuxBubblewrap,
    WindowsAppContainer,
    DockerLinux { image: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxPolicy {
    pub backend: SandboxBackend,
    /// No networking or host filesystem write access is permitted in v0.
    pub network: bool,
    pub memory_bytes: u64,
    pub max_processes: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum VerifierPolicy {
    Command {
        argv: Vec<String>,
        cwd: String,
        /// Exact environment; executors must clear inherited variables.
        environment: BTreeMap<String, String>,
        tool_versions: BTreeMap<String, String>,
        timeout_ms: u64,
        max_output_bytes: u64,
        sandbox: SandboxPolicy,
    },
    LlmReview {
        procedure: ObjectRef,
        lineage: Lineage,
    },
}

impl VerifierPolicy {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Command {
                argv,
                cwd,
                environment,
                tool_versions,
                timeout_ms,
                max_output_bytes,
                sandbox,
            } => {
                if argv.is_empty()
                    || argv[0].trim().is_empty()
                    || argv.iter().any(|a| a.contains('\0'))
                    || *timeout_ms == 0
                    || *max_output_bytes == 0
                    || *max_output_bytes > 64 * 1024 * 1024
                    || sandbox.network
                    || sandbox.memory_bytes == 0
                    || sandbox.max_processes == 0
                {
                    return Err(invalid("invalid command limits, argv, or sandbox policy").into());
                }
                if cwd != "." {
                    validate_relative_path(cwd)?;
                }
                if let SandboxBackend::DockerLinux { image } = &sandbox.backend {
                    let digest = image.strip_prefix("sha256:").unwrap_or("");
                    if digest.len() != 64
                        || !digest
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                        || argv[0].contains('=')
                    {
                        return Err(invalid(
                            "Docker requires a local sha256 image ID and executable without '='",
                        )
                        .into());
                    }
                }
                let mut names = BTreeSet::new();
                for (name, value) in environment {
                    if name.is_empty()
                        || name.contains(['=', '\0'])
                        || value.contains('\0')
                        || !names.insert(name.to_ascii_uppercase())
                    {
                        return Err(
                            invalid("invalid or case-colliding environment variables").into()
                        );
                    }
                }
                if !tool_versions.contains_key(&argv[0])
                    || tool_versions.iter().any(|(k, v)| {
                        k.trim().is_empty()
                            || v.trim().is_empty()
                            || k.contains('\0')
                            || v.contains('\0')
                            || v.trim().eq_ignore_ascii_case("unknown")
                    })
                {
                    return Err(invalid(
                        "verifier requires recorded versions including its executable",
                    )
                    .into());
                }
            }
            Self::LlmReview { procedure, lineage } => {
                procedure.require(Kind::Artifact)?;
                if !lineage.known() {
                    return Err(invalid("review policy requires complete known lineage").into());
                }
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(self)?)
    }
}

pub fn load_policy(
    graph: &Graph,
    reference: ObjectRef,
    mission_timeout_ms: u64,
) -> Result<VerifierPolicy> {
    reference.require(Kind::Artifact)?;
    if !graph.live(reference)? {
        return Err(invalid("verifier policy is inactive").into());
    }
    let bytes = graph.cas().get(reference)?;
    let policy: VerifierPolicy = serde_json::from_slice(&bytes)?;
    if policy.encode()? != bytes {
        return Err(invalid("verifier policy encoding is not canonical").into());
    }
    match &policy {
        VerifierPolicy::Command { timeout_ms, .. } if *timeout_ms > mission_timeout_ms => {
            return Err(invalid("verifier timeout exceeds mission deadline").into());
        }
        VerifierPolicy::LlmReview { procedure, .. } if !graph.live(*procedure)? => {
            return Err(invalid("review procedure is inactive").into());
        }
        _ => {}
    }
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn command() -> VerifierPolicy {
        VerifierPolicy::Command {
            argv: vec!["cargo".into(), "test".into()],
            cwd: ".".into(),
            environment: BTreeMap::new(),
            tool_versions: BTreeMap::from([("cargo".into(), "1.95.0".into())]),
            timeout_ms: 1000,
            max_output_bytes: 4096,
            sandbox: SandboxPolicy {
                backend: SandboxBackend::LinuxBubblewrap,
                network: false,
                memory_bytes: 1024 * 1024 * 1024,
                max_processes: 32,
            },
        }
    }
    #[test]
    fn invalid_paths_environment_versions_and_sandbox_settings_rejected() {
        assert!(command().validate().is_ok());
        for case in 0..6 {
            let mut p = command();
            let VerifierPolicy::Command {
                argv,
                cwd,
                environment,
                tool_versions,
                sandbox,
                timeout_ms,
                ..
            } = &mut p
            else {
                unreachable!()
            };
            match case {
                0 => *cwd = "../outside".into(),
                1 => {
                    environment.insert("Path".into(), "a".into());
                    environment.insert("PATH".into(), "b".into());
                }
                2 => tool_versions.clear(),
                3 => sandbox.network = true,
                4 => *timeout_ms = 0,
                _ => argv[0].push('\0'),
            }
            assert!(p.validate().is_err());
        }
    }
    #[test]
    fn stored_policy_requires_canonical_bytes_and_mission_bounded_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let mut graph = Graph::open(
            dir.path().join("graph.sqlite"),
            myr_cas::Store::open(dir.path().join("cas")).unwrap(),
        )
        .unwrap();
        let mut bytes = command().encode().unwrap();
        let r = graph.register_artifact(&bytes).unwrap();
        assert!(load_policy(&graph, r, 1000).is_ok());
        assert!(load_policy(&graph, r, 999).is_err());
        bytes.push(b'\n');
        let r = graph.register_artifact(&bytes).unwrap();
        assert!(load_policy(&graph, r, 1000).is_err());
    }
}
