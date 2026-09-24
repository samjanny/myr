//! Runtime candidate binding for the two sealed LLM reviewer slots.
use crate::{Result, candidate, goal};
use myr_adapter::{
    ReviewRationale,
    config::{ProviderConfig, RoleSlot},
};
use myr_core::*;
use myr_graph::Graph;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewContext {
    pub format: String,
    pub candidate: ObjectRef,
    pub slot: RoleSlot,
    pub provider: ProviderConfig,
    pub instruction: ObjectRef,
}

pub fn create_context(
    graph: &mut Graph,
    candidate_ref: ObjectRef,
    slot: RoleSlot,
    instruction: ObjectRef,
) -> Result<ObjectRef> {
    if !matches!(slot, RoleSlot::ReviewerA | RoleSlot::ReviewerB) {
        return Err(invalid("reviewer slot required").into());
    }
    instruction.require(Kind::Artifact)?;
    let manifest = candidate::load_manifest(graph, candidate_ref)?;
    let sealed = goal::load(graph, manifest.scope.goal)?;
    let context = ReviewContext {
        format: "myr-review-context-v0".into(),
        candidate: candidate_ref,
        slot,
        provider: sealed.policy().providers.get(slot).clone(),
        instruction,
    };
    Ok(graph.register_artifact_with_dependencies(
        &serde_json::to_vec(&context)?,
        &[candidate_ref, instruction],
    )?)
}

pub fn load_context(graph: &Graph, reference: ObjectRef) -> Result<ReviewContext> {
    reference.require(Kind::Artifact)?;
    if !graph.live(reference)? {
        return Err(invalid("review context is inactive").into());
    }
    let bytes = graph.cas().get(reference)?;
    let context: ReviewContext = serde_json::from_slice(&bytes)?;
    if context.format != "myr-review-context-v0"
        || serde_json::to_vec(&context)? != bytes
        || !matches!(context.slot, RoleSlot::ReviewerA | RoleSlot::ReviewerB)
    {
        return Err(invalid("review context is not canonical").into());
    }
    let manifest = candidate::load_manifest(graph, context.candidate)?;
    let sealed = goal::load(graph, manifest.scope.goal)?;
    if context.provider != *sealed.policy().providers.get(context.slot) {
        return Err(invalid("review provider differs from seal").into());
    }
    let dependencies = graph.dependencies(reference)?;
    for dependency in [context.candidate, context.instruction] {
        dependency.require(Kind::Artifact)?;
        if !dependencies.contains(&dependency) || !graph.live(dependency)? {
            return Err(invalid("review context provenance is absent or inactive").into());
        }
    }
    Ok(context)
}

pub(crate) fn validate_task(
    graph: &Graph,
    reference: ObjectRef,
    task_ref: ObjectRef,
    slot: RoleSlot,
    provider: &ProviderConfig,
) -> Result<()> {
    let context = load_context(graph, reference)?;
    let Object::Task(task) = graph.get(task_ref)? else {
        return Err(invalid("review TASK required").into());
    };
    let manifest = candidate::load_manifest(graph, context.candidate)?;
    if context.slot != slot
        || context.provider != *provider
        || task.scope != manifest.scope
        || task.instruction != context.instruction
    {
        return Err(invalid("review context differs from task, role or provider").into());
    }
    graph.fetch(&[task_ref], reference)?;
    Ok(())
}

pub(crate) fn binds(graph: &Graph, candidate_ref: ObjectRef, evidence: &Evidence) -> Result<bool> {
    if evidence.mechanism != Mechanism::LlmReview || evidence.command.is_some() {
        return Ok(false);
    }
    let bytes = graph.cas().get(evidence.rationale)?;
    let Ok(wrapper) = serde_json::from_slice::<ReviewRationale>(&bytes) else {
        return Ok(false);
    };
    if wrapper.format != "myr-review-rationale-v0"
        || serde_json::to_vec(&wrapper)? != bytes
        || wrapper.claim != evidence.claim
        || wrapper.verdict != evidence.verdict
    {
        return Ok(false);
    }
    let dependencies = graph.dependencies(evidence.rationale)?;
    for dependency in [
        wrapper.context,
        wrapper.task,
        wrapper.claim,
        wrapper.rationale,
    ] {
        if !dependencies.contains(&dependency) || !graph.live(dependency)? {
            return Ok(false);
        }
    }
    let context = load_context(graph, wrapper.context)?;
    if context.candidate != candidate_ref
        || evidence.lineage.as_ref() != Some(&context.provider.lineage)
    {
        return Ok(false);
    }
    validate_task(
        graph,
        wrapper.context,
        wrapper.task,
        context.slot,
        &context.provider,
    )?;
    let Object::Claim(claim) = graph.get(wrapper.claim)? else {
        return Ok(false);
    };
    let manifest = candidate::load_manifest(graph, candidate_ref)?;
    Ok(evidence.scope == manifest.scope && claim.scope == manifest.scope)
}
