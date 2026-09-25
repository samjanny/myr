//! One budgeted transport dispatch. Completion bytes still require Session::handle.
use crate::{
    accounting::{self, ReferenceTokenizer},
    budget::{Budget, Limits},
    context::PreparedContext,
};
use myr_adapter::{
    config::ProviderConfig,
    transport::{self, Completion, Request, Usage},
};
use myr_core::{FailCode, ObjectRef};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use std::{
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("dispatch preparation: {0}")]
    Preparation(#[from] crate::Error),
    #[error("dispatch budget: {0:?}")]
    Budget(FailCode),
    #[error("dispatch transport: {0}")]
    Transport(#[from] transport::Error),
}

pub struct Call<'a> {
    pub agent: &'a str,
    pub provider: &'a ProviderConfig,
    pub context: &'a PreparedContext,
    pub schema: &'a Value,
    pub shared_schema: &'a Value,
    pub max_native_output_tokens: u32,
    pub max_reference_output_tokens: u64,
    pub timeout: Duration,
}

#[derive(Debug, Serialize)]
pub struct Attempt {
    pub agent: String,
    pub provider: ProviderConfig,
    pub context_artifacts: Vec<ObjectRef>,
    pub output: Option<ObjectRef>,
    pub provider_usage: Usage,
    pub observed_model: Option<String>,
    /// Failure cannot establish whether provider-side context injection occurred.
    pub transport_completed: bool,
    pub transport_error: Option<String>,
    pub budget_failure: Option<FailCode>,
}

#[derive(Serialize)]
struct PendingAttempt {
    agent: String,
    provider: ProviderConfig,
    context_artifacts: Vec<ObjectRef>,
    native_output_limit: u32,
    reference_output_allowance: u64,
    timeout_ms: u128,
}

pub struct Dispatcher {
    tokenizer: ReferenceTokenizer,
    budget: Budget,
    accounting: accounting::Ledger,
    audit: myr_cas::Store,
    attempts: Vec<Attempt>,
    pending: Option<PendingAttempt>,
    journal_dir: PathBuf,
    checkpoint: Option<ObjectRef>,
    checkpoint_sequence: u64,
    audit_failed: bool,
    /// First agent that authored each artifact in this mission (Appendix C.1).
    artifact_origins: BTreeMap<ObjectRef, String>,
    /// Mission-wide model-facing short references (step B2). A view only:
    /// journaled privately for audit, never written to shared state.
    aliases: myr_adapter::aliases::Aliases,
}

impl Dispatcher {
    /// Audit storage must be harness-private, separate from the shared agent CAS.
    pub fn new(
        limits: Limits,
        pipeline: accounting::Pipeline,
        audit: myr_cas::Store,
    ) -> Result<Self, Error> {
        let budget = Budget::new(limits).map_err(Error::Budget)?;
        let tokenizer = ReferenceTokenizer::new()?;
        let journal_dir = tempfile::Builder::new()
            .prefix("dispatch-journal-")
            .tempdir_in(audit.root())
            .map_err(crate::Error::from)?
            .keep();
        Ok(Self {
            tokenizer,
            budget,
            accounting: accounting::Ledger::new(pipeline),
            audit,
            attempts: vec![],
            pending: None,
            journal_dir,
            checkpoint: None,
            checkpoint_sequence: 0,
            audit_failed: false,
            artifact_origins: BTreeMap::new(),
            aliases: Default::default(),
        })
    }
    pub fn journal_directory(&self) -> &Path {
        &self.journal_dir
    }
    pub fn last_checkpoint(&self) -> Option<ObjectRef> {
        self.checkpoint
    }

    fn persist_checkpoint(&mut self) -> Result<(), Error> {
        let result = (|| -> crate::Result<ObjectRef> {
            let bytes = serde_json::to_vec(&serde_json::json!({
                "format":"myr-dispatch-journal-v0", "sequence":self.checkpoint_sequence,
                "previous":self.checkpoint, "pending":self.pending, "attempts":self.attempts,
                "budget":self.budget.ledger(), "accounting":self.accounting,
                "artifact_origins":self.artifact_origins.iter().collect::<Vec<_>>(),
                "aliases":self.aliases.table(),
            }))?;
            let reference = self.audit.put_artifact(&bytes)?;
            let mut pointer = tempfile::NamedTempFile::new_in(&self.journal_dir)?;
            pointer.write_all(&serde_json::to_vec(&reference)?)?;
            pointer.as_file().sync_all()?;
            pointer
                .persist_noclobber(
                    self.journal_dir
                        .join(format!("{:020}.json", self.checkpoint_sequence)),
                )
                .map_err(|e| e.error)?;
            Ok(reference)
        })();
        match result {
            Ok(reference) => {
                self.checkpoint = Some(reference);
                self.checkpoint_sequence += 1;
                Ok(())
            }
            Err(error) => {
                self.audit_failed = true;
                Err(error.into())
            }
        }
    }
    pub fn accounting(&self) -> &accounting::Ledger {
        &self.accounting
    }
    pub fn budget(&self) -> &crate::budget::Ledger {
        self.budget.ledger()
    }
    pub fn limits(&self) -> Limits {
        self.budget.limits()
    }
    pub fn deadline(&self) -> std::time::Instant {
        self.budget.deadline()
    }
    pub fn audit_root(&self) -> &Path {
        self.audit.root()
    }
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    pub fn aliases(&self) -> &myr_adapter::aliases::Aliases {
        &self.aliases
    }
    pub fn aliases_mut(&mut self) -> &mut myr_adapter::aliases::Aliases {
        &mut self.aliases
    }

    /// Record that `agent` authored an artifact. The first author is retained;
    /// identical bytes produced later by another agent keep that provenance.
    pub fn record_artifact_origin(&mut self, agent: &str, reference: ObjectRef) {
        if reference.kind == myr_core::Kind::Artifact {
            self.artifact_origins
                .entry(reference)
                .or_insert_with(|| agent.into());
        }
    }

    /// Classify artifact content by provenance, not by retrieval channel:
    /// initial repository bytes are DIRECT_REPO, the agent's own artifacts are
    /// LOCAL_TOOL, another agent's are INTER_AGENT_ARTIFACT. Any other artifact
    /// (runtime/protocol records) is `other`.
    pub fn classify_artifact(
        &self,
        agent: &str,
        reference: ObjectRef,
        repository: &BTreeSet<ObjectRef>,
        other: accounting::SegmentKind,
    ) -> accounting::SegmentKind {
        use accounting::SegmentKind;
        if repository.contains(&reference) {
            SegmentKind::DirectRepo
        } else {
            match self.artifact_origins.get(&reference) {
                Some(origin) if origin == agent => SegmentKind::LocalTool,
                Some(_) => SegmentKind::InterAgentArtifact,
                None => other,
            }
        }
    }

    /// Record logical agent CAS traffic separately from injected-context tokens.
    pub fn record_cas_read(&mut self, raw_bytes: u64) -> Result<(), Error> {
        self.accounting.record_cas_read(raw_bytes)?;
        self.persist_checkpoint()
    }

    pub fn complete(&mut self, call: Call<'_>) -> Result<Completion, Error> {
        self.complete_with(call, transport::complete)
    }

    pub(crate) fn preview_tokens(&self, call: &Call<'_>) -> Result<u64, Error> {
        let mut ledger = accounting::Ledger::new(self.accounting.pipeline());
        call.context.record_with_schema(
            &self.tokenizer,
            &mut ledger,
            &self.audit,
            call.agent,
            (call.schema, call.shared_schema),
        )?;
        Ok(ledger.totals().context_reference_tokens)
    }

    pub(crate) fn complete_with(
        &mut self,
        call: Call<'_>,
        send: impl FnOnce(&ProviderConfig, &Request) -> Result<Completion, transport::Error>,
    ) -> Result<Completion, Error> {
        if self.audit_failed {
            return Err(crate::Error::from(myr_core::invalid(
                "audit persistence failed; dispatch is disabled",
            ))
            .into());
        }
        call.provider.validate().map_err(crate::Error::from)?;
        if call.timeout.is_zero()
            || call.max_native_output_tokens == 0
            || call.max_reference_output_tokens == 0
        {
            return Err(
                crate::Error::from(myr_core::invalid("dispatch limits must be positive")).into(),
            );
        }
        let mut staged = accounting::Ledger::new(self.accounting.pipeline());
        let schemas = (call.schema, call.shared_schema);
        let refs = call.context.record_with_schema(
            &self.tokenizer,
            &mut staged,
            &self.audit,
            call.agent,
            schemas,
        )?;
        let permit = self
            .budget
            .reserve(
                staged.totals().context_reference_tokens,
                call.max_reference_output_tokens,
            )
            .map_err(Error::Budget)?;
        let request = Request {
            system: call.context.system().into(),
            prompt: call.context.prompt().into(),
            schema: call.schema.clone(),
            max_output_tokens: call.max_native_output_tokens,
            timeout: call
                .timeout
                .min(permit.timeout())
                .min(Duration::from_secs(3600)),
        };
        call.context.record_with_schema(
            &self.tokenizer,
            &mut self.accounting,
            &self.audit,
            call.agent,
            schemas,
        )?;
        self.pending = Some(PendingAttempt {
            agent: call.agent.into(),
            provider: call.provider.clone(),
            context_artifacts: refs.clone(),
            native_output_limit: request.max_output_tokens,
            reference_output_allowance: permit.output_reference_allowance(),
            timeout_ms: request.timeout.as_millis(),
        });
        self.persist_checkpoint()?;
        let result = send(call.provider, &request);
        match result {
            Ok(completion) => {
                let tokens = std::str::from_utf8(&completion.raw_output)
                    .ok()
                    .map(|s| self.tokenizer.count(s));
                let settlement = self.budget.settle(permit, tokens);
                let output = self
                    .audit
                    .put_artifact(&completion.raw_output)
                    .map_err(|error| {
                        self.audit_failed = true;
                        crate::Error::from(error)
                    })?;
                self.attempts.push(Attempt {
                    agent: call.agent.into(),
                    provider: call.provider.clone(),
                    context_artifacts: refs,
                    output: Some(output),
                    provider_usage: completion.usage.clone(),
                    observed_model: completion.observed_model.clone(),
                    transport_completed: true,
                    transport_error: None,
                    budget_failure: settlement.err(),
                });
                self.pending = None;
                self.persist_checkpoint()?;
                settlement.map_err(Error::Budget)?;
                Ok(completion)
            }
            Err(error) => {
                let settlement = self.budget.settle(permit, None);
                self.attempts.push(Attempt {
                    agent: call.agent.into(),
                    provider: call.provider.clone(),
                    context_artifacts: refs,
                    output: None,
                    provider_usage: Usage::default(),
                    observed_model: None,
                    transport_completed: false,
                    transport_error: Some(error.to_string()),
                    budget_failure: settlement.err(),
                });
                self.pending = None;
                self.persist_checkpoint()?;
                Err(Error::Transport(error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myr_adapter::{Role, config::Backend, schema::response_schema};
    fn config() -> ProviderConfig {
        ProviderConfig {
            backend: Backend::ClaudeCode {
                executable: "never-spawned".into(),
            },
            model: "fixture".into(),
            lineage: myr_core::Lineage {
                provider_id: "anthropic".into(),
                family_id: "fixture".into(),
                checkpoint_id: "v0".into(),
                procedure_id: "test".into(),
            },
        }
    }
    #[test]
    fn budget_context_and_telemetry_are_connected_without_implicit_retry() {
        let dir = tempfile::tempdir().unwrap();
        let audit = myr_cas::Store::open(dir.path()).unwrap();
        let mut d = Dispatcher::new(
            Limits {
                calls: 1,
                reference_tokens: 10000,
                elapsed: Duration::from_secs(30),
            },
            accounting::Pipeline::Mw0,
            audit.clone(),
        )
        .unwrap();
        let context = PreparedContext::new("system", &[]).unwrap();
        let config = config();
        let schema = response_schema(Role::Worker);
        let call = || Call {
            agent: "worker",
            provider: &config,
            context: &context,
            schema: &schema,
            shared_schema: &schema,
            max_native_output_tokens: 100,
            max_reference_output_tokens: 100,
            timeout: Duration::from_secs(60),
        };
        let journal = d.journal_directory().to_owned();
        let completion = d
            .complete_with(call(), |provider, request| {
                let started: ObjectRef = serde_json::from_slice(
                    &std::fs::read(journal.join("00000000000000000000.json")).unwrap(),
                )
                .unwrap();
                let snapshot: Value = serde_json::from_slice(&audit.get(started).unwrap()).unwrap();
                assert!(!snapshot["pending"].is_null());
                assert_eq!(snapshot["budget"]["calls_started"], 1);
                assert_eq!(snapshot["budget"]["output_accounting_complete"], false);
                assert!(provider.subscription());
                assert_eq!(request.system, context.system());
                assert_eq!(request.schema, schema);
                assert!(request.timeout <= Duration::from_secs(30));
                Ok(Completion {
                    raw_output: b"{\"action\":{\"tool\":\"finish\",\"arguments\":{}}}".to_vec(),
                    usage: Usage {
                        input_tokens: Some(999),
                        ..Usage::default()
                    },
                    observed_model: Some("fixture".into()),
                })
            })
            .unwrap();
        let finished: ObjectRef = serde_json::from_slice(
            &std::fs::read(journal.join("00000000000000000001.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(d.last_checkpoint(), Some(finished));
        let snapshot: Value = serde_json::from_slice(&audit.get(finished).unwrap()).unwrap();
        assert!(snapshot["pending"].is_null());
        assert_eq!(snapshot["attempts"][0]["transport_completed"], true);
        assert!(!snapshot["previous"].is_null());
        assert_eq!(
            audit.get(d.attempts()[0].output.unwrap()).unwrap(),
            completion.raw_output
        );
        assert_eq!(
            d.budget().input_reference_tokens,
            d.accounting().totals().context_reference_tokens
        );
        assert_eq!(d.attempts()[0].provider_usage.input_tokens, Some(999));
        assert!(matches!(
            d.complete_with(call(), |_, _| panic!("budget must block dispatch")),
            Err(Error::Budget(FailCode::CallBudget))
        ));
        assert_eq!(d.accounting().calls().len(), 1);
    }
    #[test]
    fn artifact_content_is_classified_by_provenance_not_retrieval_channel() {
        use accounting::SegmentKind::*;
        let dir = tempfile::tempdir().unwrap();
        let mut d = Dispatcher::new(
            Limits {
                calls: 1,
                reference_tokens: 1000,
                elapsed: Duration::from_secs(30),
            },
            accounting::Pipeline::Prose,
            myr_cas::Store::open(dir.path()).unwrap(),
        )
        .unwrap();
        let artifact =
            |b: u8| ObjectRef::new(myr_core::Kind::Artifact, myr_wire::artifact_cid(&[b]));
        let repository = BTreeSet::from([artifact(0)]);
        d.record_artifact_origin("Worker", artifact(1));
        // The first author is retained even if another agent emits equal bytes.
        d.record_artifact_origin("ReviewerA", artifact(1));
        d.record_artifact_origin("ReviewerA", artifact(0));
        let classify =
            |agent, b| d.classify_artifact(agent, artifact(b), &repository, CasReferenced);
        assert_eq!(classify("ReviewerA", 0), DirectRepo);
        assert_eq!(classify("Worker", 1), LocalTool);
        assert_eq!(classify("ReviewerA", 1), InterAgentArtifact);
        assert_eq!(classify("ReviewerA", 2), CasReferenced);
        // Inter-agent artifacts count in both pipelines; the others never do.
        for pipeline in [accounting::Pipeline::Prose, accounting::Pipeline::Mw0] {
            let tokenizer = ReferenceTokenizer::new().unwrap();
            let mut ledger = accounting::Ledger::new(pipeline);
            ledger
                .record_call(
                    &tokenizer,
                    "ReviewerA",
                    &[
                        accounting::Segment {
                            kind: InterAgentArtifact,
                            text: "worker bytes",
                        },
                        accounting::Segment {
                            kind: DirectRepo,
                            text: "base bytes",
                        },
                        accounting::Segment {
                            kind: LocalTool,
                            text: "own bytes",
                        },
                    ],
                )
                .unwrap();
            assert_eq!(
                ledger.totals().communication_reference_tokens,
                tokenizer.count("worker bytes")
            );
        }
    }

    #[test]
    fn unknown_output_stops_future_dispatch_and_preserves_uncertainty() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Dispatcher::new(
            Limits {
                calls: 5,
                reference_tokens: 10000,
                elapsed: Duration::from_secs(30),
            },
            accounting::Pipeline::Mw0,
            myr_cas::Store::open(dir.path()).unwrap(),
        )
        .unwrap();
        let context = PreparedContext::new("system", &[]).unwrap();
        let config = config();
        let schema = response_schema(Role::Worker);
        let call = || Call {
            agent: "worker",
            provider: &config,
            context: &context,
            schema: &schema,
            shared_schema: &schema,
            max_native_output_tokens: 100,
            max_reference_output_tokens: 100,
            timeout: Duration::from_secs(10),
        };
        assert!(matches!(
            d.complete_with(call(), |_, _| Err(transport::Error::Network)),
            Err(Error::Transport(_))
        ));
        assert!(!d.attempts()[0].transport_completed);
        assert_eq!(d.attempts()[0].provider_usage.input_tokens, None);
        assert!(!d.budget().output_accounting_complete);
        assert!(matches!(
            d.complete_with(call(), |_, _| panic!("no retry")),
            Err(Error::Budget(FailCode::TokenBudget))
        ));
    }

    #[test]
    fn journal_failure_prevents_transport_and_disables_future_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = Dispatcher::new(
            Limits {
                calls: 5,
                reference_tokens: 10000,
                elapsed: Duration::from_secs(30),
            },
            accounting::Pipeline::Mw0,
            myr_cas::Store::open(dir.path()).unwrap(),
        )
        .unwrap();
        std::fs::write(
            d.journal_directory().join("00000000000000000000.json"),
            b"existing record",
        )
        .unwrap();
        let context = PreparedContext::new("system", &[]).unwrap();
        let config = config();
        let schema = response_schema(Role::Worker);
        let call = || Call {
            agent: "worker",
            provider: &config,
            context: &context,
            schema: &schema,
            shared_schema: &schema,
            max_native_output_tokens: 100,
            max_reference_output_tokens: 100,
            timeout: Duration::from_secs(10),
        };
        for _ in 0..2 {
            assert!(
                d.complete_with(call(), |_, _| panic!(
                    "journal must be persisted before sending"
                ))
                .is_err()
            );
        }
        assert!(d.attempts().is_empty());
        assert!(d.last_checkpoint().is_none());
        assert_eq!(
            std::fs::read(d.journal_directory().join("00000000000000000000.json")).unwrap(),
            b"existing record"
        );
    }
}
