use myr_core::*;

pub fn r(kind: Kind, byte: u8) -> ObjectRef {
    ObjectRef::new(kind, Cid([byte; 32]))
}
pub fn scope() -> Scope {
    Scope {
        goal: r(Kind::Goal, 1),
        snapshot: r(Kind::Artifact, 2),
    }
}
pub fn task() -> Object {
    Object::Task(Task {
        scope: scope(),
        inputs: vec![],
        capabilities: vec!["write:src/".into(), "read:src/".into()],
        assumptions: vec![],
        obligations: vec![],
        instruction: r(Kind::Artifact, 3),
        token_budget: 2000,
        call_budget: 4,
        time_budget_ms: 30000,
    })
}
pub fn objects() -> Vec<Object> {
    vec![
        task(),
        Object::Claim(Claim {
            atom: r(Kind::Atom, 4),
            polarity: true,
            scope: scope(),
        }),
        Object::Evidence(Box::new(Evidence {
            claim: r(Kind::Claim, 5),
            scope: scope(),
            verdict: Verdict::Supports,
            mechanism: Mechanism::LlmReview,
            lineage: Some(Lineage {
                provider_id: "openai".into(),
                family_id: "gpt".into(),
                checkpoint_id: "configured".into(),
                procedure_id: "review-v0".into(),
            }),
            command: None,
            rationale: r(Kind::Artifact, 6),
            correlation_domains: vec![],
        })),
        Object::Fact(Fact {
            claim: r(Kind::Claim, 5),
            evidence: vec![r(Kind::Evidence, 7)],
            confidence_ppm: 950000,
            policy: "promotion-policy-v0".into(),
            assumptions: vec![],
        }),
        Object::Assumption(Assumption {
            scope: scope(),
            question: "Keep quirks?".into(),
            chosen: "yes".into(),
            alternatives: vec!["no".into()],
            rationale: r(Kind::Artifact, 6),
            artifacts: vec![],
            invalidates: None,
        }),
        Object::Delta(Delta {
            scope: scope(),
            path: "src/lib.rs".into(),
            base: r(Kind::Artifact, 8),
            patch: r(Kind::Artifact, 9),
            result: r(Kind::Artifact, 10),
            codec: DeltaCodec::Replacement,
            assumptions: vec![],
        }),
        Object::Fail(Fail {
            class: FailClass::Validation,
            code: FailCode::InvalidAgentOutput,
            diagnostic: r(Kind::Artifact, 6),
            task: None,
        }),
        Object::Attest(Attest {
            claim: r(Kind::Claim, 5),
            task: r(Kind::Task, 11),
            agent: "worker".into(),
            lineage: Lineage {
                provider_id: "anthropic".into(),
                family_id: "claude".into(),
                checkpoint_id: "configured".into(),
                procedure_id: "worker-v0".into(),
            },
        }),
        Object::Atom(Atom {
            predicate: r(Kind::PredicateDef, 12),
            arguments: vec![
                Argument::Text("cafe\u{301}\r\n".into()),
                Argument::Decimal(Decimal {
                    mantissa: 1200,
                    exponent: -3,
                }),
            ],
        }),
        Object::PredicateDef(core_predicates().remove(0)),
    ]
}
