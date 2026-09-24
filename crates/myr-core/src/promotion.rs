//! Pure implementation of Appendix B. Callers supply runtime-validated evidence
//! and graph liveness; attestations deliberately have no input to this policy.
use crate::*;

pub struct EvidenceView<'a> {
    pub evidence: &'a Evidence,
    pub claim: &'a Claim,
    pub active: bool,
}

pub fn evaluate(
    claim: &Claim,
    evidence: &[EvidenceView<'_>],
    unresolved_conflict: bool,
) -> Option<u32> {
    let mut deterministic_support = false;
    let mut llm_contradiction = false;
    let mut lineages = Vec::new();
    for view in evidence {
        let e = view.evidence;
        if !view.active
            || view.claim.atom != claim.atom
            || view.claim.scope != claim.scope
            || e.scope != claim.scope
        {
            continue;
        }
        let supports = match e.verdict {
            Verdict::Inconclusive => continue,
            Verdict::Supports => view.claim.polarity == claim.polarity,
            Verdict::Contradicts => view.claim.polarity != claim.polarity,
        };
        match (e.mechanism, supports) {
            (Mechanism::Deterministic, false) => return None,
            (Mechanism::Deterministic, true) => deterministic_support = true,
            (Mechanism::LlmReview, false) => llm_contradiction = true,
            (Mechanism::LlmReview, true) => {
                if let Some(lineage) = e.lineage.as_ref().filter(|l| l.known()) {
                    lineages.push(lineage);
                }
            }
        }
    }
    let independent = lineages
        .iter()
        .enumerate()
        .any(|(i, a)| lineages[i + 1..].iter().any(|b| a.different_from(b)));
    if deterministic_support {
        Some(if llm_contradiction || unresolved_conflict {
            900_000
        } else if independent {
            970_000
        } else {
            950_000
        })
    } else if independent && !llm_contradiction && !unresolved_conflict {
        Some(800_000)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn r(kind: Kind, byte: u8) -> ObjectRef {
        ObjectRef::new(kind, Cid([byte; 32]))
    }
    fn claim() -> Claim {
        Claim {
            atom: r(Kind::Atom, 1),
            polarity: true,
            scope: Scope {
                goal: r(Kind::Goal, 2),
                snapshot: r(Kind::Artifact, 3),
            },
        }
    }
    fn evidence(mechanism: Mechanism, verdict: Verdict, provider: &str, family: &str) -> Evidence {
        Evidence {
            claim: r(Kind::Claim, 4),
            scope: claim().scope,
            verdict,
            mechanism,
            lineage: Some(Lineage {
                provider_id: provider.into(),
                family_id: family.into(),
                checkpoint_id: "v1".into(),
                procedure_id: "review-v0".into(),
            }),
            command: None,
            rationale: r(Kind::Artifact, 5),
            correlation_domains: vec![],
        }
    }
    #[test]
    fn policy_precedence_matrix() {
        // Exhaust all presences of deterministic/LLM support, opposition, conflict,
        // and a second independent reviewer: 64 combinations.
        let c = claim();
        for flags in 0..64 {
            let mut items = vec![];
            for (bit, mechanism, verdict, provider, family) in [
                (0, Mechanism::Deterministic, Verdict::Supports, "", ""),
                (1, Mechanism::Deterministic, Verdict::Contradicts, "", ""),
                (2, Mechanism::LlmReview, Verdict::Supports, "a", "a"),
                (3, Mechanism::LlmReview, Verdict::Supports, "b", "b"),
                (4, Mechanism::LlmReview, Verdict::Contradicts, "c", "c"),
            ] {
                if flags & (1 << bit) != 0 {
                    items.push(evidence(mechanism, verdict, provider, family));
                }
            }
            let conflict = flags & 32 != 0;
            let views: Vec<_> = items
                .iter()
                .map(|e| EvidenceView {
                    evidence: e,
                    claim: &c,
                    active: true,
                })
                .collect();
            let expected = if flags & 2 != 0 {
                None
            } else if flags & 1 != 0 {
                Some(if flags & 48 != 0 {
                    900_000
                } else if flags & 12 == 12 {
                    970_000
                } else {
                    950_000
                })
            } else if flags & 12 == 12 && flags & 48 == 0 {
                Some(800_000)
            } else {
                None
            };
            assert_eq!(evaluate(&c, &views, conflict), expected, "flags={flags}");
        }
    }
    #[test]
    fn opposite_polarity_support_is_counterevidence_and_scopes_are_exact() {
        let c = claim();
        let mut opposite = c.clone();
        opposite.polarity = false;
        let e = evidence(Mechanism::Deterministic, Verdict::Supports, "", "");
        let yes = EvidenceView {
            evidence: &e,
            claim: &c,
            active: true,
        };
        let no = EvidenceView {
            evidence: &e,
            claim: &opposite,
            active: true,
        };
        assert_eq!(evaluate(&c, &[yes, no], false), None);
        let mut unrelated = c.clone();
        unrelated.scope.snapshot = r(Kind::Artifact, 9);
        assert_eq!(
            evaluate(
                &c,
                &[EvidenceView {
                    evidence: &e,
                    claim: &unrelated,
                    active: true
                }],
                false
            ),
            None
        );
        assert_eq!(
            evaluate(
                &c,
                &[EvidenceView {
                    evidence: &e,
                    claim: &c,
                    active: false
                }],
                false
            ),
            None
        );
    }
    #[test]
    fn independence_requires_all_fields_and_both_dimensions() {
        let a = evidence(Mechanism::LlmReview, Verdict::Supports, "a", "x")
            .lineage
            .unwrap();
        for (provider, family, expected) in [
            ("b", "y", true),
            ("a", "y", false),
            ("b", "x", false),
            ("a", "x", false),
        ] {
            let mut b = a.clone();
            b.provider_id = provider.into();
            b.family_id = family.into();
            assert_eq!(a.different_from(&b), expected);
            b.checkpoint_id = "unknown".into();
            assert!(!a.different_from(&b));
            b.checkpoint_id = "v2".into();
            b.procedure_id.clear();
            assert!(!a.different_from(&b));
        }
    }
}
