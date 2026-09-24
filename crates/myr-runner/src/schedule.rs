//! Reproducible benchmark ordering. Planning does not admit cases or run models.
use crate::Result;
use myr_core::{Cid, invalid};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Case {
    pub id: String,
    pub poison_type: String,
    /// The falsehood lives only in comments or documents (specification
    /// composition rule: at least one third of cases must not be purely so).
    pub purely_documentary: bool,
    /// Deterministic verifiers available to agents can refute the falsehood
    /// before the patch. At least one third of cases must be refutable only by
    /// reviewers reasoning about the code; the hidden oracle stays deterministic.
    pub refutable_by_visible_verifiers: bool,
    /// Opaque reference supplied by the harness; planning does not validate it.
    pub case_manifest: Cid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub cases: Vec<Case>,
    pub configuration: Cid,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pipeline {
    Prose,
    Mw0,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    Clean,
    Poisoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Job {
    pub id: Cid,
    pub case_id: String,
    pub case_manifest: Cid,
    pub repetition: u8,
    pub pipeline: Pipeline,
    pub condition: Condition,
    /// Same requested model seed for all four jobs of a paired repetition.
    pub model_seed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub format: String,
    pub executed: bool,
    pub admission_checked: bool,
    pub input: Input,
    pub jobs: Vec<Job>,
    pub schedule_cid: Cid,
}

pub fn plan(input: &Input) -> Result<Schedule> {
    let n = input.cases.len();
    if !(8..=12).contains(&n) {
        return Err(invalid("primary schedule requires 8..12 cases").into());
    }
    let mut ids = BTreeSet::new();
    let mut manifests = BTreeSet::new();
    let mut categories = BTreeMap::new();
    for case in &input.cases {
        if case.id.trim().is_empty()
            || case.id.trim() != case.id
            || !ids.insert(&case.id)
            || !manifests.insert(case.case_manifest)
            || case.poison_type.trim().is_empty()
            || case.poison_type.trim() != case.poison_type
        {
            return Err(invalid(
                "schedule requires unique case IDs/manifests and nonempty categories",
            )
            .into());
        }
        *categories.entry(&case.poison_type).or_insert(0usize) += 1;
    }
    if categories.len() < 3 || categories.values().any(|count| count * 5 > n * 2) {
        return Err(
            invalid("schedule needs three poison categories, each at most 40 percent").into(),
        );
    }
    // Composition rules: at least one third of the primary cases are not purely
    // documentary, and at least one third can be refuted only by reviewers
    // reasoning about the code. Without the latter, the LLM-only promotion
    // path (two lineages, 800000) is never exercised by the official suite.
    let non_documentary = input.cases.iter().filter(|c| !c.purely_documentary).count();
    let reasoning_only = input
        .cases
        .iter()
        .filter(|c| !c.refutable_by_visible_verifiers)
        .count();
    if non_documentary * 3 < n || reasoning_only * 3 < n {
        return Err(invalid(
            "schedule needs at least one third non-documentary cases and one third cases refutable only by reasoning",
        )
        .into());
    }
    let mut input = input.clone();
    input.cases.sort_by(|a, b| a.id.cmp(&b.id));
    let mut jobs = Vec::with_capacity(n * 20);
    for case in &input.cases {
        for repetition in 0..5u8 {
            let seed_digest = myr_wire::artifact_cid(&serde_json::to_vec(&(
                "myr-paired-model-seed-v0",
                input.seed,
                &case.id,
                case.case_manifest,
                repetition,
            ))?);
            let model_seed =
                u64::from_le_bytes(seed_digest.0[..8].try_into().expect("eight digest bytes"));
            for pipeline in [Pipeline::Prose, Pipeline::Mw0] {
                for condition in [Condition::Clean, Condition::Poisoned] {
                    let id = myr_wire::artifact_cid(&serde_json::to_vec(&(
                        "myr-benchmark-job-v0",
                        input.configuration,
                        &case.id,
                        case.case_manifest,
                        repetition,
                        pipeline,
                        condition,
                        model_seed,
                    ))?);
                    jobs.push(Job {
                        id,
                        case_id: case.id.clone(),
                        case_manifest: case.case_manifest,
                        repetition,
                        pipeline,
                        condition,
                        model_seed,
                    });
                }
            }
        }
    }
    // Fisher-Yates over complete job records: pairing keys survive shuffling.
    let mut state = input.seed;
    for i in (1..jobs.len()).rev() {
        let j = crate::statistics::index(&mut state, i + 1);
        jobs.swap(i, j);
    }
    let format = "myr-benchmark-schedule-v0".to_owned();
    let schedule_cid = myr_wire::artifact_cid(&serde_json::to_vec(&(&format, &input, &jobs))?);
    Ok(Schedule {
        format,
        executed: false,
        admission_checked: false,
        input,
        jobs,
        schedule_cid,
    })
}

/// Reject missing, duplicated, reordered or changed jobs and altered metadata.
pub fn verify(schedule: &Schedule) -> Result<()> {
    if plan(&schedule.input)? != *schedule {
        return Err(invalid("schedule differs from reproducible plan").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(n: usize) -> Input {
        Input {
            cases: (0..n)
                .map(|i| Case {
                    id: format!("case-{i}"),
                    poison_type: format!("type-{}", i % 3),
                    purely_documentary: i % 3 != 0,
                    refutable_by_visible_verifiers: i % 3 != 0,
                    case_manifest: Cid([i as u8; 32]),
                })
                .collect(),
            configuration: Cid([99; 32]),
            seed: 42,
        }
    }
    #[test]
    fn all_paired_jobs_are_unique_and_share_configuration_and_seed() {
        for n in [8, 12] {
            let input = fixture(n);
            let schedule = plan(&input).unwrap();
            assert_eq!(schedule.jobs.len(), n * 20);
            let mut groups = BTreeMap::<_, BTreeSet<_>>::new();
            let mut ids = BTreeSet::new();
            let mut seeds = BTreeMap::new();
            for job in &schedule.jobs {
                assert!(ids.insert(job.id));
                let key = (&job.case_id, job.repetition);
                assert!(
                    groups
                        .entry(key)
                        .or_default()
                        .insert((job.pipeline, job.condition))
                );
                if let Some(previous) = seeds.insert(key, job.model_seed) {
                    assert_eq!(previous, job.model_seed);
                }
            }
            assert_eq!(groups.len(), n * 5);
            assert!(groups.values().all(|group| group.len() == 4));
            assert!(!schedule.executed && !schedule.admission_checked);
            verify(&schedule).unwrap();
            let mut reversed = input.clone();
            reversed.cases.reverse();
            assert_eq!(plan(&reversed).unwrap(), schedule);
            reversed.seed += 1;
            assert_ne!(plan(&reversed).unwrap().jobs, schedule.jobs);
        }
    }
    #[test]
    fn tampering_and_case_padding_are_rejected() {
        let original = plan(&fixture(8)).unwrap();
        for mode in 0..5 {
            let mut schedule = original.clone();
            match mode {
                0 => {
                    schedule.jobs.pop();
                }
                1 => schedule.jobs.swap(0, 1),
                2 => schedule.jobs[0].model_seed ^= 1,
                3 => schedule.admission_checked = true,
                _ => schedule.input.configuration = Cid([98; 32]),
            }
            assert!(verify(&schedule).is_err());
        }
        let mut input = fixture(8);
        input.cases[1].case_manifest = input.cases[0].case_manifest;
        assert!(plan(&input).is_err());
        assert!(plan(&fixture(7)).is_err());
        assert!(plan(&fixture(13)).is_err());
    }
    #[test]
    fn composition_requires_non_documentary_and_reasoning_only_thirds() {
        for n in [8usize, 9, 12] {
            let minimum = n.div_ceil(3);
            let admitted = fixture(n);
            assert_eq!(
                admitted
                    .cases
                    .iter()
                    .filter(|c| !c.purely_documentary)
                    .count(),
                minimum
            );
            plan(&admitted).unwrap();
            let mut documentary = admitted.clone();
            documentary.cases[0].purely_documentary = true;
            assert!(plan(&documentary).is_err(), "n={n}: below one third");
            let mut refutable = admitted.clone();
            refutable.cases[0].refutable_by_visible_verifiers = true;
            assert!(
                plan(&refutable).is_err(),
                "n={n}: LLM-only path unexercised"
            );
            let mut all = admitted.clone();
            for case in &mut all.cases {
                case.purely_documentary = true;
                case.refutable_by_visible_verifiers = true;
            }
            assert!(plan(&all).is_err());
        }
    }
}
