# Runtime command evidence

`command_verification::verify` accepts a recorded candidate, a binding ATOM from
its sealed goal, a separate private audit store, and explicit Docker runtime
configuration. It only handles the registered `core.command_succeeds` predicate.
The ATOM supplies the exact sealed command policy; callers cannot substitute a
different command or reinterpret an exit code as an arbitrary predicate.

Production recording is reached only after `docker_sandbox::execute` returns a
normal execution receipt and successful cleanup. The runtime reopens candidate
provenance and policy, retains stdout/stderr as opaque shared artifacts, and stores
the full execution transcript in private audit CAS. Private and shared stores
cannot overlap. The shared canonical `myr-command-run-v0` artifact contains the
candidate, policy, exit code, output references and private transcript reference.
It has atomic graph dependencies on the candidate, policy and both output blobs.
The private reference is not an agent-accessible graph edge.

CLAIM and runtime EVIDENCE are inserted in a single semantic transaction. The
criterion's polarity determines whether exit zero supports or contradicts the
CLAIM; ordinary nonzero exits are retained. Startup-reserved codes 125–127,
out-of-range codes, oversized output and infrastructure errors do not produce
command evidence. Runtime output artifacts can remain after a failed semantic
transaction, but no partial CLAIM/EVIDENCE batch is accepted. Command text is
normalized for MW according to the wire rules; exact original argv/environment
remain in the sealed policy and the raw execution transcript.

`load_record` verifies canonical shared bytes, dependency liveness, reconstruction,
and correspondence with the integrity-checked private transcript. Merely uploading
similar JSON into agent CAS does not create runtime provenance. Invalidation of an
assumption used by a candidate DELTA stales the run record, its EVIDENCE and FACTs,
while historical CAS bytes remain available.

`acceptance::assess_candidate` adds candidate binding to the existing goal audit.
Every EVIDENCE in an accepted FACT must match this candidate and its sealed command
semantics, outputs and private transcript. Legacy unbound evidence, LLM evidence
without candidate binding, and FACTs mixing multiple candidates are not sufficient
for completion. The gate does not silently filter evidence or invent a replacement
FACT. Candidate iteration still needs explicit evidence lifecycle management.
[LLM review binding](candidate-reviews.md) is implemented separately; complete
terminal mission orchestration remains pending.

Tests use synthetic executor receipts solely to exercise persistence and policy.
They cover positive/negative obligations, zero/nonzero exits, opaque outputs,
private-store isolation, missing transcripts, mixed candidates and transitive
assumption invalidation. They do not establish live Docker confinement, observed
tool versions, or full mission success. Those acceptance gates remain open.
