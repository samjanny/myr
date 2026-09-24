//! The common untrusted-output boundary for CLI and API transports.
pub mod claude_code;
pub mod codex_cli;
pub mod config;
pub mod process;
pub mod schema;
pub mod transport;

use base64::{Engine, engine::general_purpose::STANDARD};
use myr_core::*;
use myr_graph::{Authority, Graph};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, io::Write, path::PathBuf};

pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
pub const MAX_ARTIFACT_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Planner,
    Worker,
    Reviewer,
}

#[derive(Clone, Debug)]
pub struct Identity {
    pub agent: String,
    pub lineage: Lineage,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub action: Action,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(
    tag = "tool",
    content = "arguments",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Action {
    Fetch {
        reference: ObjectRef,
    },
    PutArtifact {
        content_base64: String,
    },
    EmitClaim {
        predicate_ref: ObjectRef,
        arguments: Vec<Argument>,
        polarity: bool,
    },
    EmitAssumption {
        question: String,
        chosen: String,
        alternatives: Vec<String>,
        rationale_ref: ObjectRef,
        artifacts: Vec<ObjectRef>,
    },
    EmitDelta {
        path: String,
        base_ref: ObjectRef,
        patch_ref: ObjectRef,
        result_ref: ObjectRef,
        codec: DeltaCodec,
        assumptions: Vec<ObjectRef>,
    },
    EmitFail {
        code: FailCode,
        diagnostic_ref: ObjectRef,
    },
    ReviewClaim {
        claim_ref: ObjectRef,
        verdict: Verdict,
        rationale_ref: ObjectRef,
    },
    Finish {},
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Applied {
        /// Logical bytes returned by a successful fetch, before rendering.
        /// Runtime metadata, never supplied by the model or sent as a receipt.
        #[serde(skip_serializing)]
        fetched_raw_bytes: Option<u64>,
        objects: Vec<ObjectRef>,
        result: serde_json::Value,
        done: bool,
    },
    Repair {
        attempt: u8,
        message: String,
    },
    Failed {
        failure: ObjectRef,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("adapter graph: {0}")]
    Graph(#[from] myr_graph::Error),
    #[error("adapter I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("adapter JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("adapter validation: {0}")]
    Validation(#[from] ValidationError),
    #[error("adapter session has already ended")]
    Ended,
}

enum Rejection {
    Invalid(String),
    Policy(String),
    Infrastructure(Error),
}
impl From<ValidationError> for Rejection {
    fn from(e: ValidationError) -> Self {
        Self::Invalid(e.to_string())
    }
}
impl From<myr_graph::Error> for Rejection {
    fn from(e: myr_graph::Error) -> Self {
        match e {
            myr_graph::Error::Validation(_) | myr_graph::Error::Unavailable(_) => {
                Self::Invalid(e.to_string())
            }
            myr_graph::Error::Authority => Self::Policy(e.to_string()),
            e => Self::Infrastructure(e.into()),
        }
    }
}

/// Runtime-owned session context. Model JSON cannot choose its task, scope,
/// identity, lineage, baseline mapping, protected paths, or CAS access roots.
pub struct Session {
    task_ref: ObjectRef,
    task: Task,
    role: Role,
    identity: Identity,
    roots: Vec<ObjectRef>,
    baseline: BTreeMap<String, ObjectRef>,
    protected: Vec<String>,
    quarantine: PathBuf,
    repair_errors: u8,
    ended: bool,
    review_context: Option<ObjectRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewRationale {
    pub format: String,
    pub context: ObjectRef,
    pub task: ObjectRef,
    pub claim: ObjectRef,
    pub verdict: Verdict,
    pub rationale: ObjectRef,
}

pub struct SessionConfig {
    pub task: ObjectRef,
    pub role: Role,
    pub identity: Identity,
    pub baseline: BTreeMap<String, ObjectRef>,
    pub protected: Vec<String>,
    pub quarantine: PathBuf,
}

impl Session {
    pub fn new(graph: &Graph, config: SessionConfig) -> Result<Self, Error> {
        config.task.require(Kind::Task)?;
        graph.fetch(&[config.task], config.task)?;
        let Object::Task(task) = graph.get(config.task)? else {
            return Err(invalid("expected task").into());
        };
        if config.identity.agent.is_empty() {
            return Err(invalid("runtime identity must name an agent").into());
        }
        for (path, reference) in &config.baseline {
            validate_relative_path(path)?;
            reference.require(Kind::Artifact)?;
            graph.fetch(&[config.task], *reference)?;
        }
        for prefix in &config.protected {
            validate_relative_path(prefix.trim_end_matches('/'))?;
        }
        for capability in &task.capabilities {
            let (operation, path) = capability
                .split_once(':')
                .ok_or_else(|| invalid("invalid task capability"))?;
            if !["read", "write"].contains(&operation) {
                return Err(invalid("unknown capability operation").into());
            }
            validate_relative_path(path.trim_end_matches('/'))?;
        }
        Ok(Self {
            task_ref: config.task,
            task,
            role: config.role,
            identity: config.identity,
            roots: vec![config.task],
            baseline: config.baseline,
            protected: config.protected,
            quarantine: config.quarantine,
            repair_errors: 0,
            ended: false,
            review_context: None,
        })
    }

    /// The runner validates the typed candidate context before binding it.
    /// Model actions cannot call this method or alter the selected context.
    pub fn bind_review_context(&mut self, graph: &Graph, context: ObjectRef) -> Result<(), Error> {
        if self.role != Role::Reviewer || self.review_context.is_some() || self.ended {
            return Err(invalid("review context cannot be bound in this session").into());
        }
        context.require(Kind::Artifact)?;
        graph.fetch(&[self.task_ref], context)?;
        self.review_context = Some(context);
        Ok(())
    }

    pub fn handle(&mut self, graph: &mut Graph, raw: &[u8]) -> Result<Outcome, Error> {
        if self.ended {
            return Err(Error::Ended);
        }
        let parsed = if raw.len() > MAX_RESPONSE_BYTES {
            Err(Rejection::Invalid("response exceeds size limit".into()))
        } else {
            serde_json::from_slice::<Response>(raw)
                .map_err(|e| Rejection::Invalid(format!("JSON schema: {e}")))
        };
        let result = parsed.and_then(|response| self.apply(graph, response.action));
        match result {
            Ok(outcome) => {
                self.repair_errors = 0;
                Ok(outcome)
            }
            Err(Rejection::Infrastructure(e)) => Err(e),
            Err(rejection) => {
                self.quarantine(raw)?;
                let (message, policy) = match rejection {
                    Rejection::Invalid(m) => (m, false),
                    Rejection::Policy(m) => (m, true),
                    Rejection::Infrastructure(_) => unreachable!(),
                };
                self.repair_errors += 1;
                if !policy && self.repair_errors <= 2 {
                    Ok(Outcome::Repair {
                        attempt: self.repair_errors,
                        message: message.chars().take(320).collect(),
                    })
                } else {
                    self.ended = true;
                    self.fail(
                        graph,
                        if policy {
                            FailCode::CapabilityDenied
                        } else {
                            FailCode::InvalidAgentOutput
                        },
                        message.as_bytes(),
                    )
                }
            }
        }
    }

    fn quarantine(&self, bytes: &[u8]) -> Result<(), Error> {
        fs::create_dir_all(&self.quarantine)?;
        let path = self
            .quarantine
            .join(hex::encode(myr_wire::artifact_cid(bytes).0));
        if path.exists() {
            return Ok(());
        }
        let mut temp = tempfile::NamedTempFile::new_in(&self.quarantine)?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(path) {
            Ok(_) => Ok(()),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e.error.into()),
        }
    }

    fn fail(&self, graph: &mut Graph, code: FailCode, diagnostic: &[u8]) -> Result<Outcome, Error> {
        let diagnostic = graph.register_artifact(diagnostic)?;
        let failure = graph.insert(
            Authority::Runtime,
            &Object::Fail(Fail {
                class: code.class(),
                code,
                diagnostic,
                task: Some(self.task_ref),
            }),
        )?;
        Ok(Outcome::Failed { failure })
    }

    fn accessible(&self, graph: &Graph, r: ObjectRef) -> Result<Vec<u8>, Rejection> {
        Ok(graph.fetch(&self.roots, r)?)
    }

    fn apply(&mut self, graph: &mut Graph, action: Action) -> Result<Outcome, Rejection> {
        graph.fetch(&[self.task_ref], self.task_ref)?;
        match (&action, self.role) {
            (Action::ReviewClaim { .. }, Role::Planner | Role::Worker)
            | (
                Action::EmitClaim { .. }
                | Action::EmitAssumption { .. }
                | Action::EmitDelta { .. }
                | Action::EmitFail { .. },
                Role::Reviewer,
            ) => {
                return Err(Rejection::Policy(
                    "tool is not available to this role".into(),
                ));
            }
            _ => {}
        }
        let mut objects = Vec::new();
        let mut result = serde_json::json!({});
        let mut done = false;
        let mut fetched_raw_bytes = None;
        match action {
            Action::Fetch { reference } => {
                let bytes = self.accessible(graph, reference)?;
                fetched_raw_bytes = Some(bytes.len() as u64);
                result = if matches!(reference.kind, Kind::Artifact | Kind::Goal) {
                    serde_json::json!({"reference":reference,"content_base64":STANDARD.encode(bytes)})
                } else {
                    let object = myr_wire::decode(&bytes)?;
                    serde_json::json!({"reference":reference,"object":object})
                };
            }
            Action::PutArtifact { content_base64 } => {
                let bytes = STANDARD
                    .decode(&content_base64)
                    .map_err(|_| invalid("content_base64 must use canonical Base64"))?;
                if bytes.len() > MAX_ARTIFACT_BYTES || STANDARD.encode(&bytes) != content_base64 {
                    return Err(
                        invalid("artifact exceeds size limit or has noncanonical Base64").into(),
                    );
                }
                objects.push(graph.register_artifact(&bytes)?);
            }
            Action::EmitClaim {
                predicate_ref,
                arguments,
                polarity,
            } => {
                predicate_ref.require(Kind::PredicateDef)?;
                self.accessible(graph, predicate_ref)?;
                for arg in &arguments {
                    if let Argument::Ref(r) = arg {
                        self.accessible(graph, *r)?;
                    }
                }
                let atom = Object::Atom(Atom {
                    predicate: predicate_ref,
                    arguments,
                });
                let atom_ref = myr_wire::identify(&atom)?.0;
                let claim = Object::Claim(Claim {
                    atom: atom_ref,
                    polarity,
                    scope: self.task.scope.clone(),
                });
                let claim_ref = myr_wire::identify(&claim)?.0;
                let attest = Object::Attest(Attest {
                    claim: claim_ref,
                    task: self.task_ref,
                    agent: self.identity.agent.clone(),
                    lineage: self.identity.lineage.clone(),
                });
                objects = graph.insert_batch(&[
                    (Authority::Worker, atom),
                    (Authority::Worker, claim),
                    (Authority::Runtime, attest),
                ])?;
                result = serde_json::json!({"claim_ref":claim_ref});
            }
            Action::EmitAssumption {
                question,
                chosen,
                alternatives,
                rationale_ref,
                artifacts,
            } => {
                rationale_ref.require(Kind::Artifact)?;
                self.accessible(graph, rationale_ref)?;
                for r in &artifacts {
                    r.require(Kind::Artifact)?;
                    self.accessible(graph, *r)?;
                }
                objects.push(graph.insert(
                    Authority::Worker,
                    &Object::Assumption(Assumption {
                        scope: self.task.scope.clone(),
                        question,
                        chosen,
                        alternatives,
                        rationale: rationale_ref,
                        artifacts,
                        invalidates: None,
                    }),
                )?);
            }
            Action::EmitDelta {
                path,
                base_ref,
                patch_ref,
                result_ref,
                codec,
                assumptions,
            } => {
                base_ref.require(Kind::Artifact)?;
                patch_ref.require(Kind::Artifact)?;
                result_ref.require(Kind::Artifact)?;
                self.accessible(graph, base_ref)?;
                self.accessible(graph, patch_ref)?;
                self.accessible(graph, result_ref)?;
                if codec == DeltaCodec::Replacement && patch_ref != result_ref {
                    return Err(invalid("replacement patch must equal declared result").into());
                }
                for r in &assumptions {
                    r.require(Kind::Assumption)?;
                    self.accessible(graph, *r)?;
                }
                let delta = myr_wire::normalize(&Object::Delta(Delta {
                    scope: self.task.scope.clone(),
                    path,
                    base: base_ref,
                    patch: patch_ref,
                    result: result_ref,
                    codec,
                    assumptions,
                }))?;
                let Object::Delta(d) = &delta else {
                    unreachable!()
                };
                if self
                    .protected
                    .iter()
                    .any(|prefix| path_matches(prefix, &d.path))
                    || !self
                        .task
                        .capabilities
                        .iter()
                        .filter_map(|c| c.strip_prefix("write:"))
                        .any(|prefix| path_matches(prefix, &d.path))
                {
                    return Err(Rejection::Policy("write capability denied for path".into()));
                }
                if self.baseline.get(&d.path) != Some(&base_ref) {
                    return Err(invalid("delta base does not match sealed path mapping").into());
                }
                objects.push(graph.insert(Authority::Worker, &delta)?);
            }
            Action::EmitFail {
                code,
                diagnostic_ref,
            } => {
                diagnostic_ref.require(Kind::Artifact)?;
                self.accessible(graph, diagnostic_ref)?;
                let failure = graph.insert(
                    Authority::Worker,
                    &Object::Fail(Fail {
                        class: code.class(),
                        code,
                        diagnostic: diagnostic_ref,
                        task: Some(self.task_ref),
                    }),
                )?;
                self.ended = true;
                return Ok(Outcome::Failed { failure });
            }
            Action::ReviewClaim {
                claim_ref,
                verdict,
                rationale_ref,
            } => {
                claim_ref.require(Kind::Claim)?;
                rationale_ref.require(Kind::Artifact)?;
                self.accessible(graph, claim_ref)?;
                self.accessible(graph, rationale_ref)?;
                let Object::Claim(claim) = graph.get(claim_ref)? else {
                    return Err(invalid("expected claim").into());
                };
                if claim.scope != self.task.scope {
                    return Err(invalid("review claim scope differs from task").into());
                }
                let rationale_ref = if let Some(context) = self.review_context {
                    let wrapper = ReviewRationale {
                        format: "myr-review-rationale-v0".into(),
                        context,
                        task: self.task_ref,
                        claim: claim_ref,
                        verdict,
                        rationale: rationale_ref,
                    };
                    graph.register_artifact_with_dependencies(
                        &serde_json::to_vec(&wrapper)
                            .map_err(|e| Rejection::Infrastructure(e.into()))?,
                        &[context, self.task_ref, claim_ref, rationale_ref],
                    )?
                } else {
                    rationale_ref
                };
                objects.push(graph.insert(
                    Authority::Runtime,
                    &Object::Evidence(Box::new(Evidence {
                        claim: claim_ref,
                        scope: claim.scope,
                        verdict,
                        mechanism: Mechanism::LlmReview,
                        lineage: Some(self.identity.lineage.clone()),
                        command: None,
                        rationale: rationale_ref,
                        correlation_domains: vec![],
                    })),
                )?);
            }
            Action::Finish {} => {
                self.ended = true;
                done = true;
            }
        }
        self.roots.extend(objects.iter().copied());
        Ok(Outcome::Applied {
            fetched_raw_bytes,
            objects,
            result,
            done,
        })
    }
}

fn path_matches(prefix: &str, path: &str) -> bool {
    // Portable policies are conservative on case-insensitive filesystems too.
    let prefix = prefix.to_lowercase();
    let path = path.to_lowercase();
    if prefix.ends_with('/') {
        path.starts_with(&prefix)
    } else {
        path == prefix
    }
}
