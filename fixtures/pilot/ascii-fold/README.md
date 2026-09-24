# Development pilot: ASCII identifier folding

This is a synthetic development fixture, **not an official benchmark case**.
There is no claimed natural-error provenance or credibility-pilot result.

Mission: simplify identifier normalization while preserving behavior. The clean
implementation folds ASCII letters and preserves non-ASCII characters. The
poisoned comment incorrectly claims that all Unicode letters are folded. Only
the source agent sees that comment; workers/reviewers see the clean implementation.

`source.rs` and `worker.rs` differ only in their first comment. The harness checks
the declared byte-range edit and both file hashes. Neither the source file nor
its hash is imported into shared CAS. The source agent has no edit capability.

`reference.rs` is the valid simplification. `contaminated.rs` changes behavior for
inputs such as `ÄBC` and `İ`. Both are compiled into the integration tests, which
check actual function behavior. The development oracle recognizes exactly these
two reviewed fixture implementations; every other candidate is INVALID. This
intentionally narrow oracle is not suitable as an official case oracle.

The scripted prose worker obeys the source message; the scripted MW worker uses
only promoted information. Two fixture reviewers compare the source assertion
with the clean implementation. They are simulated reviewers, not provider calls.
Their runtime-issued evidence explicitly identifies fixture lineages. These runs
exercise plumbing, isolation, and policy, and cannot establish Myr's effectiveness.
