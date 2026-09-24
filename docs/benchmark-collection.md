# Collecting planned results

`myr collect-benchmark input.json` joins receipts to a reproducible schedule and
prints coverage plus, for complete data, the statistics report. It never invokes
providers or oracles and never certifies an official experiment.

Input fields are `schedule` (the complete `plan-benchmark` output), `receipts`,
`bootstrap_samples`, `tail_ppm` and `bootstrap_seed`. Each receipt contains:

- `schedule_cid`: must match the verified schedule.
- `job_id`: must identify exactly one scheduled job.
- `run_record`: CID supplied as the execution provenance reference.
- `outcome`: a tagged measured result or declared unavailability.

Measured outcome shape:

```json
{"kind":"measured","run":{"oracle":"PASS","communication_tokens":123,"structured_output_failed":false}}
```

Unavailable outcome shape (the diagnostic CID is a placeholder):

```json
{"kind":"unavailable","reason":"provider","diagnostic":"b3:<diagnostic-hash>"}
```

The other unavailability reason is `infrastructure`. Measured oracles are PASS,
HARMFUL or INVALID. INVALID makes the pair unavailable and is never reclassified.
Unknown fields, duplicate job receipts, foreign schedules and unplanned jobs
are rejected. Receipts may arrive out of order; results are joined using the
job's case, repetition, pipeline and condition, not receipt array position.

Coverage counts one pair per case/repetition/pipeline. A pair is unavailable if
either condition has an explicit provider/infrastructure failure or INVALID
oracle, even if the other condition is still absent. Both conditions failing
still count as one unavailable pair. Otherwise a pair is pending until both
conditions arrive, or evaluable once both valid measurements arrive. Missing job
IDs remain listed regardless of pair classification.

`unavailable_pairs_exceed_ten_percent` uses exact integer comparison against all
scheduled pairs. Exactly 10% does not trigger it. Pending jobs alone do not
trigger this flag: absence is not automatically a failed execution. This is the
coverage component of the specification's INCONCLUSIVE gate, not an official
terminal verdict.

Statistics are computed only when every pair is evaluable. Smaller amounts of
missingness require an explicit preregistered treatment still to be implemented;
the collector does not silently discard cases or repetitions. The sorted receipt
content CID makes aggregation independent of receipt arrival order.

`official_acceptance_checked` and `receipt_provenance_checked` are always false.
Referenced execution/diagnostic artifacts are not fetched or authenticated here.
Connecting receipts to actual executions, verifying corpus/oracle provenance,
freezing statistical parameters and producing the official terminal report
remain pending. Synthetic collector tests are not benchmark results.
