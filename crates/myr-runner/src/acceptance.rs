//! Read-time goal acceptance audit. Not a terminal mission verdict: candidate
//! provenance and completed pipeline execution must also be established.
use crate::{Result, goal};
use myr_core::{Claim, Kind, Object, ObjectRef, invalid};
use myr_graph::Graph;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObligationStatus {
    Proven,
    MissingFact,
    InactiveDependency,
    UnboundEvidence,
}

#[derive(Clone, Debug, Serialize)]
pub struct Obligation {
    pub atom: ObjectRef,
    pub polarity: bool,
    pub claim: ObjectRef,
    pub fact: Option<ObjectRef>,
    pub status: ObligationStatus,
    pub unresolved_conflict: bool,
    pub inactive_dependencies: Vec<ObjectRef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Acceptance {
    pub goal: ObjectRef,
    pub obligations: Vec<Obligation>,
    pub evidence: Vec<ObjectRef>,
    pub active_assumptions: Vec<ObjectRef>,
    pub artifacts: Vec<ObjectRef>,
    pub provenance: Vec<ObjectRef>,
}

impl Acceptance {
    /// Necessary, not sufficient, for COMPLETE. In particular, this audit does
    /// not prove the evidence was generated against the final candidate tree.
    pub fn all_binding_proven(&self) -> bool {
        !self.obligations.is_empty()
            && self
                .obligations
                .iter()
                .all(|o| o.status == ObligationStatus::Proven)
    }
}

/// Use the exact sealed atom, polarity, and scope. Never create a CLAIM, promote
/// a FACT, infer UNSAT from a failed criterion, or accept an agent's `finish`.
/// Errors (including CAS corruption) propagate instead of becoming success.
pub fn assess(graph: &Graph, goal_ref: ObjectRef) -> Result<Acceptance> {
    let sealed = goal::load(graph, goal_ref)?;
    let mut result = Acceptance {
        goal: goal_ref,
        obligations: vec![],
        evidence: vec![],
        active_assumptions: vec![],
        artifacts: vec![],
        provenance: vec![],
    };
    let mut provenance = BTreeSet::new();
    for criterion in sealed.ir().criteria.iter().filter(|c| c.binding) {
        let (claim, _) = myr_wire::identify(&Object::Claim(Claim {
            atom: criterion.atom,
            polarity: criterion.polarity,
            scope: sealed.scope(),
        }))?;
        let fact = graph.active_fact(claim)?;
        let mut obligation = Obligation {
            atom: criterion.atom,
            polarity: criterion.polarity,
            claim,
            fact,
            status: ObligationStatus::MissingFact,
            unresolved_conflict: graph.unresolved_conflict(claim)?,
            inactive_dependencies: vec![],
        };
        if let Some(reference) = fact {
            let Object::Fact(value) = graph.get(reference)? else {
                return Err(invalid("active fact index does not refer to FACT").into());
            };
            if value.claim != claim {
                return Err(invalid("active fact does not match sealed obligation").into());
            }
            let dependencies = graph.dependencies(reference)?;
            for dependency in &dependencies {
                if !graph.live(*dependency)? {
                    obligation.inactive_dependencies.push(*dependency);
                }
            }
            if obligation.inactive_dependencies.is_empty() {
                obligation.status = ObligationStatus::Proven;
                provenance.extend(dependencies);
            } else {
                obligation.status = ObligationStatus::InactiveDependency;
            }
        }
        result.obligations.push(obligation);
    }
    for reference in provenance {
        match reference.kind {
            Kind::Evidence => result.evidence.push(reference),
            Kind::Assumption => {
                let Object::Assumption(assumption) = graph.get(reference)? else {
                    return Err(invalid("assumption dependency has wrong kind").into());
                };
                if assumption.invalidates.is_none() {
                    result.active_assumptions.push(reference);
                }
            }
            Kind::Artifact => result.artifacts.push(reference),
            _ => {}
        }
        result.provenance.push(reference);
    }
    Ok(result)
}

/// Conservative final-candidate gate: every EVIDENCE participating in each FACT
/// must be bound to this exact candidate. Mixed-candidate proofs are rejected,
/// never silently filtered or re-promoted.
pub fn assess_candidate(
    graph: &Graph,
    audit: &myr_cas::Store,
    candidate_ref: ObjectRef,
) -> Result<Acceptance> {
    let manifest = crate::candidate::load_manifest(graph, candidate_ref)?;
    let mut result = assess(graph, manifest.scope.goal)?;
    for obligation in &mut result.obligations {
        if obligation.status != ObligationStatus::Proven {
            continue;
        }
        let Object::Fact(fact) = graph.get(obligation.fact.expect("proven FACT"))? else {
            unreachable!()
        };
        for reference in fact.evidence {
            let Object::Evidence(evidence) = graph.get(reference)? else {
                unreachable!()
            };
            let bound = match evidence.mechanism {
                myr_core::Mechanism::Deterministic => {
                    crate::command_verification::binds(graph, audit, candidate_ref, &evidence)?
                }
                myr_core::Mechanism::LlmReview => {
                    crate::review::binds(graph, candidate_ref, &evidence)?
                }
            };
            if !bound {
                obligation.status = ObligationStatus::UnboundEvidence;
                break;
            }
        }
    }
    Ok(result)
}
