//! Fixed post-seal pipeline. Natural-language planning and INVALID_GOAL/UNSAT
//! proofs are separate; this runner never infers infeasibility from failure.
use crate::{
    acceptance, accounting, budget, candidate, command_verification, dispatch::Dispatcher,
    docker_sandbox::DockerRuntime, goal, review, task,
};
use myr_adapter::{Identity, SessionConfig, config::RoleSlot};
use myr_cas::Store;
use myr_core::*;
use myr_graph::{Authority, Graph};
use serde::Serialize;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    time::{Duration, Instant},
};

pub struct Config {
    pub worker_instruction: ObjectRef,
    pub review_instruction: ObjectRef,
    pub writable: Vec<String>,
    pub system: String,
    pub shared_schema: serde_json::Value,
    pub max_native_output_tokens: u32,
    pub max_reference_output_tokens: u64,
    pub quarantine: PathBuf,
    pub docker: DockerRuntime,
}

#[derive(Debug, Serialize)]
pub struct Stage {
    pub slot: RoleSlot,
    pub task: ObjectRef,
    pub result: task::TaskResult,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub format: String,
    pub goal: ObjectRef,
    pub state: MissionState,
    pub deadline_exceeded: bool,
    pub candidate: Option<ObjectRef>,
    pub stages: Vec<Stage>,
    pub commands: Vec<command_verification::Verification>,
    pub failures: Vec<ObjectRef>,
    pub acceptance: Option<acceptance::Acceptance>,
    pub artifacts: Vec<ObjectRef>,
    pub evidence: Vec<ObjectRef>,
    pub active_assumptions: Vec<ObjectRef>,
    pub provenance: Vec<ObjectRef>,
    pub budget: budget::Ledger,
    pub private_dispatch_checkpoint: Option<ObjectRef>,
}

pub struct Outcome {
    pub report: Report,
    /// Private immutable historical report, not an always-current verdict.
    pub private_record: ObjectRef,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("pipeline: {0}")]
    Runner(#[from] crate::Error),
    #[error("pipeline dispatch: {0}")]
    Dispatch(#[from] crate::dispatch::Error),
    #[error("mission planning: {0}")]
    Planning(#[from] crate::planning::Error),
}

pub struct MissionConfig {
    pub planner: crate::planning::Config,
    pub execution: Config,
}

#[derive(Debug, Default, Serialize)]
pub struct ResultContents {
    pub artifacts: Vec<ObjectRef>,
    pub evidence: Vec<ObjectRef>,
    pub active_assumptions: Vec<ObjectRef>,
    pub provenance: Vec<ObjectRef>,
}

#[derive(Debug, Serialize)]
pub struct PlanningFailureReport {
    pub format: String,
    pub state: MissionState,
    pub phase: String,
    pub failure: ObjectRef,
    pub created: Vec<ObjectRef>,
    #[serde(flatten)]
    pub contents: ResultContents,
    pub budget: budget::Ledger,
    pub private_dispatch_checkpoint: Option<ObjectRef>,
}

pub enum MissionOutcome {
    Executed {
        planning_created: Vec<ObjectRef>,
        outcome: Box<Outcome>,
    },
    PlanningFailed {
        report: Box<PlanningFailureReport>,
        private_record: ObjectRef,
    },
}

/// Full provider coordination for an already prepared mission catalog/policy.
/// This executes real configured transports. No budget or deadline resets at G0.
pub fn plan_and_run(
    graph: &mut Graph,
    audit: &Store,
    mission: &crate::mission::Mission,
    policy: &goal::SealPolicy,
    catalog: &mut crate::planner::Catalog,
    config: &MissionConfig,
) -> Result<MissionOutcome, Error> {
    if audit.root().starts_with(graph.cas().root()) || graph.cas().root().starts_with(audit.root())
    {
        return Err(
            crate::Error::from(invalid("mission audit must be separate from shared CAS")).into(),
        );
    }
    for reference in [
        config.execution.worker_instruction,
        config.execution.review_instruction,
    ] {
        reference
            .require(Kind::Artifact)
            .map_err(crate::Error::from)?;
        if !graph.live(reference).map_err(crate::Error::from)? {
            return Err(crate::Error::from(invalid("mission instruction is inactive")).into());
        }
    }
    let mut inputs = policy.verifier_policies.clone();
    inputs.extend([
        policy.baseline,
        config.execution.worker_instruction,
        config.execution.review_instruction,
    ]);
    let input = graph
        .register_artifact_with_dependencies(
            &serde_json::to_vec(&serde_json::json!({"mission":mission,"policy":policy}))
                .map_err(crate::Error::from)?,
            &inputs,
        )
        .map_err(crate::Error::from)?;
    let mut dispatcher = Dispatcher::new(
        budget::Limits {
            calls: policy.call_budget,
            reference_tokens: policy.token_budget,
            elapsed: Duration::from_millis(policy.time_budget_ms),
        },
        accounting::Pipeline::Mw0,
        audit.clone(),
    )?;
    match crate::planning::run(
        graph,
        &mut dispatcher,
        mission,
        policy,
        catalog,
        &config.planner,
    )? {
        crate::planning::Outcome::Failed { failure, created } => {
            let mut roots = created.clone();
            roots.extend([failure, input]);
            let report = PlanningFailureReport {
                format: "myr-planning-failure-v0".into(),
                state: MissionState::Partial,
                phase: "planning".into(),
                failure,
                created,
                contents: collect_contents(graph, roots)?,
                budget: dispatcher.budget().clone(),
                private_dispatch_checkpoint: dispatcher.last_checkpoint(),
            };
            let private_record = audit
                .put_artifact(&serde_json::to_vec(&report).map_err(crate::Error::from)?)
                .map_err(crate::Error::from)?;
            Ok(MissionOutcome::PlanningFailed {
                report: Box::new(report),
                private_record,
            })
        }
        crate::planning::Outcome::Sealed { goal, created } => {
            let outcome = run_continuing(
                graph,
                audit,
                goal.reference(),
                &config.execution,
                &mut dispatcher,
            )?;
            Ok(MissionOutcome::Executed {
                planning_created: created,
                outcome: Box::new(outcome),
            })
        }
    }
}

trait Driver {
    fn task(
        &mut self,
        graph: &mut Graph,
        dispatcher: &mut Dispatcher,
        execution: task::Execution,
    ) -> Result<task::TaskResult, task::Error>;
    fn command(
        &mut self,
        graph: &mut Graph,
        audit: &Store,
        candidate: &candidate::RecordedCandidate,
        atom: ObjectRef,
        remaining: Duration,
    ) -> Result<command_verification::Verification, command_verification::Error>;
}
struct Native<'a>(&'a DockerRuntime);
impl Driver for Native<'_> {
    fn task(
        &mut self,
        graph: &mut Graph,
        dispatcher: &mut Dispatcher,
        execution: task::Execution,
    ) -> Result<task::TaskResult, task::Error> {
        task::run(graph, dispatcher, execution)
    }
    fn command(
        &mut self,
        graph: &mut Graph,
        audit: &Store,
        candidate: &candidate::RecordedCandidate,
        atom: ObjectRef,
        remaining: Duration,
    ) -> Result<command_verification::Verification, command_verification::Error> {
        command_verification::verify_bounded(graph, audit, candidate, atom, self.0, remaining)
    }
}

/// Executes configured providers and consumes their selected billing route.
/// Calling this function is an explicit mission execution, unlike inspection.
pub fn run_sealed(
    graph: &mut Graph,
    audit: &Store,
    goal_ref: ObjectRef,
    config: &Config,
) -> Result<Outcome, Error> {
    run_with(graph, audit, goal_ref, config, &mut Native(&config.docker))
}

fn execution(
    sealed: &goal::SealedGoal,
    config: &Config,
    slot: RoleSlot,
    task: ObjectRef,
    context: Option<ObjectRef>,
) -> task::Execution {
    let provider = sealed.policy().providers.get(slot).clone();
    task::Execution {
        review_context: context,
        slot,
        provider: provider.clone(),
        session: SessionConfig {
            task,
            role: slot.role(),
            identity: Identity {
                agent: format!("{slot:?}"),
                lineage: provider.lineage,
            },
            baseline: Default::default(),
            protected: vec![],
            quarantine: config.quarantine.join(format!("{slot:?}")),
        },
        system: config.system.clone(),
        shared_schema: config.shared_schema.clone(),
        max_native_output_tokens: config.max_native_output_tokens,
        max_reference_output_tokens: config.max_reference_output_tokens,
    }
}

fn fail(
    graph: &mut Graph,
    report: &mut Report,
    code: FailCode,
    message: &str,
) -> crate::Result<()> {
    let diagnostic = graph.register_artifact(message.as_bytes())?;
    report.failures.push(graph.insert(
        Authority::Runtime,
        &Object::Fail(Fail {
            class: code.class(),
            code,
            diagnostic,
            task: None,
        }),
    )?);
    Ok(())
}

fn run_with(
    graph: &mut Graph,
    audit: &Store,
    goal_ref: ObjectRef,
    config: &Config,
    driver: &mut impl Driver,
) -> Result<Outcome, Error> {
    run_existing_with(graph, audit, goal_ref, config, driver, None)
}

/// Retain the issued task even when the task loop cannot return a result.
fn fail_stage(
    graph: &mut Graph,
    report: &mut Report,
    slot: RoleSlot,
    task: ObjectRef,
    error: &task::Error,
) -> crate::Result<()> {
    let diagnostic = graph.register_artifact(error.to_string().as_bytes())?;
    let failure = graph.insert(
        Authority::Runtime,
        &Object::Fail(Fail {
            class: FailCode::RuntimeError.class(),
            code: FailCode::RuntimeError,
            diagnostic,
            task: graph.live(task)?.then_some(task),
        }),
    )?;
    report.failures.push(failure);
    report.stages.push(Stage {
        slot,
        task,
        result: task::TaskResult {
            finished: false,
            objects: error.committed_objects(task).to_vec(),
            failure: Some(failure),
        },
    });
    Ok(())
}

/// Continue planning's dispatcher, retaining its consumption, audit and deadline.
pub fn run_continuing(
    graph: &mut Graph,
    audit: &Store,
    goal_ref: ObjectRef,
    config: &Config,
    dispatcher: &mut Dispatcher,
) -> Result<Outcome, Error> {
    run_existing_with(
        graph,
        audit,
        goal_ref,
        config,
        &mut Native(&config.docker),
        Some(dispatcher),
    )
}

fn run_existing_with(
    graph: &mut Graph,
    audit: &Store,
    goal_ref: ObjectRef,
    config: &Config,
    driver: &mut impl Driver,
    existing: Option<&mut Dispatcher>,
) -> Result<Outcome, Error> {
    let started = Instant::now();
    let sealed = goal::load(graph, goal_ref)?;
    if audit.root().starts_with(graph.cas().root()) || graph.cas().root().starts_with(audit.root())
    {
        return Err(
            crate::Error::from(invalid("pipeline audit must be separate from shared CAS")).into(),
        );
    }
    for reference in [config.worker_instruction, config.review_instruction] {
        reference
            .require(Kind::Artifact)
            .map_err(crate::Error::from)?;
        if !graph.live(reference).map_err(crate::Error::from)? {
            return Err(crate::Error::from(invalid("pipeline instruction is inactive")).into());
        }
    }
    if config.max_native_output_tokens == 0 || config.max_reference_output_tokens == 0 {
        return Err(crate::Error::from(invalid("positive output limits required")).into());
    }
    let duration = Duration::from_millis(sealed.policy().time_budget_ms);
    let mut owned = if existing.is_none() {
        Some(Dispatcher::new(
            budget::Limits {
                calls: sealed.policy().call_budget,
                reference_tokens: sealed.policy().token_budget,
                elapsed: duration.saturating_sub(started.elapsed()),
            },
            accounting::Pipeline::Mw0,
            audit.clone(),
        )?)
    } else {
        None
    };
    let dispatcher = existing.unwrap_or_else(|| owned.as_mut().expect("owned dispatcher"));
    let limits = dispatcher.limits();
    if limits.calls > sealed.policy().call_budget
        || limits.reference_tokens > sealed.policy().token_budget
        || limits.elapsed > duration
        || dispatcher.audit_root() != audit.root()
        || dispatcher.accounting().pipeline() != accounting::Pipeline::Mw0
    {
        return Err(crate::Error::from(invalid(
            "continuing dispatcher differs from sealed mission limits or audit",
        ))
        .into());
    }
    let deadline = dispatcher.deadline();
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let mut report = Report {
        format: "myr-sealed-pipeline-v0".into(),
        goal: goal_ref,
        state: MissionState::Partial,
        deadline_exceeded: false,
        candidate: None,
        stages: vec![],
        commands: vec![],
        failures: vec![],
        acceptance: None,
        artifacts: vec![],
        evidence: vec![],
        active_assumptions: vec![],
        provenance: vec![],
        budget: dispatcher.budget().clone(),
        private_dispatch_checkpoint: None,
    };
    let obligations: Vec<_> = sealed
        .ir()
        .criteria
        .iter()
        .filter(|c| c.binding)
        .map(|c| c.atom)
        .collect();
    // The worker produces the artifacts every sealed interpretive choice affects,
    // so all pending assumptions materialize here, not earlier and not silently.
    let decisions: Vec<_> = sealed.ir().assumptions.iter().map(|a| a.atom).collect();
    let worker = goal::issue_task(
        graph,
        goal_ref,
        goal::TaskDraft {
            // The sealed registry is the only predicate vocabulary the worker
            // may use in claims; the rest of its closure comes from the scope.
            inputs: sealed.ir().registry.clone(),
            capabilities: config
                .writable
                .iter()
                .map(|p| format!("write:{p}"))
                .collect(),
            obligations: obligations.clone(),
            decisions,
            instruction: config.worker_instruction,
        },
    )?;
    let result = driver.task(
        graph,
        dispatcher,
        execution(&sealed, config, RoleSlot::Worker, worker, None),
    );
    let work = match result {
        Ok(result) => result,
        Err(error) => {
            fail_stage(graph, &mut report, RoleSlot::Worker, worker, &error)?;
            return finish(graph, audit, dispatcher, report, deadline);
        }
    };
    let finished = work.finished;
    if let Some(failure) = work.failure {
        report.failures.push(failure);
    }
    let deltas: Vec<_> = work
        .objects
        .iter()
        .copied()
        .filter(|r| r.kind == Kind::Delta)
        .collect();
    report.stages.push(Stage {
        slot: RoleSlot::Worker,
        task: worker,
        result: work,
    });
    if !finished || !report.failures.is_empty() {
        return finish(graph, audit, dispatcher, report, deadline);
    }
    let candidate = match candidate::prepare_recorded(graph, goal_ref, &deltas, &config.writable) {
        Ok(candidate) => candidate,
        Err(error) => {
            fail(
                graph,
                &mut report,
                FailCode::InvalidAgentOutput,
                &error.to_string(),
            )?;
            return finish(graph, audit, dispatcher, report, deadline);
        }
    };
    report.candidate = Some(candidate.reference());
    let mut claims = vec![];
    for criterion in sealed.ir().criteria.iter().filter(|c| c.binding) {
        if remaining().is_zero() {
            fail(
                graph,
                &mut report,
                FailCode::TimeBudget,
                "Mission deadline reached",
            )?;
            return finish(graph, audit, dispatcher, report, deadline);
        }
        let Object::Atom(atom) = graph.get(criterion.atom).map_err(crate::Error::from)? else {
            unreachable!()
        };
        let Object::PredicateDef(predicate) =
            graph.get(atom.predicate).map_err(crate::Error::from)?
        else {
            unreachable!()
        };
        if predicate.name == "core.command_succeeds" || predicate.class == PredicateClass::Empirical
        {
            match driver.command(graph, audit, &candidate, criterion.atom, remaining()) {
                Ok(verification) => report.commands.push(verification),
                Err(error) => fail(
                    graph,
                    &mut report,
                    match &error {
                        command_verification::Error::Execution(_) => FailCode::SandboxUnavailable,
                        command_verification::Error::Runner(_) => FailCode::RuntimeError,
                    },
                    &error.to_string(),
                )?,
            }
        }
        claims.push(
            graph
                .insert(
                    Authority::Runtime,
                    &Object::Claim(Claim {
                        atom: criterion.atom,
                        polarity: criterion.polarity,
                        scope: sealed.scope(),
                    }),
                )
                .map_err(crate::Error::from)?,
        );
    }
    // Evidence-backed UNSAT: when the sealed policy leaves no baseline path
    // writable, the baseline is the only admissible candidate. Deterministic
    // evidence contradicting a binding obligation on it proves the obligations
    // incompatible (Appendix B.3 rule 1: no reviewer agreement can override D-).
    if let Some(proof) = unsat_proof(graph, audit, &sealed, &candidate)? {
        let diagnostic = graph
            .register_artifact_with_dependencies(
                &serde_json::to_vec(&proof).map_err(crate::Error::from)?,
                &proof.dependencies(),
            )
            .map_err(crate::Error::from)?;
        report.failures.push(
            graph
                .insert(
                    Authority::Runtime,
                    &Object::Fail(Fail {
                        class: FailCode::ConflictingObligations.class(),
                        code: FailCode::ConflictingObligations,
                        diagnostic,
                        task: None,
                    }),
                )
                .map_err(crate::Error::from)?,
        );
        report.acceptance = Some(proof.acceptance);
        report.state = MissionState::Unsat;
        return finish(graph, audit, dispatcher, report, deadline);
    }
    for slot in [RoleSlot::ReviewerA, RoleSlot::ReviewerB] {
        if remaining().is_zero() {
            fail(
                graph,
                &mut report,
                FailCode::TimeBudget,
                "Mission deadline reached",
            )?;
            break;
        }
        let context = review::create_context(
            graph,
            candidate.reference(),
            slot,
            config.review_instruction,
        )?;
        let inputs = std::iter::once(context)
            .chain(claims.iter().copied())
            .collect();
        let task = goal::issue_task(
            graph,
            goal_ref,
            goal::TaskDraft {
                inputs,
                capabilities: vec![],
                obligations: obligations.clone(),
                decisions: vec![],
                instruction: config.review_instruction,
            },
        )?;
        match driver.task(
            graph,
            dispatcher,
            execution(&sealed, config, slot, task, Some(context)),
        ) {
            Ok(result) => {
                if let Some(failure) = result.failure {
                    report.failures.push(failure);
                }
                let mut reviewed = BTreeSet::new();
                for reference in &result.objects {
                    if reference.kind == Kind::Evidence {
                        let Object::Evidence(evidence) =
                            graph.get(*reference).map_err(crate::Error::from)?
                        else {
                            unreachable!()
                        };
                        if review::binds(graph, candidate.reference(), &evidence)? {
                            reviewed.insert(evidence.claim);
                        }
                    }
                }
                if !result.finished || claims.iter().any(|c| !reviewed.contains(c)) {
                    fail(
                        graph,
                        &mut report,
                        FailCode::InvalidAgentOutput,
                        "Reviewer did not finish a bound review of every binding claim",
                    )?;
                }
                report.stages.push(Stage { slot, task, result });
            }
            Err(error) => fail_stage(graph, &mut report, slot, task, &error)?,
        }
    }
    if remaining().is_zero() {
        fail(
            graph,
            &mut report,
            FailCode::TimeBudget,
            "Mission deadline reached",
        )?;
    }
    match acceptance::assess_candidate(graph, audit, candidate.reference()) {
        Ok(assessment) => report.acceptance = Some(assessment),
        Err(error) => fail(
            graph,
            &mut report,
            FailCode::InvalidatedReference,
            &error.to_string(),
        )?,
    }
    finish(graph, audit, dispatcher, report, deadline)
}

/// Machine-readable proof recorded as the CONFLICTING_OBLIGATIONS diagnostic.
#[derive(Serialize)]
struct UnsatProof {
    format: String,
    candidate: ObjectRef,
    writable: Vec<String>,
    protected: Vec<String>,
    editable_paths: Vec<String>,
    refuted: Vec<acceptance::Obligation>,
    #[serde(skip)]
    acceptance: acceptance::Acceptance,
}

impl UnsatProof {
    fn dependencies(&self) -> Vec<ObjectRef> {
        let mut dependencies = vec![self.candidate];
        for obligation in &self.refuted {
            dependencies.push(obligation.atom);
            dependencies.extend(obligation.refuting_evidence.iter().copied());
        }
        dependencies.sort();
        dependencies.dedup();
        dependencies
    }
}

fn unsat_proof(
    graph: &Graph,
    audit: &Store,
    sealed: &goal::SealedGoal,
    candidate: &candidate::RecordedCandidate,
) -> Result<Option<UnsatProof>, Error> {
    let manifest = candidate::load_manifest(graph, candidate.reference())?;
    let editable_paths = manifest.editable_paths(&sealed.policy().protected);
    if !editable_paths.is_empty() {
        return Ok(None);
    }
    let assessment = acceptance::assess_candidate(graph, audit, candidate.reference())?;
    if !assessment.refuted_by_deterministic_evidence() {
        return Ok(None);
    }
    let refuted = assessment
        .obligations
        .iter()
        .filter(|o| o.status == acceptance::ObligationStatus::Refuted)
        .cloned()
        .collect();
    Ok(Some(UnsatProof {
        format: "myr-unsat-proof-v0".into(),
        candidate: candidate.reference(),
        writable: manifest.writable,
        protected: sealed.policy().protected.clone(),
        editable_paths,
        refuted,
        acceptance: assessment,
    }))
}

fn finish(
    graph: &Graph,
    audit: &Store,
    dispatcher: &Dispatcher,
    mut report: Report,
    deadline: Instant,
) -> Result<Outcome, Error> {
    let mut roots = report.failures.clone();
    roots.push(report.goal);
    if let Some(candidate) = report.candidate {
        roots.push(candidate);
    }
    for stage in &report.stages {
        roots.push(stage.task);
        roots.extend(stage.result.objects.iter().copied());
    }
    for command in &report.commands {
        roots.extend([command.record, command.claim, command.evidence]);
    }
    let contents = collect_contents(graph, roots)?;
    report.artifacts = contents.artifacts;
    report.evidence = contents.evidence;
    report.active_assumptions = contents.active_assumptions;
    report.provenance = contents.provenance;
    report.deadline_exceeded = Instant::now() >= deadline;
    if !report.deadline_exceeded
        && report.failures.is_empty()
        && report.stages.len() == 3
        && report.stages.iter().all(|s| s.result.finished)
        && report
            .acceptance
            .as_ref()
            .is_some_and(|a| a.all_binding_proven())
    {
        report.state = if report.active_assumptions.is_empty() {
            MissionState::Complete
        } else {
            MissionState::CompleteWithAssumptions
        };
    }
    report.budget = dispatcher.budget().clone();
    report.private_dispatch_checkpoint = dispatcher.last_checkpoint();
    let private_record = audit
        .put_artifact(&serde_json::to_vec(&report).map_err(crate::Error::from)?)
        .map_err(crate::Error::from)?;
    Ok(Outcome {
        report,
        private_record,
    })
}

fn collect_contents(
    graph: &Graph,
    roots: impl IntoIterator<Item = ObjectRef>,
) -> Result<ResultContents, Error> {
    let mut result = ResultContents::default();
    let mut provenance = BTreeSet::new();
    for root in roots {
        provenance.extend(graph.dependencies(root).map_err(crate::Error::from)?);
    }
    for reference in provenance {
        match reference.kind {
            Kind::Artifact => result.artifacts.push(reference),
            Kind::Evidence => result.evidence.push(reference),
            Kind::Assumption if graph.live(reference).map_err(crate::Error::from)? => {
                if let Object::Assumption(a) = graph.get(reference).map_err(crate::Error::from)?
                    && a.invalidates.is_none()
                {
                    result.active_assumptions.push(reference);
                }
            }
            _ => {}
        }
        result.provenance.push(reference);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_verification::tests::{Fixture, record_fixture};
    use myr_adapter::{
        Action, Response,
        transport::{Completion, Usage},
    };

    struct Simulated {
        mode: &'static str,
        policy: ObjectRef,
        commands: usize,
    }
    impl Driver for Simulated {
        fn task(
            &mut self,
            graph: &mut Graph,
            dispatcher: &mut Dispatcher,
            execution: task::Execution,
        ) -> Result<task::TaskResult, task::Error> {
            let slot = execution.slot;
            if (self.mode == "worker_runtime_error" && slot == RoleSlot::Worker)
                || (self.mode == "reviewer_runtime_error" && slot == RoleSlot::ReviewerA)
            {
                return Err(
                    crate::Error::from(invalid("Injected task infrastructure failure")).into(),
                );
            }
            let Object::Task(task) = graph.get(execution.session.task).unwrap() else {
                unreachable!()
            };
            let claims: Vec<_> = task
                .inputs
                .iter()
                .copied()
                .filter(|r| r.kind == Kind::Claim)
                .collect();
            let interrupt = (self.mode == "interrupt_worker" && slot == RoleSlot::Worker)
                || (self.mode == "interrupt_reviewer" && slot == RoleSlot::ReviewerA);
            if interrupt {
                let sequence = std::fs::read_dir(dispatcher.journal_directory())
                    .unwrap()
                    .count();
                std::fs::write(
                    dispatcher
                        .journal_directory()
                        .join(format!("{:020}.json", sequence + 3)),
                    b"injected checkpoint collision",
                )
                .unwrap();
            }
            let mut calls = 0;
            task::run_with(graph, dispatcher, execution, |_, _| {
                let raw_output = if self.mode == "bad_worker" && slot == RoleSlot::Worker {
                    b"invalid response".to_vec()
                } else {
                    let action = if interrupt && slot == RoleSlot::Worker && calls == 0 {
                        Action::PutArtifact {
                            content_base64: "eA==".into(),
                        }
                    } else if slot == RoleSlot::Worker
                        || self.mode == "empty_review"
                        || calls >= claims.len()
                    {
                        Action::Finish {}
                    } else {
                        Action::ReviewClaim {
                            claim_ref: claims[calls],
                            verdict: Verdict::Supports,
                            rationale_ref: task.instruction,
                        }
                    };
                    serde_json::to_vec(&Response { action }).unwrap()
                };
                calls += 1;
                Ok(Completion {
                    raw_output,
                    usage: Usage::default(),
                    observed_model: Some("fixture".into()),
                })
            })
        }
        fn command(
            &mut self,
            graph: &mut Graph,
            audit: &Store,
            candidate: &candidate::RecordedCandidate,
            atom: ObjectRef,
            remaining: Duration,
        ) -> Result<command_verification::Verification, command_verification::Error> {
            self.commands += 1;
            assert!(!remaining.is_zero());
            if self.mode == "sandbox_error" {
                return Err(
                    crate::docker_sandbox::Error::Rejected("fixture unavailable".into()).into(),
                );
            }
            // Persistence test only: never interpreted as a live container run.
            Ok(record_fixture(
                graph,
                audit,
                atom,
                crate::docker_sandbox::Execution {
                    candidate: candidate.reference(),
                    policy: self.policy,
                    exit_code: if self.mode == "refute" { 7 } else { 0 },
                    stdout: if self.mode == "empirical_success" || self.mode == "empirical_outside"
                    {
                        serde_json::to_vec(&crate::measurement::Observation {
                            format: "myr-measurement-v0".into(),
                            value: Decimal {
                                mantissa: if self.mode == "empirical_success" {
                                    17
                                } else {
                                    18
                                },
                                exponent: 0,
                            },
                            unit: "milliseconds".into(),
                        })
                        .unwrap()
                    } else {
                        b"fixture output".to_vec()
                    },
                    stderr: vec![],
                    image_metadata: serde_json::json!({"fixture":true}),
                    daemon_metadata: serde_json::json!({"fixture":true}),
                    container_metadata: serde_json::json!({"fixture":true}),
                    create_argv: vec!["fixture".into()],
                },
            )?)
        }
    }
    fn config(f: &mut Fixture) -> Config {
        let instruction = f
            .graph
            .register_artifact(b"Process every binding criterion using task references.")
            .unwrap();
        Config {
            worker_instruction: instruction,
            review_instruction: instruction,
            writable: vec!["f".into()],
            system: "Return Myr actions.".into(),
            shared_schema: myr_adapter::schema::response_schema(myr_adapter::Role::Worker),
            max_native_output_tokens: 1024,
            max_reference_output_tokens: 1024,
            quarantine: f._dir.path().join("quarantine"),
            docker: DockerRuntime {
                executable: f._dir.path().join("never-spawned"),
            },
        }
    }

    #[test]
    fn empirical_pipeline_requires_valid_measurement_within_sealed_tolerance() {
        for mode in [
            "empirical_success",
            "empirical_outside",
            "empirical_invalid",
        ] {
            let mut f = Fixture::new(true);
            f.empirical(true);
            let config = config(&mut f);
            let mut driver = Simulated {
                mode,
                policy: f.policy,
                commands: 0,
            };
            let result = run_with(&mut f.graph, &f.audit, f.goal, &config, &mut driver).unwrap();
            assert_eq!(driver.commands, 1);
            assert_eq!(result.report.stages.len(), 3);
            assert_eq!(
                result.report.state,
                if mode == "empirical_success" {
                    MissionState::Complete
                } else {
                    MissionState::Partial
                }
            );
            if mode == "empirical_invalid" {
                assert!(result.report.commands.is_empty());
                assert!(!result.report.failures.is_empty());
            } else {
                assert_eq!(result.report.commands.len(), 1);
                assert_eq!(
                    result.report.acceptance.unwrap().all_binding_proven(),
                    mode == "empirical_success"
                );
            }
        }
    }

    #[test]
    fn interrupted_tasks_preserve_committed_outputs_in_partial_private_reports() {
        for (mode, slot, kind) in [
            ("interrupt_worker", RoleSlot::Worker, Kind::Artifact),
            ("interrupt_reviewer", RoleSlot::ReviewerA, Kind::Evidence),
        ] {
            let mut f = Fixture::new(true);
            let config = config(&mut f);
            let mut driver = Simulated {
                mode,
                policy: f.policy,
                commands: 0,
            };
            let outcome = run_with(&mut f.graph, &f.audit, f.goal, &config, &mut driver).unwrap();
            let report = &outcome.report;
            assert_eq!(report.state, MissionState::Partial);
            let stage = report
                .stages
                .iter()
                .find(|stage| stage.slot == slot)
                .unwrap();
            assert!(!stage.result.finished);
            assert!(stage.result.failure.is_some());
            let output = *stage
                .result
                .objects
                .iter()
                .find(|r| r.kind == kind)
                .unwrap();
            assert!(report.provenance.contains(&output));
            if kind == Kind::Artifact {
                assert!(report.artifacts.contains(&output));
                assert_eq!(f.graph.cas().get(output).unwrap(), b"x");
                assert!(report.candidate.is_none());
            } else {
                assert!(report.evidence.contains(&output));
            }
            let stored: serde_json::Value =
                serde_json::from_slice(&f.audit.get(outcome.private_record).unwrap()).unwrap();
            assert_eq!(stored, serde_json::to_value(report).unwrap());
        }
    }

    #[test]
    fn task_infrastructure_errors_retain_failed_stage_and_dependencies() {
        for (mode, failed_slot, stages) in [
            ("worker_runtime_error", RoleSlot::Worker, 1),
            ("reviewer_runtime_error", RoleSlot::ReviewerA, 3),
        ] {
            let mut f = Fixture::new(true);
            let config = config(&mut f);
            let mut driver = Simulated {
                mode,
                policy: f.policy,
                commands: 0,
            };
            let outcome = run_with(&mut f.graph, &f.audit, f.goal, &config, &mut driver).unwrap();
            let report = outcome.report;
            assert_eq!(report.state, MissionState::Partial);
            assert_eq!(report.stages.len(), stages);
            let stage = report
                .stages
                .iter()
                .find(|s| s.slot == failed_slot)
                .unwrap();
            assert!(!stage.result.finished);
            let failure = stage.result.failure.unwrap();
            assert!(report.failures.contains(&failure));
            let Object::Fail(fail) = f.graph.get(failure).unwrap() else {
                panic!()
            };
            assert_eq!(fail.task, Some(stage.task));
            assert_eq!(fail.code, FailCode::RuntimeError);
            assert!(report.artifacts.contains(&fail.diagnostic));
            for dependency in f.graph.dependencies(stage.task).unwrap() {
                assert!(report.provenance.contains(&dependency));
            }
            if failed_slot == RoleSlot::ReviewerA {
                assert!(
                    report
                        .stages
                        .iter()
                        .any(|s| s.slot == RoleSlot::ReviewerB && s.result.finished)
                );
            }
        }
    }

    #[test]
    fn planner_handoff_keeps_budget_deadline_and_journal_across_all_roles() {
        use crate::{mission::Mission, planner::Catalog, planning};
        for limit in [1, 5, 6] {
            let mut f = Fixture::new(true);
            let config = config(&mut f);
            let sealed = goal::load(&f.graph, f.goal).unwrap();
            let mut catalog =
                Catalog::new(&f.graph, &sealed.ir().registry, &[f.atom], &[f.policy]).unwrap();
            let mut dispatcher = Dispatcher::new(
                budget::Limits {
                    calls: limit,
                    reference_tokens: sealed.policy().token_budget,
                    elapsed: Duration::from_millis(sealed.policy().time_budget_ms),
                },
                accounting::Pipeline::Mw0,
                f.audit.clone(),
            )
            .unwrap();
            let deadline = dispatcher.deadline();
            let journal = dispatcher.journal_directory().to_owned();
            let planned = planning::run_with(&mut f.graph, &mut dispatcher,
                &Mission { goal: sealed.ir().goal.clone(), verify: vec![vec!["check".into()]], protected: vec![] },
                sealed.policy(), &mut catalog, &planning::Config { system: "Plan the mission.".into(), shared_schema: config.shared_schema.clone(),
                    max_native_output_tokens: 1024, max_reference_output_tokens: 4096 },
                |_,_| Ok(Completion { raw_output: serde_json::to_vec(&serde_json::json!({"action":{"tool":"submit_goal","arguments":sealed.ir()}})).unwrap(),
                    usage: Usage::default(), observed_model: Some("fixture".into()) })
            ).unwrap();
            let planning::Outcome::Sealed { goal, .. } = planned else {
                panic!()
            };
            let mut driver = Simulated {
                mode: "success",
                policy: f.policy,
                commands: 0,
            };
            let outcome = run_existing_with(
                &mut f.graph,
                &f.audit,
                goal.reference(),
                &config,
                &mut driver,
                Some(&mut dispatcher),
            )
            .unwrap();
            assert_eq!(
                outcome.report.state,
                if limit == 6 {
                    MissionState::Complete
                } else {
                    MissionState::Partial
                }
            );
            assert_eq!(outcome.report.budget.calls_started, limit);
            assert_eq!(dispatcher.deadline(), deadline);
            assert_eq!(dispatcher.journal_directory(), journal);
            assert_eq!(dispatcher.accounting().calls().len(), limit as usize);
            if limit == 1 {
                assert_eq!(driver.commands, 0);
            }
            assert_eq!(dispatcher.attempts()[0].agent, "planner");
        }
    }

    #[test]
    fn continuing_dispatcher_cannot_expand_sealed_limits_or_change_audit() {
        let mut f = Fixture::new(true);
        let config = config(&mut f);
        let sealed = goal::load(&f.graph, f.goal).unwrap();
        for alternate_audit in [false, true] {
            let audit = if alternate_audit {
                Store::open(f._dir.path().join("wrong-audit")).unwrap()
            } else {
                f.audit.clone()
            };
            let mut dispatcher = Dispatcher::new(
                budget::Limits {
                    calls: sealed.policy().call_budget + u32::from(!alternate_audit),
                    reference_tokens: sealed.policy().token_budget,
                    elapsed: Duration::from_millis(sealed.policy().time_budget_ms),
                },
                accounting::Pipeline::Mw0,
                audit,
            )
            .unwrap();
            let mut driver = Simulated {
                mode: "success",
                policy: f.policy,
                commands: 0,
            };
            assert!(
                run_existing_with(
                    &mut f.graph,
                    &f.audit,
                    f.goal,
                    &config,
                    &mut driver,
                    Some(&mut dispatcher)
                )
                .is_err()
            );
            assert!(dispatcher.attempts().is_empty());
            assert_eq!(driver.commands, 0);
        }
    }

    #[test]
    fn sealed_pipeline_runs_all_stages_and_keeps_conditional_completion_explicit() {
        for conditional in [false, true] {
            let mut f = Fixture::new(true);
            let config = config(&mut f);
            if conditional {
                let sealed = goal::load(&f.graph, f.goal).unwrap();
                let mut ir = sealed.ir().clone();
                let predicate = f
                    .graph
                    .insert(
                        Authority::Compiler,
                        &Object::PredicateDef(PredicateDef {
                            name: "mission.fixture_choice".into(),
                            version: 1,
                            arguments: vec![],
                            semantics: "Fixture unresolved choice".into(),
                            class: PredicateClass::Unresolved,
                            provenance: Some(config.worker_instruction),
                        }),
                    )
                    .unwrap();
                let atom = f
                    .graph
                    .insert(
                        Authority::Compiler,
                        &Object::Atom(Atom {
                            predicate,
                            arguments: vec![],
                        }),
                    )
                    .unwrap();
                ir.registry.push(predicate);
                ir.assumptions.push(goal::PendingAssumption {
                    atom,
                    question: "Choose fixture behavior?".into(),
                    chosen: "yes".into(),
                    alternatives: vec!["no".into()],
                    rationale: config.worker_instruction,
                    artifacts: vec![],
                });
                f.goal = goal::compile(&mut f.graph, ir, sealed.policy().clone())
                    .unwrap()
                    .reference();
            }
            let mut driver = Simulated {
                mode: "success",
                policy: f.policy,
                commands: 0,
            };
            let outcome = run_with(&mut f.graph, &f.audit, f.goal, &config, &mut driver).unwrap();
            assert_eq!(
                outcome.report.state,
                if conditional {
                    MissionState::CompleteWithAssumptions
                } else {
                    MissionState::Complete
                }
            );
            // Every pending assumption sealed in the Goal IR materializes when
            // the worker task is issued: the worker depends on those choices.
            let Object::Task(worker_task) = f.graph.get(outcome.report.stages[0].task).unwrap()
            else {
                unreachable!()
            };
            assert_eq!(worker_task.assumptions.len(), usize::from(conditional));
            let sealed = goal::load(&f.graph, f.goal).unwrap();
            for pending in &sealed.ir().assumptions {
                assert!(worker_task.inputs.contains(&pending.atom));
            }
            // The worker may only use registered predicates, so every sealed
            // registry entry must be inside its task reference closure.
            assert!(!sealed.ir().registry.is_empty());
            for predicate in &sealed.ir().registry {
                assert!(worker_task.inputs.contains(predicate));
                assert!(
                    f.graph
                        .fetch(&[outcome.report.stages[0].task], *predicate)
                        .is_ok()
                );
            }
            assert_eq!(
                outcome
                    .report
                    .stages
                    .iter()
                    .map(|s| s.slot)
                    .collect::<Vec<_>>(),
                [RoleSlot::Worker, RoleSlot::ReviewerA, RoleSlot::ReviewerB]
            );
            assert_eq!(outcome.report.budget.calls_started, 5);
            assert_eq!(driver.commands, 1);
            assert_eq!(outcome.report.evidence.len(), 3);
            assert!(outcome.report.provenance.contains(&f.goal));
            for evidence in &outcome.report.evidence {
                assert!(outcome.report.provenance.contains(evidence));
                assert!(matches!(
                    f.graph.get(*evidence).unwrap(),
                    Object::Evidence(_)
                ));
            }
            assert_eq!(
                outcome.report.active_assumptions.len(),
                usize::from(conditional)
            );
            assert!(outcome.report.acceptance.unwrap().all_binding_proven());
            assert!(f.audit.get(outcome.private_record).is_ok());
            assert!(f.graph.resolve(outcome.private_record.cid).is_err());
        }
    }

    #[test]
    fn failures_and_finish_without_reviews_are_partial_never_unsat() {
        for mode in ["bad_worker", "sandbox_error", "empty_review"] {
            let mut f = Fixture::new(true);
            let config = config(&mut f);
            let mut driver = Simulated {
                mode,
                policy: f.policy,
                commands: 0,
            };
            let outcome = run_with(&mut f.graph, &f.audit, f.goal, &config, &mut driver).unwrap();
            assert_eq!(outcome.report.state, MissionState::Partial);
            assert!(!outcome.report.failures.is_empty());
            if mode == "bad_worker" {
                assert_eq!(driver.commands, 0);
                assert_eq!(outcome.report.stages.len(), 1);
            } else {
                assert_eq!(driver.commands, 1);
                assert_eq!(outcome.report.stages.len(), 3);
            }
        }
    }

    #[test]
    fn refuted_baseline_without_admissible_edits_is_unsat_with_conflicting_obligations() {
        use crate::acceptance::ObligationStatus;
        for writable in [vec![], vec!["f".to_owned()]] {
            let mut f = Fixture::new(true);
            let mut config = config(&mut f);
            config.writable = writable.clone();
            let mut driver = Simulated {
                mode: "refute",
                policy: f.policy,
                commands: 0,
            };
            let outcome = run_with(&mut f.graph, &f.audit, f.goal, &config, &mut driver).unwrap();
            let report = &outcome.report;
            assert_eq!(driver.commands, 1);
            let acceptance = report.acceptance.as_ref().unwrap();
            assert_eq!(acceptance.obligations[0].status, ObligationStatus::Refuted);
            assert_eq!(acceptance.obligations[0].refuting_evidence.len(), 1);
            if writable.is_empty() {
                // The sealed policy admits exactly one candidate (the baseline)
                // and deterministic evidence contradicts a binding obligation.
                assert_eq!(report.state, MissionState::Unsat);
                assert_eq!(report.stages.len(), 1, "reviewers cannot override D-");
                let conflict = report
                    .failures
                    .iter()
                    .find_map(|r| match f.graph.get(*r).unwrap() {
                        Object::Fail(fail) if fail.code == FailCode::ConflictingObligations => {
                            Some(fail)
                        }
                        _ => None,
                    })
                    .expect("conflicting obligations failure");
                assert_eq!(conflict.class, FailClass::Goal);
                let diagnostic = f.graph.cas().get(conflict.diagnostic).unwrap();
                let text = String::from_utf8(diagnostic).unwrap();
                assert!(
                    text.contains(
                        &acceptance.obligations[0].refuting_evidence[0]
                            .cid
                            .to_string()
                    )
                );
                assert!(
                    report
                        .evidence
                        .contains(&acceptance.obligations[0].refuting_evidence[0])
                );
                let stored: serde_json::Value =
                    serde_json::from_slice(&f.audit.get(outcome.private_record).unwrap()).unwrap();
                assert_eq!(stored["state"], "UNSAT");
            } else {
                // Another candidate could exist: a failed candidate is PARTIAL.
                assert_eq!(report.state, MissionState::Partial);
                assert_eq!(report.stages.len(), 3);
                assert!(report.failures.is_empty());
            }
        }
    }
}
