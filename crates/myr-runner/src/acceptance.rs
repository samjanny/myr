//! Read-time goal acceptance audit. Not a terminal mission verdict: candidate
//! provenance and completed pipeline execution must also be established.
use crate::{Result, goal};
use myr_core::{Claim, Kind, Mechanism, Object, ObjectRef, Verdict, invalid};
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
    /// No active FACT, and live deterministic EVIDENCE contradicts the sealed
    /// obligation in its exact scope (Appendix B.3 rule 1 blocks promotion).
    Refuted,
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
    /// Live deterministic EVIDENCE contrary to this obligation. In a candidate
    /// audit only evidence bound to that candidate is retained here.
    pub refuting_evidence: Vec<ObjectRef>,
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

    /// Some binding obligation is contradicted by live deterministic evidence.
    /// This refutes one candidate; it is not by itself an UNSAT verdict.
    pub fn refuted_by_deterministic_evidence(&self) -> bool {
        self.obligations
            .iter()
            .any(|o| o.status == ObligationStatus::Refuted && !o.refuting_evidence.is_empty())
    }
}

/// Live deterministic EVIDENCE contrary to `claim`: contradiction of the claim
/// itself or support of the same ATOM with opposite polarity (Appendix B.1).
fn deterministic_refutations(graph: &Graph, claim: &Claim) -> Result<Vec<ObjectRef>> {
    let (positive, _) = myr_wire::identify(&Object::Claim(claim.clone()))?;
    let mut opposite = claim.clone();
    opposite.polarity = !claim.polarity;
    let (negative, _) = myr_wire::identify(&Object::Claim(opposite))?;
    let mut refutations = Vec::new();
    for (reference, contrary) in [
        (positive, Verdict::Contradicts),
        (negative, Verdict::Supports),
    ] {
        if graph.resolve(reference.cid).is_err() {
            continue;
        }
        for evidence_ref in graph.live_evidence(reference)? {
            let Object::Evidence(evidence) = graph.get(evidence_ref)? else {
                return Err(invalid("evidence index does not refer to EVIDENCE").into());
            };
            if evidence.mechanism == Mechanism::Deterministic
                && evidence.verdict == contrary
                && evidence.scope == claim.scope
            {
                refutations.push(evidence_ref);
            }
        }
    }
    refutations.sort();
    refutations.dedup();
    Ok(refutations)
}

/// Optional final-candidate binding applied to every EVIDENCE the audit relies on.
type Binder<'a> = Option<(&'a myr_cas::Store, ObjectRef)>;

fn bound(graph: &Graph, binder: Binder<'_>, reference: ObjectRef) -> Result<bool> {
    let Some((audit, candidate)) = binder else {
        return Ok(true);
    };
    let Object::Evidence(evidence) = graph.get(reference)? else {
        return Err(invalid("evidence reference does not refer to EVIDENCE").into());
    };
    match evidence.mechanism {
        Mechanism::Deterministic => {
            crate::command_verification::binds(graph, audit, candidate, &evidence)
        }
        Mechanism::LlmReview => crate::review::binds(graph, candidate, &evidence),
    }
}

/// Use the exact sealed atom, polarity, and scope. Never create a CLAIM, promote
/// a FACT, infer UNSAT from a failed criterion, or accept an agent's `finish`.
/// Errors (including CAS corruption) propagate instead of becoming success.
pub fn assess(graph: &Graph, goal_ref: ObjectRef) -> Result<Acceptance> {
    evaluate(graph, goal_ref, None)
}

fn evaluate(graph: &Graph, goal_ref: ObjectRef, binder: Binder<'_>) -> Result<Acceptance> {
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
        let sealed_claim = Claim {
            atom: criterion.atom,
            polarity: criterion.polarity,
            scope: sealed.scope(),
        };
        let (claim, _) = myr_wire::identify(&Object::Claim(sealed_claim.clone()))?;
        let fact = graph.active_fact(claim)?;
        let mut obligation = Obligation {
            atom: criterion.atom,
            polarity: criterion.polarity,
            claim,
            fact,
            status: ObligationStatus::MissingFact,
            unresolved_conflict: graph.unresolved_conflict(claim)?,
            inactive_dependencies: vec![],
            refuting_evidence: vec![],
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
            if !obligation.inactive_dependencies.is_empty() {
                obligation.status = ObligationStatus::InactiveDependency;
            } else {
                // Every EVIDENCE participating in the FACT must be bound to the
                // candidate; mixed-candidate proofs are rejected, never filtered.
                let mut all_bound = true;
                for evidence in &value.evidence {
                    if !bound(graph, binder, *evidence)? {
                        all_bound = false;
                        break;
                    }
                }
                if all_bound {
                    obligation.status = ObligationStatus::Proven;
                    provenance.extend(dependencies);
                } else {
                    obligation.status = ObligationStatus::UnboundEvidence;
                }
            }
        } else {
            for evidence in deterministic_refutations(graph, &sealed_claim)? {
                if bound(graph, binder, evidence)? {
                    obligation.refuting_evidence.push(evidence);
                }
            }
            if !obligation.refuting_evidence.is_empty() {
                obligation.status = ObligationStatus::Refuted;
                for evidence in &obligation.refuting_evidence {
                    provenance.extend(graph.dependencies(*evidence)?);
                }
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
/// or refutation must be bound to this exact candidate. Mixed-candidate proofs
/// are rejected, never silently filtered or re-promoted.
pub fn assess_candidate(
    graph: &Graph,
    audit: &myr_cas::Store,
    candidate_ref: ObjectRef,
) -> Result<Acceptance> {
    let manifest = crate::candidate::load_manifest(graph, candidate_ref)?;
    evaluate(graph, manifest.scope.goal, Some((audit, candidate_ref)))
}
