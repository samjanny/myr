//! Budgeted pre-seal planner loop using the same dispatcher as later stages.
use crate::{
    accounting::{ReferenceTokenizer, Segment, SegmentKind},
    budget::{Budget, Limits},
    context::PreparedContext,
    dispatch::{Call, Dispatcher},
    goal::{SealPolicy, SealedGoal},
    mission::Mission,
    planner::{Catalog, Step},
};
use myr_adapter::{
    config::ProviderConfig,
    transport::{self, Completion, Request},
};
use myr_core::*;
use myr_graph::{Authority, Graph};
use std::time::{Duration, Instant};

pub struct Config {
    pub system: String,
    pub shared_schema: serde_json::Value,
    pub max_native_output_tokens: u32,
    pub max_reference_output_tokens: u64,
}

pub enum Outcome {
    Sealed {
        goal: Box<SealedGoal>,
        created: Vec<ObjectRef>,
    },
    Failed {
        failure: ObjectRef,
        created: Vec<ObjectRef>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("planner: {0}")]
    Runner(#[from] crate::Error),
    #[error("planner dispatch: {0}")]
    Dispatch(#[from] crate::dispatch::Error),
}

fn failure(
    graph: &mut Graph,
    created: Vec<ObjectRef>,
    code: FailCode,
    message: &str,
) -> Result<Outcome, Error> {
    let diagnostic = graph
        .register_artifact(message.as_bytes())
        .map_err(crate::Error::from)?;
    let failure = graph
        .insert(
            Authority::Runtime,
            &Object::Fail(Fail {
                class: code.class(),
                code,
                diagnostic,
                task: None,
            }),
        )
        .map_err(crate::Error::from)?;
    Ok(Outcome::Failed { failure, created })
}

fn catalog_live(graph: &Graph, catalog: &Catalog) -> Result<bool, Error> {
    match catalog.revalidate(graph) {
        Ok(()) => Ok(true),
        Err(crate::Error::Validation(_))
        | Err(crate::Error::Graph(myr_graph::Error::Unavailable(_))) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Executes the planner provider explicitly selected by runtime policy. The
/// supplied dispatcher is not replaced or reset when planning finishes.
pub fn run(
    graph: &mut Graph,
    dispatcher: &mut Dispatcher,
    mission: &Mission,
    policy: &SealPolicy,
    catalog: &mut Catalog,
    config: &Config,
) -> Result<Outcome, Error> {
    run_with(
        graph,
        dispatcher,
        mission,
        policy,
        catalog,
        config,
        transport::complete,
    )
}

pub(crate) fn run_with(
    graph: &mut Graph,
    dispatcher: &mut Dispatcher,
    mission: &Mission,
    policy: &SealPolicy,
    catalog: &mut Catalog,
    config: &Config,
    mut send: impl FnMut(&ProviderConfig, &Request) -> Result<Completion, transport::Error>,
) -> Result<Outcome, Error> {
    let started = Instant::now();
    policy.providers.validate().map_err(crate::Error::from)?;
    if config.max_native_output_tokens == 0 || config.max_reference_output_tokens == 0 {
        return Err(crate::Error::from(invalid("positive planner output limits required")).into());
    }
    let mut budget = Budget::new(Limits {
        calls: policy.call_budget,
        reference_tokens: policy.token_budget,
        elapsed: Duration::from_millis(policy.time_budget_ms),
    })
    .map_err(crate::dispatch::Error::Budget)?;
    let tokenizer = ReferenceTokenizer::new()?;
    policy
        .baseline
        .require(Kind::Artifact)
        .map_err(crate::Error::from)?;
    let baseline_bytes = graph
        .cas()
        .get(policy.baseline)
        .map_err(crate::Error::from)?;
    let baseline: std::collections::BTreeMap<String, ObjectRef> =
        serde_json::from_slice(&baseline_bytes).map_err(crate::Error::from)?;
    if serde_json::to_vec(&baseline).map_err(crate::Error::from)? != baseline_bytes {
        return Err(
            crate::Error::from(invalid("planner baseline manifest is not canonical")).into(),
        );
    }
    let repository: std::collections::BTreeSet<ObjectRef> = baseline.values().copied().collect();
    let mut history = vec![
        (
            SegmentKind::Goal,
            serde_json::to_string(mission).map_err(crate::Error::from)?,
        ),
        (
            SegmentKind::MwRender,
            catalog.description(graph)?.to_string(),
        ),
    ];
    let mut created = vec![];
    let mut cas_segments = vec![];
    let mut invalid = 0;
    loop {
        if !catalog_live(graph, catalog)? {
            return failure(
                graph,
                created,
                FailCode::InvalidatedReference,
                "Planner catalog became unavailable",
            );
        }
        let segments: Vec<_> = history
            .iter()
            .map(|(kind, text)| Segment { kind: *kind, text })
            .collect();
        let context = PreparedContext::new(&config.system, &segments)?
            .with_cas_prompt_segments(&cas_segments)?;
        let schema = catalog.response_schema(mission);
        let mut call = Call {
            agent: "planner",
            provider: &policy.providers.planner,
            context: &context,
            schema: &schema,
            shared_schema: &config.shared_schema,
            max_native_output_tokens: config.max_native_output_tokens,
            max_reference_output_tokens: config.max_reference_output_tokens,
            timeout: Duration::from_millis(policy.time_budget_ms),
        };
        let input = dispatcher.preview_tokens(&call)?;
        let permit = match budget.reserve(input, config.max_reference_output_tokens) {
            Ok(p) => p,
            Err(code) => return failure(graph, created, code, "Planner budget exhausted"),
        };
        call.timeout = permit.timeout();
        call.max_reference_output_tokens = permit.output_reference_allowance();
        let completion = match dispatcher.complete_with(call, &mut send) {
            Ok(completion) => completion,
            Err(crate::dispatch::Error::Budget(code)) => {
                let _ = budget.settle(permit, None);
                return failure(
                    graph,
                    created,
                    code,
                    "Mission budget exhausted during planning",
                );
            }
            Err(crate::dispatch::Error::Transport(error)) => {
                let _ = budget.settle(permit, None);
                return if error.is_agent_output_failure() {
                    failure(
                        graph,
                        created,
                        FailCode::InvalidAgentOutput,
                        "Planner provider exhausted its structured-output attempts",
                    )
                } else {
                    failure(
                        graph,
                        created,
                        FailCode::ProviderUnavailable,
                        "Planner provider failed; no billing fallback attempted",
                    )
                };
            }
            Err(error) => return Err(error.into()),
        };
        if let Err(code) = budget.settle(
            permit,
            std::str::from_utf8(&completion.raw_output)
                .ok()
                .map(|text| tokenizer.count(text)),
        ) {
            return failure(
                graph,
                created,
                code,
                "Planner output exceeded its budget or was not accountable",
            );
        }
        if !catalog_live(graph, catalog)? {
            return failure(
                graph,
                created,
                FailCode::InvalidatedReference,
                "Planner catalog became unavailable",
            );
        }
        if matches!(catalog.access_allowed(&completion.raw_output), Ok(false)) {
            return failure(
                graph,
                created,
                FailCode::CapabilityDenied,
                "Planner attempted an out-of-catalog reference",
            );
        }
        match catalog.handle_response(graph, mission, policy, &completion.raw_output) {
            Ok(Step::Created(reference)) => {
                dispatcher.record_artifact_origin("planner", reference);
                created.push(reference);
                history.push((
                    SegmentKind::LocalTool,
                    serde_json::json!({"created":reference}).to_string(),
                ));
            }
            Ok(Step::FetchedMany(items)) => {
                for (reference, bytes) in items {
                    dispatcher.record_cas_read(bytes.len() as u64)?;
                    let (kind, value) = if reference.kind == Kind::Artifact {
                        (
                            dispatcher.classify_artifact(
                                "planner",
                                reference,
                                &repository,
                                SegmentKind::CasReferenced,
                            ),
                            myr_adapter::render_artifact(reference, &bytes).to_string(),
                        )
                    } else {
                        (
                            SegmentKind::MwRender,
                            myr_wire::render(&graph.get(reference).map_err(crate::Error::from)?)
                                .map_err(crate::Error::from)?,
                        )
                    };
                    cas_segments.push(history.len());
                    history.push((kind, value));
                }
            }
            Ok(Step::Fetched { reference, bytes }) => {
                dispatcher.record_cas_read(bytes.len() as u64)?;
                let (kind, value) = if reference.kind == Kind::Artifact {
                    (
                        dispatcher.classify_artifact(
                            "planner",
                            reference,
                            &repository,
                            SegmentKind::CasReferenced,
                        ),
                        myr_adapter::render_artifact(reference, &bytes).to_string(),
                    )
                } else {
                    (
                        SegmentKind::MwRender,
                        myr_wire::render(&graph.get(reference).map_err(crate::Error::from)?)
                            .map_err(crate::Error::from)?,
                    )
                };
                cas_segments.push(history.len());
                history.push((kind, value));
            }
            Ok(Step::Sealed(goal)) => {
                if started.elapsed() >= Duration::from_millis(policy.time_budget_ms) {
                    return failure(
                        graph,
                        created,
                        FailCode::TimeBudget,
                        "Planner deadline expired before seal acceptance",
                    );
                }
                return Ok(Outcome::Sealed { goal, created });
            }
            Err(error) => {
                let repairable = matches!(
                    error,
                    crate::Error::Validation(_)
                        | crate::Error::Json(_)
                        | crate::Error::Graph(myr_graph::Error::Validation(_))
                );
                if !repairable {
                    return Err(error.into());
                }
                invalid += 1;
                if invalid > 2 {
                    return failure(
                        graph,
                        created,
                        FailCode::InvalidAgentOutput,
                        "Planner exceeded two repair attempts",
                    );
                }
                history.push((SegmentKind::ValidationFeedback, format!("Invalid planner response: {error}. Return an action matching the current schema and mission constraints.")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounting::Pipeline, command_verification::tests::Fixture, goal};
    fn config() -> Config {
        Config {
            system: "Use registered predicates and submit the unchanged user goal.".into(),
            shared_schema: myr_adapter::schema::response_schema(myr_adapter::Role::Planner),
            max_native_output_tokens: 1024,
            max_reference_output_tokens: 4096,
        }
    }
    fn reply(value: serde_json::Value) -> Result<Completion, transport::Error> {
        Ok(Completion {
            raw_output: serde_json::to_vec(&value).unwrap(),
            usage: Default::default(),
            observed_model: Some("fixture".into()),
        })
    }
    fn mission() -> Mission {
        Mission {
            goal: "Fixture command".into(),
            verify: vec![vec!["check".into()]],
            protected: vec![],
        }
    }
    fn dispatcher(f: &Fixture, calls: u32) -> Dispatcher {
        Dispatcher::new(
            Limits {
                calls,
                reference_tokens: 1000000,
                elapsed: Duration::from_secs(30),
            },
            Pipeline::Mw0,
            f.audit.clone(),
        )
        .unwrap()
    }

    #[test]
    fn planner_fetches_defines_and_seals_without_resetting_shared_budget() {
        let mut f = Fixture::new(true);
        let sealed = goal::load(&f.graph, f.goal).unwrap();
        let baseline: std::collections::BTreeMap<String, ObjectRef> =
            serde_json::from_slice(&f.graph.cas().get(sealed.policy().baseline).unwrap()).unwrap();
        let file = baseline["f"];
        let mut catalog =
            Catalog::new(&f.graph, &sealed.ir().registry, &[], &[f.policy, file]).unwrap();
        let mut dispatcher = dispatcher(&f, 3);
        let mut calls = 0;
        let outcome = run_with(&mut f.graph, &mut dispatcher, &mission(), sealed.policy(), &mut catalog, &config(), |provider, request| {
            assert_eq!(provider, &sealed.policy().providers.planner);
            calls += 1;
            match calls {
                1 => reply(serde_json::json!({"action":{"tool":"fetch","arguments":{"reference":file}}})),
                2 => {
                    assert!(request.prompt.contains(r#""content_text":"baseline""#));
                    reply(serde_json::json!({"action":{"tool":"define_atom","arguments":{"predicate_ref":sealed.ir().registry[0],
                        "arguments":[{"type":"ref","value":f.policy}]}}}))
                }
                3 => {
                    assert!(request.prompt.contains(&f.atom.cid.to_string()));
                    // The schema no longer enumerates catalog CIDs (cost pilot B).
                    assert!(!request.schema.to_string().contains(&f.atom.cid.to_string()));
                    reply(serde_json::json!({"action":{"tool":"submit_goal","arguments":sealed.ir()}}))
                }
                _ => panic!("unexpected request"),
            }
        }).unwrap();
        let Outcome::Sealed { goal, created } = outcome else {
            panic!()
        };
        assert_eq!(goal.reference(), f.goal);
        assert_eq!(created, vec![f.atom]);
        assert_eq!(calls, 3);
        assert_eq!(
            dispatcher.accounting().totals().cas_raw_bytes,
            b"baseline".len() as u64
        );
        assert!(
            dispatcher.accounting().calls()[1]
                .segments
                .iter()
                .any(|s| s.kind == SegmentKind::DirectRepo && s.cas_derived && !s.communication)
        );
        let retrieved: Vec<_> = dispatcher
            .accounting()
            .calls()
            .iter()
            .flat_map(|call| &call.segments)
            .filter(|s| s.cas_derived)
            .collect();
        assert_eq!(retrieved.len(), 2);
        assert_eq!(
            dispatcher.accounting().totals().cas_context_bytes,
            retrieved.iter().map(|s| s.utf8_bytes).sum::<u64>()
        );
        assert!(dispatcher.last_checkpoint().is_some());
        let outcome = run_with(
            &mut f.graph,
            &mut dispatcher,
            &mission(),
            sealed.policy(),
            &mut catalog,
            &config(),
            |_, _| panic!("shared call budget must not reset"),
        )
        .unwrap();
        let Outcome::Failed { failure, .. } = outcome else {
            panic!()
        };
        let Object::Fail(failure) = f.graph.get(failure).unwrap() else {
            panic!()
        };
        assert_eq!(failure.code, FailCode::CallBudget);
        assert_eq!(dispatcher.budget().calls_started, 3);
    }

    #[test]
    fn planner_repairs_are_bounded_and_policy_transport_failures_do_not_retry() {
        for (mode, expected_calls, expected_code) in [
            ("invalid", 3, FailCode::InvalidAgentOutput),
            ("denied", 1, FailCode::CapabilityDenied),
            ("network", 1, FailCode::ProviderUnavailable),
            ("local_budget", 1, FailCode::CallBudget),
        ] {
            let mut f = Fixture::new(true);
            let sealed = goal::load(&f.graph, f.goal).unwrap();
            let mut policy = sealed.policy().clone();
            if mode == "local_budget" {
                policy.call_budget = 1;
            }
            let mut catalog =
                Catalog::new(&f.graph, &sealed.ir().registry, &[], &[f.policy]).unwrap();
            let outside = f.graph.register_artifact(b"not accessible").unwrap();
            let mut dispatcher = dispatcher(&f, 10);
            let mut calls = 0;
            let outcome = run_with(&mut f.graph, &mut dispatcher, &mission(), &policy, &mut catalog, &config(), |_,_| {
                calls += 1;
                match mode {
                    "denied" => reply(serde_json::json!({"action":{"tool":"fetch","arguments":{"reference":outside}}})),
                    "network" => Err(transport::Error::Network),
                    _ => Ok(Completion { raw_output: b"invalid".to_vec(), usage: Default::default(), observed_model: None }),
                }
            }).unwrap();
            let Outcome::Failed { failure, created } = outcome else {
                panic!()
            };
            assert!(created.is_empty());
            let Object::Fail(failure) = f.graph.get(failure).unwrap() else {
                panic!()
            };
            assert_eq!(failure.code, expected_code);
            assert_eq!(calls, expected_calls);
            assert_eq!(dispatcher.budget().calls_started, expected_calls);
            if mode == "invalid" {
                assert!(
                    dispatcher.accounting().calls()[1]
                        .segments
                        .iter()
                        .any(|s| s.kind == SegmentKind::ValidationFeedback)
                );
                let raw = dispatcher.attempts()[0].output.unwrap();
                assert_eq!(f.audit.get(raw).unwrap(), b"invalid");
            }
        }
    }
}
