//! Typed numeric measurement contract. Parsing/evaluation alone is not EVIDENCE;
//! only a verified sandbox execution may supply a measurement observation.
use crate::{
    Result,
    goal::SealPolicy,
    verifier::{self, VerifierPolicy},
};
use myr_core::{Decimal, Kind, ObjectRef, invalid};
use myr_graph::Graph;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    AtMost,
    AtLeast,
    Within,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Procedure {
    pub format: String,
    pub command: ObjectRef,
    pub target: Decimal,
    pub relation: Relation,
    pub unit: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub format: String,
    pub value: Decimal,
    pub unit: String,
}

impl Procedure {
    pub fn encode(&self) -> Result<Vec<u8>> {
        self.command.require(Kind::Artifact)?;
        if self.format != "myr-measurement-procedure-v0"
            || self.unit.trim().is_empty()
            || self.unit.trim() != self.unit
            || self.unit.len() > 128
        {
            return Err(invalid("invalid measurement procedure format or unit").into());
        }
        let mut normalized = self.clone();
        normalized.target.normalize()?;
        Ok(serde_json::to_vec(&normalized)?)
    }
}

/// Resolve only explicitly sealed command policies; an arbitrary model artifact
/// cannot introduce another command, environment, image, or timeout.
pub fn load(graph: &Graph, reference: ObjectRef, policy: &SealPolicy) -> Result<Procedure> {
    reference.require(Kind::Artifact)?;
    if !graph.live(reference)? {
        return Err(invalid("measurement procedure is inactive").into());
    }
    let bytes = graph.cas().get(reference)?;
    let procedure: Procedure = serde_json::from_slice(&bytes)?;
    if procedure.encode()? != bytes || !policy.verifier_policies.contains(&procedure.command) {
        return Err(
            invalid("measurement procedure must be canonical and use a sealed command").into(),
        );
    }
    if !matches!(
        verifier::load_policy(graph, procedure.command, policy.time_budget_ms)?,
        VerifierPolicy::Command { .. }
    ) {
        return Err(invalid("measurement requires a command verifier").into());
    }
    Ok(procedure)
}

// Exact sign of three decimal terms. Huge exponent gaps never require huge
// allocations: a nonzero leading term with at least 21 decimal places exceeds
// the sum of the at most two remaining signed i64 coefficients (< 2 * 10^19).
fn sign(mut terms: [(i128, i64); 3]) -> Ordering {
    terms.sort_by_key(|term| std::cmp::Reverse(term.1));
    let (mut sum, mut exponent) = terms[0];
    for (coefficient, next_exponent) in terms.into_iter().skip(1) {
        if sum == 0 {
            sum = coefficient;
            exponent = next_exponent;
            continue;
        }
        let gap = i128::from(exponent) - i128::from(next_exponent);
        let digits = sum.unsigned_abs().to_string().len() as i128;
        if digits + gap >= 21 {
            return sum.cmp(&0);
        }
        sum = sum * 10i128.pow(gap as u32) + coefficient;
        exponent = next_exponent;
    }
    sum.cmp(&0)
}

/// Compare exact base-10 values. Tolerance widens at-most/at-least thresholds;
/// within means absolute distance from target <= tolerance. Polarity is applied
/// later by the evidence-producing runtime, not by the measurement program.
pub fn evaluate(
    procedure: &Procedure,
    tolerance: &Decimal,
    stdout: &[u8],
) -> Result<(Observation, bool)> {
    procedure.encode()?;
    if tolerance.mantissa < 0 || stdout.len() > 64 * 1024 {
        return Err(invalid("negative tolerance or oversized measurement output").into());
    }
    let mut observation: Observation = serde_json::from_slice(stdout)?;
    if observation.format != "myr-measurement-v0" || observation.unit != procedure.unit {
        return Err(invalid("measurement format or unit differs from procedure").into());
    }
    observation.value.normalize()?;
    let v = &observation.value;
    let t = &procedure.target;
    let upper = sign([
        (i128::from(v.mantissa), v.exponent),
        (-i128::from(t.mantissa), t.exponent),
        (-i128::from(tolerance.mantissa), tolerance.exponent),
    ]);
    let lower = sign([
        (i128::from(v.mantissa), v.exponent),
        (-i128::from(t.mantissa), t.exponent),
        (i128::from(tolerance.mantissa), tolerance.exponent),
    ]);
    let satisfied = match procedure.relation {
        Relation::AtMost => upper != Ordering::Greater,
        Relation::AtLeast => lower != Ordering::Less,
        Relation::Within => upper != Ordering::Greater && lower != Ordering::Less,
    };
    Ok((observation, satisfied))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn decimal(mantissa: i64, exponent: i64) -> Decimal {
        Decimal { mantissa, exponent }
    }
    fn procedure() -> Procedure {
        Procedure {
            format: "myr-measurement-procedure-v0".into(),
            command: ObjectRef::new(Kind::Artifact, myr_core::Cid([0; 32])),
            target: decimal(16, -3),
            relation: Relation::AtMost,
            unit: "seconds".into(),
        }
    }
    fn output(value: Decimal, unit: &str) -> Vec<u8> {
        serde_json::to_vec(&Observation {
            format: "myr-measurement-v0".into(),
            value,
            unit: unit.into(),
        })
        .unwrap()
    }
    #[test]
    fn tolerance_boundaries_are_exact_and_invalid_outputs_are_not_results() {
        let mut p = procedure();
        let tolerance = decimal(1, -3);
        assert!(
            evaluate(&p, &tolerance, &output(decimal(17, -3), "seconds"))
                .unwrap()
                .1
        );
        assert!(
            !evaluate(&p, &tolerance, &output(decimal(17000001, -9), "seconds"))
                .unwrap()
                .1
        );
        p.relation = Relation::Within;
        assert!(
            evaluate(&p, &tolerance, &output(decimal(15, -3), "seconds"))
                .unwrap()
                .1
        );
        assert!(
            !evaluate(&p, &tolerance, &output(decimal(14999999, -9), "seconds"))
                .unwrap()
                .1
        );
        p.relation = Relation::AtLeast;
        assert!(
            evaluate(&p, &tolerance, &output(decimal(15, -3), "seconds"))
                .unwrap()
                .1
        );
        assert!(evaluate(&p, &tolerance, &output(decimal(15, -3), "milliseconds")).is_err());
        assert!(evaluate(&p, &decimal(-1, 0), &output(decimal(1, 0), "seconds")).is_err());
        assert!(evaluate(&p, &tolerance, b"NaN").is_err());
        assert!(evaluate(&p, &tolerance, &vec![b' '; 65537]).is_err());
    }
    #[test]
    fn exact_sum_handles_extreme_exponents_cancellation_and_negative_minimum() {
        assert_eq!(
            sign([(1, i64::MAX), (-1, i64::MAX), (-1, i64::MIN)]),
            Ordering::Less
        );
        assert_eq!(
            sign([(1, i64::MAX), (-i128::from(i64::MAX), i64::MIN), (0, 0)]),
            Ordering::Greater
        );
        assert_eq!(
            sign([(i128::from(i64::MIN), 0), (i128::from(i64::MAX), 0), (1, 0)]),
            Ordering::Equal
        );
        for a in -12..=12i128 {
            for b in -12..=12i128 {
                for c in -12..=12i128 {
                    assert_eq!(
                        sign([(a, 2), (b, 1), (c, 0)]),
                        (100 * a + 10 * b + c).cmp(&0)
                    );
                }
            }
        }
    }
    #[test]
    fn procedure_cannot_execute_an_unsealed_command() {
        let mut f = crate::command_verification::tests::Fixture::new(true);
        let sealed = crate::goal::load(&f.graph, f.goal).unwrap();
        let mut p = procedure();
        let reference = f.graph.register_artifact(&p.encode().unwrap()).unwrap();
        assert!(load(&f.graph, reference, sealed.policy()).is_err());
        p.command = f.policy;
        let reference = f.graph.register_artifact(&p.encode().unwrap()).unwrap();
        assert!(load(&f.graph, reference, sealed.policy()).is_ok());
    }
}
