//! Validate planner IR against a pre-existing predicate registry and seal all
//! acceptance inputs. This module does not infer obligations from natural text.
use crate::Result;
use myr_core::*;
use myr_graph::Authority;
use myr_graph::Graph;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub procedure: ObjectRef,
    pub tolerance: Decimal,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub atom: ObjectRef,
    pub polarity: bool,
    pub binding: bool,
    pub measurement: Option<Measurement>,
}

/// Unresolved decisions remain proposals until a dependent task needs them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingAssumption {
    pub atom: ObjectRef,
    pub question: String,
    pub chosen: String,
    pub alternatives: Vec<String>,
    pub rationale: ObjectRef,
    pub artifacts: Vec<ObjectRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoalIr {
    pub goal: String,
    pub registry: Vec<ObjectRef>,
    pub criteria: Vec<Criterion>,
    pub assumptions: Vec<PendingAssumption>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealPolicy {
    pub providers: myr_adapter::config::RoleProviders,
    pub baseline: ObjectRef,
    pub protected: Vec<String>,
    /// Immutable runtime configuration, including argv, environment allowlist,
    /// tool versions, measurement/review procedures, and sandbox policy.
    pub verifier_policies: Vec<ObjectRef>,
    pub token_budget: u64,
    pub call_budget: u32,
    pub time_budget_ms: u64,
}

#[derive(Serialize)]
struct Envelope<'a> {
    format: &'static str,
    ir: &'a GoalIr,
    policy: &'a SealPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEnvelope {
    format: String,
    ir: GoalIr,
    policy: SealPolicy,
}

fn encode(ir: &GoalIr, policy: &SealPolicy) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&Envelope {
        format: "myr-goal-ir-v0",
        ir,
        policy,
    })?)
}

/// Revalidate an existing seal without creating objects or changing graph state.
pub fn load(graph: &Graph, reference: ObjectRef) -> Result<SealedGoal> {
    live(graph, reference, Kind::Goal)?;
    let bytes = graph.cas().get(reference)?;
    let stored: StoredEnvelope = serde_json::from_slice(&bytes)?;
    if stored.format != "myr-goal-ir-v0" {
        return Err(invalid("unsupported sealed goal format").into());
    }
    let (ir, policy) = validate(graph, stored.ir, stored.policy)?;
    if encode(&ir, &policy)? != bytes {
        return Err(invalid("sealed goal is not in canonical compiler encoding").into());
    }
    Ok(SealedGoal {
        reference,
        ir,
        policy,
    })
}

/// Runtime-owned task proposal. `decisions` selects unresolved ATOMs from the
/// sealed IR; it cannot change their chosen values or inject new assumptions.
pub struct TaskDraft {
    pub inputs: Vec<ObjectRef>,
    pub capabilities: Vec<String>,
    pub obligations: Vec<ObjectRef>,
    pub decisions: Vec<ObjectRef>,
    pub instruction: ObjectRef,
}

/// Materialize only requested assumptions and the dependent task in one graph
/// transaction. Reusing an invalidated assumption causes the transaction to fail.
pub fn issue_task(graph: &mut Graph, goal: ObjectRef, draft: TaskDraft) -> Result<ObjectRef> {
    let sealed = load(graph, goal)?;
    live(graph, draft.instruction, Kind::Artifact)?;
    for r in &draft.inputs {
        if !graph.live(*r)? {
            return Err(invalid("task input is unavailable").into());
        }
    }
    for capability in &draft.capabilities {
        let (operation, path) = capability
            .split_once(':')
            .ok_or_else(|| invalid("invalid task capability"))?;
        if !["read", "write"].contains(&operation) {
            return Err(invalid("unknown task capability").into());
        }
        validate_relative_path(path.trim_end_matches('/'))?;
    }
    for atom in &draft.obligations {
        if !sealed
            .ir
            .criteria
            .iter()
            .any(|c| c.atom == *atom && c.binding)
        {
            return Err(invalid("task obligation is not a sealed binding criterion").into());
        }
    }
    let mut batch = Vec::new();
    let mut assumptions = Vec::new();
    let mut seen = BTreeSet::new();
    for atom in &draft.decisions {
        if !seen.insert(*atom) {
            return Err(invalid("duplicate task decision").into());
        }
        let pending = sealed
            .ir
            .assumptions
            .iter()
            .find(|a| a.atom == *atom)
            .ok_or_else(|| invalid("task decision is absent from sealed assumptions"))?;
        let object = Object::Assumption(Assumption {
            scope: sealed.scope(),
            question: pending.question.clone(),
            chosen: pending.chosen.clone(),
            alternatives: pending.alternatives.clone(),
            rationale: pending.rationale,
            artifacts: pending.artifacts.clone(),
            invalidates: None,
        });
        let reference = ObjectRef::new(
            Kind::Assumption,
            myr_wire::message_cid(&myr_wire::encode(&object)?),
        );
        assumptions.push(reference);
        batch.push((Authority::Runtime, object));
    }
    let mut inputs = draft.inputs;
    inputs.extend(draft.decisions);
    batch.push((
        Authority::Runtime,
        Object::Task(Task {
            scope: sealed.scope(),
            inputs,
            capabilities: draft.capabilities,
            assumptions,
            obligations: draft.obligations,
            instruction: draft.instruction,
            token_budget: sealed.policy.token_budget,
            call_budget: sealed.policy.call_budget,
            time_budget_ms: sealed.policy.time_budget_ms,
        }),
    ));
    let refs = graph.insert_batch(&batch)?;
    refs.last()
        .copied()
        .ok_or_else(|| invalid("task transaction returned no objects").into())
}

/// No mutation API: changing any field requires compilation into another goal.
pub struct SealedGoal {
    reference: ObjectRef,
    ir: GoalIr,
    policy: SealPolicy,
}

impl SealedGoal {
    pub fn reference(&self) -> ObjectRef {
        self.reference
    }
    pub fn ir(&self) -> &GoalIr {
        &self.ir
    }
    pub fn policy(&self) -> &SealPolicy {
        &self.policy
    }
    pub fn scope(&self) -> Scope {
        Scope {
            goal: self.reference,
            snapshot: self.policy.baseline,
        }
    }
}

fn live(graph: &Graph, r: ObjectRef, kind: Kind) -> Result<()> {
    r.require(kind)?;
    if !graph.live(r)? {
        return Err(invalid("goal references an absent or inactive object").into());
    }
    Ok(())
}

pub fn compile(graph: &mut Graph, ir: GoalIr, policy: SealPolicy) -> Result<SealedGoal> {
    let (ir, policy) = validate(graph, ir, policy)?;
    let reference = graph.register_goal(&encode(&ir, &policy)?)?;
    Ok(SealedGoal {
        reference,
        ir,
        policy,
    })
}

fn validate(graph: &Graph, mut ir: GoalIr, mut policy: SealPolicy) -> Result<(GoalIr, SealPolicy)> {
    policy.providers.validate()?;
    if ir.goal.trim().is_empty()
        || ir.criteria.is_empty()
        || policy.verifier_policies.is_empty()
        || policy.token_budget == 0
        || policy.call_budget == 0
        || policy.time_budget_ms == 0
    {
        return Err(invalid(
            "goal requires text, acceptance criteria, verifiers, and positive budgets",
        )
        .into());
    }
    live(graph, policy.baseline, Kind::Artifact)?;
    let baseline_bytes = graph.cas().get(policy.baseline)?;
    let files: BTreeMap<String, ObjectRef> = serde_json::from_slice(&baseline_bytes)?;
    if serde_json::to_vec(&files)? != baseline_bytes {
        return Err(invalid("baseline manifest is not in the runtime encoding").into());
    }
    let mut tree = crate::views::Tree::new();
    for (path, r) in files {
        live(graph, r, Kind::Artifact)?;
        tree.insert(path, graph.cas().get(r)?);
    }
    crate::views::validate_tree(&tree)?;
    for path in &mut policy.protected {
        *path = path.trim_end_matches('/').to_owned();
        validate_relative_path(path)?;
    }
    policy.protected.sort();
    policy.protected.dedup();
    policy.verifier_policies.sort();
    policy.verifier_policies.dedup();
    for r in &policy.verifier_policies {
        live(graph, *r, Kind::Artifact)?;
        crate::verifier::load_policy(graph, *r, policy.time_budget_ms)?;
    }
    ir.registry.sort();
    if ir.registry.windows(2).any(|w| w[0] == w[1]) {
        return Err(invalid("duplicate predicate registry entry").into());
    }
    let mut names = BTreeSet::new();
    let mut registry = BTreeMap::new();
    for r in &ir.registry {
        live(graph, *r, Kind::PredicateDef)?;
        let Object::PredicateDef(def) = graph.get(*r)? else {
            unreachable!()
        };
        if !names.insert(def.name.clone()) {
            return Err(invalid("predicate registry name collision").into());
        }
        if def.name.starts_with("core.") && !core_predicates().contains(&def) {
            return Err(invalid("core predicate differs from closed registry").into());
        }
        if let Some(provenance) = def.provenance {
            live(graph, provenance, Kind::Artifact)?;
        }
        registry.insert(*r, def);
    }
    let class = |r: ObjectRef| -> Result<PredicateClass> {
        live(graph, r, Kind::Atom)?;
        let Object::Atom(atom) = graph.get(r)? else {
            unreachable!()
        };
        let def = registry
            .get(&atom.predicate)
            .ok_or_else(|| invalid("atom predicate absent from mission registry"))?;
        atom.validate_arguments(def)?;
        for arg in atom.arguments {
            if let Argument::Ref(reference) = arg
                && !graph.live(reference)?
            {
                return Err(invalid("goal atom has inactive argument").into());
            }
        }
        Ok(def.class)
    };
    let mut atoms = BTreeMap::new();
    for c in &mut ir.criteria {
        if atoms.insert(c.atom, c.polarity).is_some() {
            return Err(invalid("duplicate or contradictory acceptance atom").into());
        }
        let Object::Atom(atom) = graph.get(c.atom)? else {
            return Err(invalid("goal criterion must reference an ATOM").into());
        };
        if registry
            .get(&atom.predicate)
            .is_some_and(|p| p.name == "core.command_succeeds")
        {
            let [Argument::Ref(command)] = atom.arguments.as_slice() else {
                return Err(invalid("command criterion requires a policy reference").into());
            };
            if !policy.verifier_policies.contains(command)
                || !matches!(
                    crate::verifier::load_policy(graph, *command, policy.time_budget_ms)?,
                    crate::verifier::VerifierPolicy::Command { .. }
                )
            {
                return Err(invalid("command criterion must use a sealed command policy").into());
            }
        }
        match class(c.atom)? {
            PredicateClass::Unresolved => {
                return Err(
                    invalid("unresolved predicate must become a pending assumption").into(),
                );
            }
            PredicateClass::Heuristic if c.binding => {
                return Err(invalid("heuristic predicate cannot be binding").into());
            }
            PredicateClass::Empirical if c.measurement.is_none() => {
                return Err(
                    invalid("empirical criterion requires measurement and tolerance").into(),
                );
            }
            _ => {}
        }
        if let Some(m) = &mut c.measurement {
            if class(c.atom)? != PredicateClass::Empirical || m.tolerance.mantissa < 0 {
                return Err(invalid("invalid measurement class or negative tolerance").into());
            }
            live(graph, m.procedure, Kind::Artifact)?;
            m.tolerance.normalize()?;
            crate::measurement::load(graph, m.procedure, &policy)?;
        }
    }
    if !ir.criteria.iter().any(|c| c.binding) {
        return Err(invalid("goal has no binding acceptance obligation").into());
    }
    let mut pending = BTreeSet::new();
    for a in &ir.assumptions {
        if class(a.atom)? != PredicateClass::Unresolved
            || !pending.insert(a.atom)
            || a.question.trim().is_empty()
            || a.chosen.trim().is_empty()
            || a.alternatives.is_empty()
            || a.alternatives
                .iter()
                .any(|s| s.trim().is_empty() || s == &a.chosen)
        {
            return Err(invalid("invalid or duplicate pending assumption").into());
        }
        live(graph, a.rationale, Kind::Artifact)?;
        for r in &a.artifacts {
            live(graph, *r, Kind::Artifact)?;
        }
    }
    ir.criteria.sort_by_key(|c| c.atom);
    ir.assumptions.sort_by_key(|a| a.atom);
    Ok((ir, policy))
}
