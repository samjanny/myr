# Reproducible paired schedule

`myr plan-benchmark input.json` prints a deterministic randomized job plan. It
does not create stores, run commands, access providers or consume model quota.
The strict input has:

- `cases`: 8–12 objects with unique `id`, `poison_type`, `purely_documentary`,
  `refutable_by_visible_verifiers` and `case_manifest` CID.
- `configuration`: CID identifying the common experiment configuration.
- `seed`: unsigned 64-bit randomization seed, chosen before execution.

There must be at least three poison categories and none may exceed 40% of cases.
At least one third of the cases must not be purely documentary, and at least one
third must be refutable only by reviewers reasoning about the code, meaning the
deterministic verifiers available to agents cannot refute the falsehood before
the patch. Without the latter the official suite would never exercise the
LLM-only promotion path (two lineages, confidence 800000). The two flags are
harness declarations; each case's go/no-go must check them against the actual
fixtures, as the development pilot does. Duplicate case-manifest references are
rejected to prevent counting the same submitted artifact twice. This structural check does not establish independence,
natural provenance, realistic poisoning or valid oracle fixtures.

After sorting case IDs, the planner generates five repetitions, each with prose
and MW/0 pipelines in clean and poisoned conditions: 20 jobs per case. Each paired
group receives the same requested model seed, derived from the input seed, case
ID, case-manifest CID and repetition using the domain-labeled artifact digest.
The first eight digest bytes in little-endian order form the requested u64 seed.
Providers without compatible seed support must record that limitation; the
planner does not claim that current provider transports apply these seeds.

Fisher-Yates randomizes the complete list using the same fixed SplitMix64 and
unbiased bounded-index algorithm used by statistics. Cases/repetitions/pipelines
remain identifiable in each shuffled job. Input case ordering does not change
the resulting plan. Job IDs commit the common configuration, case, repetition,
pipeline, condition and model seed; the schedule CID commits format, canonical
input order and the ordered jobs. The library's `schedule::verify` recomputes
the plan and rejects alterations, deletions, duplicates and reordered jobs.

Output explicitly says `executed: false` and `admission_checked: false`. Supplied
manifest/configuration CIDs are not fetched or verified by this planning command.
The plan must remain private to the experimental harness: case labels, poison
categories and case manifests must not leak into agent contexts. Corpus admission,
oracle fixture checks, source/worker view validation, configuration/tokenizer
freeze, baseline execution, provider seed support, actual scheduling, result
collection and official acceptance remain pending integration work.
