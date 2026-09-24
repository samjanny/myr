//! Join benchmark receipts to a verified schedule without dropping missing pairs.
use crate::{
    Result,
    schedule::{self, Condition, Pipeline, Schedule},
    statistics,
};
use myr_core::{Cid, invalid};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnavailableReason {
    Infrastructure,
    Provider,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Measured {
        run: statistics::Run,
    },
    Unavailable {
        reason: UnavailableReason,
        diagnostic: Cid,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub schedule_cid: Cid,
    pub job_id: Cid,
    pub run_record: Cid,
    pub outcome: Outcome,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub schedule: Schedule,
    pub receipts: Vec<Receipt>,
    pub bootstrap_samples: u32,
    pub tail_ppm: u32,
    pub bootstrap_seed: u64,
}

#[derive(Debug, Serialize)]
pub struct Coverage {
    pub expected_jobs: usize,
    pub received_jobs: usize,
    pub missing_job_ids: Vec<Cid>,
    pub expected_pairs: usize,
    pub evaluable_pairs: usize,
    pub unavailable_pairs: usize,
    pub pending_pairs: usize,
    /// Known infrastructure/provider/INVALID pairs only; pending jobs are not failures.
    pub unavailable_pairs_exceed_ten_percent: bool,
}

#[derive(Debug, Serialize)]
pub struct Collection {
    pub format: String,
    pub official_acceptance_checked: bool,
    pub receipt_provenance_checked: bool,
    pub schedule_cid: Cid,
    pub receipts_cid: Cid,
    pub coverage: Coverage,
    /// Absent unless every planned pair is evaluable. No complete-case deletion.
    pub analysis: Option<statistics::Analysis>,
}

fn run(outcome: &Outcome) -> Option<&statistics::Run> {
    match outcome {
        Outcome::Measured { run } if run.oracle != statistics::Oracle::Invalid => Some(run),
        _ => None,
    }
}

pub fn collect(input: &Input) -> Result<Collection> {
    schedule::verify(&input.schedule)?;
    if !(1000..=100_000).contains(&input.bootstrap_samples)
        || !(1..=100_000).contains(&input.tail_ppm)
    {
        return Err(invalid("invalid bootstrap parameters").into());
    }
    let jobs: BTreeMap<_, _> = input
        .schedule
        .jobs
        .iter()
        .map(|job| (job.id, job))
        .collect();
    let mut receipts = BTreeMap::new();
    let mut measurements = BTreeMap::new();
    for receipt in &input.receipts {
        let job = jobs
            .get(&receipt.job_id)
            .ok_or_else(|| invalid("receipt refers to an unplanned job"))?;
        if receipt.schedule_cid != input.schedule.schedule_cid
            || receipts.insert(receipt.job_id, receipt).is_some()
        {
            return Err(invalid("receipt belongs to another schedule or duplicates a job").into());
        }
        measurements.insert(
            (
                job.case_id.as_str(),
                job.repetition,
                job.pipeline,
                job.condition,
            ),
            &receipt.outcome,
        );
    }
    let mut coverage = Coverage {
        expected_jobs: jobs.len(),
        received_jobs: receipts.len(),
        missing_job_ids: jobs
            .keys()
            .filter(|id| !receipts.contains_key(id))
            .copied()
            .collect(),
        expected_pairs: jobs.len() / 2,
        evaluable_pairs: 0,
        unavailable_pairs: 0,
        pending_pairs: 0,
        unavailable_pairs_exceed_ten_percent: false,
    };
    let mut cases = Vec::new();
    for case in &input.schedule.input.cases {
        let mut repetitions = Vec::new();
        for repetition in 0..5 {
            let mut pair_data = Vec::new();
            for pipeline in [Pipeline::Prose, Pipeline::Mw0] {
                let clean = measurements
                    .get(&(case.id.as_str(), repetition, pipeline, Condition::Clean))
                    .copied();
                let poisoned = measurements
                    .get(&(case.id.as_str(), repetition, pipeline, Condition::Poisoned))
                    .copied();
                if [clean, poisoned]
                    .into_iter()
                    .flatten()
                    .any(|outcome| run(outcome).is_none())
                {
                    coverage.unavailable_pairs += 1;
                } else if let (Some(clean), Some(poisoned)) =
                    (clean.and_then(run), poisoned.and_then(run))
                {
                    coverage.evaluable_pairs += 1;
                    pair_data.push(statistics::Pair {
                        clean: clean.clone(),
                        poisoned: poisoned.clone(),
                    });
                } else {
                    coverage.pending_pairs += 1;
                }
            }
            if pair_data.len() == 2 {
                let myr = pair_data.pop().expect("two pairs");
                let prose = pair_data.pop().expect("two pairs");
                repetitions.push(statistics::Repetition { prose, myr });
            }
        }
        cases.push(statistics::Case {
            id: case.id.clone(),
            poison_type: case.poison_type.clone(),
            repetitions,
        });
    }
    coverage.unavailable_pairs_exceed_ten_percent =
        coverage.unavailable_pairs * 10 > coverage.expected_pairs;
    let analysis = if coverage.evaluable_pairs == coverage.expected_pairs {
        Some(statistics::analyze(&statistics::Input {
            cases,
            bootstrap_samples: input.bootstrap_samples,
            tail_ppm: input.tail_ppm,
            seed: input.bootstrap_seed,
        })?)
    } else {
        None
    };
    let ordered: Vec<_> = receipts.values().copied().collect();
    Ok(Collection {
        format: "myr-benchmark-collection-v0".into(),
        official_acceptance_checked: false,
        receipt_provenance_checked: false,
        schedule_cid: input.schedule.schedule_cid,
        receipts_cid: myr_wire::artifact_cid(&serde_json::to_vec(&ordered)?),
        coverage,
        analysis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Input {
        let schedule = schedule::plan(&schedule::Input {
            cases: (0..8)
                .map(|i| schedule::Case {
                    id: format!("case-{i}"),
                    poison_type: format!("type-{}", i % 3),
                    case_manifest: Cid([i; 32]),
                })
                .collect(),
            configuration: Cid([99; 32]),
            seed: 42,
        })
        .unwrap();
        let receipts = schedule
            .jobs
            .iter()
            .map(|job| Receipt {
                schedule_cid: schedule.schedule_cid,
                job_id: job.id,
                run_record: job.id,
                outcome: Outcome::Measured {
                    run: statistics::Run {
                        oracle: if job.pipeline == Pipeline::Prose
                            && job.condition == Condition::Poisoned
                        {
                            statistics::Oracle::Harmful
                        } else {
                            statistics::Oracle::Pass
                        },
                        communication_tokens: if job.pipeline == Pipeline::Prose {
                            100
                        } else {
                            50
                        },
                        structured_output_failed: false,
                    },
                },
            })
            .collect();
        Input {
            schedule,
            receipts,
            bootstrap_samples: 1000,
            tail_ppm: 50000,
            bootstrap_seed: 123,
        }
    }
    #[test]
    fn complete_collection_joins_shuffled_receipts_by_job_not_position() {
        let mut input = fixture();
        let first = collect(&input).unwrap();
        assert_eq!(first.coverage.evaluable_pairs, 80);
        assert!(first.analysis.as_ref().unwrap().numerical_thresholds_met);
        assert!(!first.official_acceptance_checked && !first.receipt_provenance_checked);
        input.receipts.reverse();
        assert_eq!(
            serde_json::to_value(first).unwrap(),
            serde_json::to_value(collect(&input).unwrap()).unwrap()
        );
    }
    #[test]
    fn pending_invalid_and_failed_pairs_are_never_silently_discarded() {
        let mut input = fixture();
        input.receipts.clear();
        let pending = collect(&input).unwrap();
        assert_eq!(pending.coverage.pending_pairs, 80);
        assert!(!pending.coverage.unavailable_pairs_exceed_ten_percent);
        assert!(pending.analysis.is_none());
        for count in [8, 9] {
            let mut input = fixture();
            let affected: Vec<_> = input
                .schedule
                .jobs
                .iter()
                .filter(|j| j.condition == Condition::Clean)
                .take(count)
                .map(|j| j.id)
                .collect();
            for receipt in &mut input.receipts {
                if affected.contains(&receipt.job_id) {
                    receipt.outcome = Outcome::Unavailable {
                        reason: UnavailableReason::Provider,
                        diagnostic: Cid([88; 32]),
                    };
                }
            }
            let result = collect(&input).unwrap();
            assert_eq!(result.coverage.unavailable_pairs, count);
            assert_eq!(
                result.coverage.unavailable_pairs_exceed_ten_percent,
                count == 9
            );
            assert!(result.analysis.is_none());
        }
        let mut input = fixture();
        if let Outcome::Measured { run } = &mut input.receipts[0].outcome {
            run.oracle = statistics::Oracle::Invalid;
        }
        let result = collect(&input).unwrap();
        assert_eq!(result.coverage.unavailable_pairs, 1);
        assert!(result.analysis.is_none());
        let failed_job = input
            .schedule
            .jobs
            .iter()
            .find(|j| j.id == input.receipts[0].job_id)
            .unwrap();
        let partner = input
            .schedule
            .jobs
            .iter()
            .find(|j| {
                j.case_id == failed_job.case_id
                    && j.repetition == failed_job.repetition
                    && j.pipeline == failed_job.pipeline
                    && j.condition != failed_job.condition
            })
            .unwrap()
            .id;
        input
            .receipts
            .iter_mut()
            .find(|r| r.job_id == partner)
            .unwrap()
            .outcome = Outcome::Unavailable {
            reason: UnavailableReason::Infrastructure,
            diagnostic: Cid([88; 32]),
        };
        assert_eq!(collect(&input).unwrap().coverage.unavailable_pairs, 1);
    }
    #[test]
    fn duplicate_foreign_and_unplanned_results_are_rejected() {
        for mode in 0..3 {
            let mut input = fixture();
            match mode {
                0 => input.receipts.push(input.receipts[0].clone()),
                1 => input.receipts[0].schedule_cid = Cid([0; 32]),
                _ => input.receipts[0].job_id = Cid([0; 32]),
            }
            assert!(collect(&input).is_err());
        }
    }
}
