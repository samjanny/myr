//! Sequential single-task action loop. Mission topology and terminal mission
//! decisions are separate; an agent saying finish is not a verified mission.
use crate::{
    accounting::{ReferenceTokenizer, Segment, SegmentKind},
    budget::{Budget, Limits},
    context::PreparedContext,
    dispatch::{Call, Dispatcher},
};
use myr_adapter::{
    Action, Outcome, Response, Session, SessionConfig,
    config::ProviderConfig,
    transport::{self, Completion, Request},
};
use myr_core::*;
use myr_graph::{Authority, Graph};
use std::time::Duration;

pub struct Execution {
    pub review_context: Option<ObjectRef>,
    pub slot: myr_adapter::config::RoleSlot,
    pub session: SessionConfig,
    pub provider: ProviderConfig,
    pub system: String,
    pub shared_schema: serde_json::Value,
    pub max_native_output_tokens: u32,
    pub max_reference_output_tokens: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct TaskResult {
    pub finished: bool,
    pub objects: Vec<ObjectRef>,
    pub failure: Option<ObjectRef>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("task runner: {0}")]
    Runner(#[from] crate::Error),
    #[error("task adapter: {0}")]
    Adapter(#[from] myr_adapter::Error),
    #[error("task dispatch: {0}")]
    Dispatch(#[from] crate::dispatch::Error),
}

pub fn run(
    graph: &mut Graph,
    dispatcher: &mut Dispatcher,
    execution: Execution,
) -> Result<TaskResult, Error> {
    run_with(graph, dispatcher, execution, transport::complete)
}

fn failure(
    graph: &mut Graph,
    task: ObjectRef,
    objects: Vec<ObjectRef>,
    code: FailCode,
) -> Result<TaskResult, Error> {
    let diagnostic = graph
        .register_artifact(format!("Task {task:?} execution stopped: {code:?}").as_bytes())
        .map_err(crate::Error::from)?;
    let live_task = graph
        .live(task)
        .map_err(crate::Error::from)?
        .then_some(task);
    let fail = graph
        .insert(
            Authority::Runtime,
            &Object::Fail(Fail {
                class: code.class(),
                code,
                diagnostic,
                task: live_task,
            }),
        )
        .map_err(crate::Error::from)?;
    Ok(TaskResult {
        finished: false,
        objects,
        failure: Some(fail),
    })
}

pub(crate) fn run_with(
    graph: &mut Graph,
    dispatcher: &mut Dispatcher,
    mut execution: Execution,
    mut send: impl FnMut(&ProviderConfig, &Request) -> Result<Completion, transport::Error>,
) -> Result<TaskResult, Error> {
    let task_ref = execution.session.task;
    let Object::Task(task) = graph.get(task_ref).map_err(crate::Error::from)? else {
        return Err(crate::Error::from(invalid("task runner requires TASK")).into());
    };
    let sealed = crate::goal::load(graph, task.scope.goal)?;
    if execution.session.role != execution.slot.role()
        || &execution.provider != sealed.policy().providers.get(execution.slot)
    {
        return Err(crate::Error::from(invalid(
            "execution role or provider differs from sealed configuration",
        ))
        .into());
    }
    if task.scope != sealed.scope()
        || task.token_budget > sealed.policy().token_budget
        || task.call_budget > sealed.policy().call_budget
        || task.time_budget_ms > sealed.policy().time_budget_ms
        || task.obligations.iter().any(|a| {
            !sealed
                .ir()
                .criteria
                .iter()
                .any(|c| c.atom == *a && c.binding)
        })
    {
        return Err(crate::Error::from(invalid(
            "task scope, budgets or obligations exceed sealed goal",
        ))
        .into());
    }
    execution.session.baseline = serde_json::from_slice(
        &graph
            .cas()
            .get(sealed.policy().baseline)
            .map_err(crate::Error::from)?,
    )
    .map_err(crate::Error::from)?;
    execution.session.protected = sealed.policy().protected.clone();
    if execution.session.identity.lineage != execution.provider.lineage {
        return Err(crate::Error::from(invalid(
            "session identity and configured provider lineage differ",
        ))
        .into());
    }
    let agent = execution.session.identity.agent.clone();
    let schema = myr_adapter::schema::response_schema(execution.session.role);
    let baseline = execution.session.baseline.clone();
    if let Some(context) = execution.review_context {
        crate::review::validate_task(
            graph,
            context,
            task_ref,
            execution.slot,
            &execution.provider,
        )?;
    }
    let mut session = Session::new(graph, execution.session)?;
    if let Some(context) = execution.review_context {
        session.bind_review_context(graph, context)?;
    }
    let mut budget = Budget::new(Limits {
        calls: task.call_budget,
        reference_tokens: task.token_budget,
        elapsed: Duration::from_millis(task.time_budget_ms),
    })
    .map_err(crate::dispatch::Error::Budget)?;
    let tokenizer = ReferenceTokenizer::new()?;
    let mut history = vec![(
        SegmentKind::MwRender,
        myr_wire::render(&Object::Task(task.clone())).map_err(crate::Error::from)?,
    )];
    let mut objects = Vec::new();
    let mut cas_segments = Vec::new();
    loop {
        if !graph.live(task_ref).map_err(crate::Error::from)? {
            return failure(graph, task_ref, objects, FailCode::InvalidatedReference);
        }
        let segments: Vec<_> = history
            .iter()
            .map(|(kind, text)| Segment { kind: *kind, text })
            .collect();
        let context = PreparedContext::new(&execution.system, &segments)?
            .with_cas_prompt_segments(&cas_segments)?;
        let mut call = Call {
            agent: &agent,
            provider: &execution.provider,
            context: &context,
            schema: &schema,
            shared_schema: &execution.shared_schema,
            max_native_output_tokens: execution.max_native_output_tokens,
            max_reference_output_tokens: execution.max_reference_output_tokens,
            timeout: Duration::from_millis(task.time_budget_ms),
        };
        let input = dispatcher.preview_tokens(&call)?;
        let permit = match budget.reserve(input, execution.max_reference_output_tokens) {
            Ok(p) => p,
            Err(code) => return failure(graph, task_ref, objects, code),
        };
        call.timeout = permit.timeout();
        call.max_reference_output_tokens = permit.output_reference_allowance();
        let completion = match dispatcher.complete_with(call, &mut send) {
            Ok(c) => c,
            Err(crate::dispatch::Error::Budget(code)) => {
                let _ = budget.settle(permit, None);
                return failure(graph, task_ref, objects, code);
            }
            Err(crate::dispatch::Error::Transport(_)) => {
                let _ = budget.settle(permit, None);
                return failure(graph, task_ref, objects, FailCode::ProviderUnavailable);
            }
            Err(e) => return Err(e.into()),
        };
        let raw = std::str::from_utf8(&completion.raw_output).ok();
        if let Err(code) = budget.settle(permit, raw.map(|s| tokenizer.count(s))) {
            return failure(graph, task_ref, objects, code);
        }
        if !graph.live(task_ref).map_err(crate::Error::from)? {
            return failure(graph, task_ref, objects, FailCode::InvalidatedReference);
        }
        let response = serde_json::from_slice::<Response>(&completion.raw_output).ok();
        let outcome = session.handle(graph, &completion.raw_output)?;
        match outcome {
            Outcome::Applied {
                fetched_raw_bytes,
                objects: emitted,
                result,
                done,
            } => {
                if let Some(bytes) = fetched_raw_bytes {
                    dispatcher.record_cas_read(bytes)?;
                }
                let receipt = serde_json::json!({"objects":emitted,"result":result});
                objects.extend(emitted);
                if done {
                    return Ok(TaskResult {
                        finished: true,
                        objects,
                        failure: None,
                    });
                }
                let kind = match response.map(|r| r.action) {
                    Some(Action::Fetch { reference }) if reference == task.instruction => {
                        SegmentKind::Goal
                    }
                    Some(Action::Fetch { reference })
                        if baseline.values().any(|r| *r == reference) =>
                    {
                        SegmentKind::DirectRepo
                    }
                    Some(Action::Fetch { reference }) if reference.kind == Kind::Artifact => {
                        SegmentKind::CasReferenced
                    }
                    Some(Action::Fetch { .. }) => SegmentKind::MwRender,
                    _ => SegmentKind::LocalTool,
                };
                history.push((SegmentKind::LocalTool, raw.unwrap_or_default().into()));
                if fetched_raw_bytes.is_some() {
                    cas_segments.push(history.len());
                }
                history.push((
                    kind,
                    serde_json::to_string(&receipt).map_err(crate::Error::from)?,
                ));
            }
            Outcome::Repair { attempt, message } => {
                history.push((SegmentKind::LocalTool, raw.unwrap_or_default().into()));
                history.push((
                    SegmentKind::ValidationFeedback,
                    serde_json::json!({"repair_attempt":attempt,"message":message}).to_string(),
                ));
            }
            Outcome::Failed { failure } => {
                return Ok(TaskResult {
                    finished: false,
                    objects,
                    failure: Some(failure),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        accounting::Pipeline,
        views::{Tree, import_worker},
    };
    use myr_adapter::{Identity, Role, config::Backend};
    fn fixture(calls: u32) -> (tempfile::TempDir, Graph, Dispatcher, Execution) {
        let dir = tempfile::tempdir().unwrap();
        let mut graph = Graph::open(
            dir.path().join("graph.sqlite"),
            myr_cas::Store::open(dir.path().join("shared")).unwrap(),
        )
        .unwrap();
        let (snapshot, baseline) = import_worker(
            &mut graph,
            &Tree::from([("f".into(), b"original".to_vec())]),
        )
        .unwrap();
        let instruction = graph.register_artifact(b"Fixture instruction").unwrap();
        let policy = crate::verifier::VerifierPolicy::Command {
            argv: vec!["fixture".into()],
            cwd: ".".into(),
            environment: Default::default(),
            tool_versions: std::collections::BTreeMap::from([("fixture".into(), "v0".into())]),
            timeout_ms: 1000,
            max_output_bytes: 4096,
            sandbox: crate::verifier::SandboxPolicy {
                backend: crate::verifier::SandboxBackend::WindowsAppContainer,
                network: false,
                memory_bytes: 1024 * 1024,
                max_processes: 2,
            },
        };
        let policy_ref = graph.register_artifact(&policy.encode().unwrap()).unwrap();
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
        let goal = crate::goal::compile(
            &mut graph,
            crate::goal::GoalIr {
                goal: "Task loop fixture".into(),
                registry: vec![predicate],
                criteria: vec![crate::goal::Criterion {
                    atom,
                    polarity: true,
                    binding: true,
                    measurement: None,
                }],
                assumptions: vec![],
            },
            crate::goal::SealPolicy {
                providers: serde_json::from_str(include_str!(
                    "../../../fixtures/providers-test-only.json"
                ))
                .unwrap(),
                baseline: snapshot,
                protected: vec!["f".into()],
                verifier_policies: vec![policy_ref],
                token_budget: 100000,
                call_budget: calls,
                time_budget_ms: 30000,
            },
        )
        .unwrap()
        .reference();
        let task = graph
            .insert(
                Authority::Runtime,
                &Object::Task(Task {
                    scope: Scope { goal, snapshot },
                    inputs: baseline.values().copied().collect(),
                    capabilities: vec!["write:f".into()],
                    assumptions: vec![],
                    obligations: vec![],
                    instruction,
                    token_budget: 100000,
                    call_budget: calls,
                    time_budget_ms: 30000,
                }),
            )
            .unwrap();
        let provider = ProviderConfig {
            backend: Backend::ClaudeCode {
                executable: "never-spawned".into(),
            },
            model: "fixture".into(),
            lineage: Lineage {
                provider_id: "anthropic".into(),
                family_id: "fixture".into(),
                checkpoint_id: "v0".into(),
                procedure_id: "test".into(),
            },
        };
        let execution = Execution {
            review_context: None,
            slot: myr_adapter::config::RoleSlot::Worker,
            session: SessionConfig {
                task,
                role: Role::Worker,
                identity: Identity {
                    agent: "worker".into(),
                    lineage: provider.lineage.clone(),
                },
                baseline,
                protected: vec![],
                quarantine: dir.path().join("quarantine"),
            },
            provider,
            system: "Use the Myr action schema.".into(),
            shared_schema: myr_adapter::schema::response_schema(Role::Worker),
            max_native_output_tokens: 1000,
            max_reference_output_tokens: 1000,
        };
        let dispatcher = Dispatcher::new(
            Limits {
                calls: 10,
                reference_tokens: 1000000,
                elapsed: Duration::from_secs(60),
            },
            Pipeline::Mw0,
            myr_cas::Store::open(dir.path().join("private-audit")).unwrap(),
        )
        .unwrap();
        (dir, graph, dispatcher, execution)
    }
    fn reply(raw: &str) -> Result<Completion, transport::Error> {
        Ok(Completion {
            raw_output: raw.as_bytes().to_vec(),
            usage: transport::Usage::default(),
            observed_model: Some("fixture".into()),
        })
    }
    #[test]
    fn two_reviewers_bind_evidence_to_candidate_and_sealed_slots() {
        use crate::{acceptance, candidate, goal, review};
        use myr_adapter::config::RoleSlot;
        let (dir, mut graph, mut dispatcher, original) = fixture(5);
        let Object::Task(original_task) = graph.get(original.session.task).unwrap() else {
            unreachable!()
        };
        let sealed = goal::load(&graph, original_task.scope.goal).unwrap();
        let candidate =
            candidate::prepare_recorded(&mut graph, sealed.reference(), &[], &[]).unwrap();
        let atom = sealed.ir().criteria[0].atom;
        let claim = graph
            .insert(
                Authority::Worker,
                &Object::Claim(Claim {
                    atom,
                    polarity: sealed.ir().criteria[0].polarity,
                    scope: sealed.scope(),
                }),
            )
            .unwrap();
        let audit = myr_cas::Store::open(dir.path().join("review-audit")).unwrap();
        let mut contexts = vec![];
        for slot in [RoleSlot::ReviewerA, RoleSlot::ReviewerB] {
            let context = review::create_context(
                &mut graph,
                candidate.reference(),
                slot,
                original_task.instruction,
            )
            .unwrap();
            contexts.push(context);
            let task = goal::issue_task(
                &mut graph,
                sealed.reference(),
                goal::TaskDraft {
                    inputs: vec![context, claim],
                    capabilities: vec![],
                    obligations: vec![atom],
                    decisions: vec![],
                    instruction: original_task.instruction,
                },
            )
            .unwrap();
            let provider = sealed.policy().providers.get(slot).clone();
            let make_execution = |context| Execution {
                review_context: Some(context),
                slot,
                provider: provider.clone(),
                session: SessionConfig {
                    task,
                    role: Role::Reviewer,
                    identity: Identity {
                        agent: format!("{slot:?}"),
                        lineage: provider.lineage.clone(),
                    },
                    baseline: Default::default(),
                    protected: vec![],
                    quarantine: dir.path().join("review-quarantine"),
                },
                system: "Review the task references and return Myr actions.".into(),
                shared_schema: myr_adapter::schema::response_schema(Role::Reviewer),
                max_native_output_tokens: 1000,
                max_reference_output_tokens: 1000,
            };
            if slot == RoleSlot::ReviewerB {
                assert!(
                    run_with(
                        &mut graph,
                        &mut dispatcher,
                        make_execution(contexts[0]),
                        |_, _| panic!("mismatched context must not dispatch")
                    )
                    .is_err()
                );
            }
            let mut calls = 0;
            let result = run_with(
                &mut graph,
                &mut dispatcher,
                make_execution(context),
                |_, _| {
                    calls += 1;
                    if calls == 1 {
                        reply(
                            &serde_json::to_string(&Response {
                                action: Action::ReviewClaim {
                                    claim_ref: claim,
                                    verdict: Verdict::Supports,
                                    rationale_ref: original_task.instruction,
                                },
                            })
                            .unwrap(),
                        )
                    } else {
                        reply(r#"{"action":{"tool":"finish","arguments":{}}}"#)
                    }
                },
            )
            .unwrap();
            assert!(result.finished);
            let Object::Evidence(evidence) = graph.get(result.objects[0]).unwrap() else {
                unreachable!()
            };
            assert!(review::binds(&graph, candidate.reference(), &evidence).unwrap());
            let mut wrapper: myr_adapter::ReviewRationale =
                serde_json::from_slice(&graph.cas().get(evidence.rationale).unwrap()).unwrap();
            wrapper.rationale = candidate.reference();
            let forged = graph
                .register_artifact(&serde_json::to_vec(&wrapper).unwrap())
                .unwrap();
            let mut forged_evidence = evidence.clone();
            forged_evidence.rationale = forged;
            assert!(!review::binds(&graph, candidate.reference(), &forged_evidence).unwrap());
            assert_eq!(
                acceptance::assess_candidate(&graph, &audit, candidate.reference())
                    .unwrap()
                    .all_binding_proven(),
                slot == RoleSlot::ReviewerB
            );
        }
        let other = candidate::prepare_recorded(&mut graph, sealed.reference(), &[], &["f".into()])
            .unwrap();
        assert!(
            !acceptance::assess_candidate(&graph, &audit, other.reference())
                .unwrap()
                .all_binding_proven()
        );
        assert_eq!(dispatcher.attempts().len(), 4);
    }

    #[test]
    fn task_loop_returns_artifact_receipts_then_finishes_without_claiming_mission_success() {
        let (_dir, mut graph, mut dispatcher, execution) = fixture(5);
        let artifact = myr_wire::artifact_cid(b"x");
        let mut calls = 0;
        let result = run_with(&mut graph, &mut dispatcher, execution, |_, request| {
            calls += 1;
            if calls == 1 {
                reply(r#"{"action":{"tool":"put_artifact","arguments":{"content_base64":"eA=="}}}"#)
            } else {
                assert!(request.prompt.contains(&artifact.to_string()));
                reply(r#"{"action":{"tool":"finish","arguments":{}}}"#)
            }
        })
        .unwrap();
        assert!(result.finished);
        assert!(result.failure.is_none());
        assert_eq!(
            result.objects,
            vec![ObjectRef::new(Kind::Artifact, artifact)]
        );
        assert_eq!(dispatcher.attempts().len(), 2);
    }
    #[test]
    fn invalid_output_repairs_are_counted_and_third_invalid_response_fails() {
        let (_dir, mut graph, mut dispatcher, execution) = fixture(5);
        let result = run_with(&mut graph, &mut dispatcher, execution, |_, _| {
            reply("not json")
        })
        .unwrap();
        let Object::Fail(f) = graph.get(result.failure.unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(f.code, FailCode::InvalidAgentOutput);
        assert_eq!(dispatcher.attempts().len(), 3);
        assert!(
            dispatcher.accounting().calls()[1]
                .segments
                .iter()
                .any(|s| s.kind == SegmentKind::ValidationFeedback)
        );
    }
    #[test]
    fn fetch_bytes_are_journaled_once_per_read_even_without_a_later_dispatch() {
        for calls in [1, 3] {
            let (_dir, mut graph, mut dispatcher, execution) = fixture(calls);
            let reference = execution.session.baseline["f"];
            let raw_len = graph.cas().get(reference).unwrap().len() as u64;
            let fetch = serde_json::to_string(&Response {
                action: Action::Fetch { reference },
            })
            .unwrap();
            let mut sent = 0;
            let result = run_with(&mut graph, &mut dispatcher, execution, |_, request| {
                sent += 1;
                // Accounting metadata must not enlarge the model's context.
                assert!(!request.prompt.contains("fetched_raw_bytes"));
                if sent < 3 {
                    reply(&fetch)
                } else {
                    reply(r#"{"action":{"tool":"finish","arguments":{}}}"#)
                }
            })
            .unwrap();
            assert_eq!(result.finished, calls == 3);
            assert_eq!(sent, calls);
            assert_eq!(
                dispatcher.accounting().totals().cas_raw_bytes,
                raw_len * if calls == 1 { 1 } else { 2 }
            );
            let checkpoint = dispatcher.last_checkpoint().unwrap();
            let store = myr_cas::Store::open(dispatcher.audit_root()).unwrap();
            let journal: serde_json::Value =
                serde_json::from_slice(&store.get(checkpoint).unwrap()).unwrap();
            assert_eq!(
                journal["accounting"]["totals"]["cas_raw_bytes"],
                dispatcher.accounting().totals().cas_raw_bytes
            );
            // Raw reads do not themselves create an extra provider invocation.
            assert_eq!(dispatcher.accounting().calls().len(), calls as usize);
            let retrieved: Vec<_> = dispatcher
                .accounting()
                .calls()
                .iter()
                .flat_map(|call| &call.segments)
                .filter(|s| s.cas_derived)
                .collect();
            assert_eq!(retrieved.len(), if calls == 1 { 0 } else { 3 });
            assert!(
                retrieved
                    .iter()
                    .all(|s| s.kind == SegmentKind::DirectRepo && !s.communication)
            );
            assert_eq!(
                dispatcher.accounting().totals().cas_context_bytes,
                retrieved.iter().map(|s| s.utf8_bytes).sum::<u64>()
            );
        }
        let (_dir, mut graph, mut dispatcher, execution) = fixture(3);
        let inaccessible = graph.register_artifact(b"outside task closure").unwrap();
        let denied = serde_json::to_string(&Response {
            action: Action::Fetch {
                reference: inaccessible,
            },
        })
        .unwrap();
        let result = run_with(&mut graph, &mut dispatcher, execution, |_, _| {
            reply(&denied)
        })
        .unwrap();
        assert!(!result.finished);
        assert_eq!(dispatcher.accounting().totals().cas_raw_bytes, 0);
    }

    #[test]
    fn task_call_limit_stops_dispatch_even_with_mission_capacity_remaining() {
        let (_dir, mut graph, mut dispatcher, execution) = fixture(1);
        let result = run_with(&mut graph, &mut dispatcher, execution, |_, _| {
            reply("not json")
        })
        .unwrap();
        let Object::Fail(f) = graph.get(result.failure.unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(f.code, FailCode::CallBudget);
        assert_eq!(dispatcher.attempts().len(), 1);
    }

    #[test]
    fn seal_overrides_session_protection_and_rejects_expanded_task_budget() {
        let (_dir, mut graph, mut dispatcher, mut execution) = fixture(5);
        assert!(execution.session.protected.is_empty());
        let base = execution.session.baseline["f"];
        let response = serde_json::to_string(&Response {
            action: Action::EmitDelta {
                path: "f".into(),
                base_ref: base,
                patch_ref: base,
                result_ref: base,
                codec: DeltaCodec::Replacement,
                assumptions: vec![],
            },
        })
        .unwrap();
        // The caller cannot remove a protection sealed in the goal.
        let Object::Task(mut inflated) = graph.get(execution.session.task).unwrap() else {
            panic!()
        };
        inflated.token_budget += 1;
        let original = execution.session.task;
        execution.session.task = graph
            .insert(Authority::Runtime, &Object::Task(inflated))
            .unwrap();
        assert!(
            run_with(&mut graph, &mut dispatcher, execution, |_, _| panic!(
                "invalid task must not dispatch"
            ))
            .is_err()
        );
        assert!(dispatcher.attempts().is_empty());
        let (_dir2, mut graph2, mut dispatcher2, execution2) = fixture(5);
        let result = run_with(&mut graph2, &mut dispatcher2, execution2, |_, _| {
            reply(&response)
        })
        .unwrap();
        let Object::Fail(f) = graph2.get(result.failure.unwrap()).unwrap() else {
            panic!()
        };
        assert_eq!(f.code, FailCode::CapabilityDenied);
        assert!(graph.live(original).unwrap());
    }

    #[test]
    fn sealed_role_model_and_billing_cannot_change_at_dispatch() {
        for case in 0..3 {
            let (_dir, mut graph, mut dispatcher, mut execution) = fixture(5);
            match case {
                0 => execution.provider.model = "different-model".into(),
                1 => {
                    execution.provider.backend = Backend::AnthropicApi {
                        api_key_env: "NOT_READ".into(),
                    }
                }
                _ => execution.session.role = Role::Reviewer,
            }
            assert!(
                run_with(&mut graph, &mut dispatcher, execution, |_, _| panic!(
                    "sealed mismatch must not dispatch"
                ))
                .is_err()
            );
            assert!(dispatcher.attempts().is_empty());
        }
    }
}
