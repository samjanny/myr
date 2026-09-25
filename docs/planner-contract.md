# Pre-seal planner contract

`planner::Catalog` is an explicitly selected set of registered predicates, ATOMs
and permitted artifacts. Construction rejects duplicate/inactive references and
ATOMs outside the supplied predicate registry. It is not an unrestricted CAS
listing. The runtime must select worker-visible inputs and exclude source-view
benchmark data. Existing references are revalidated when a proposal is compiled.

The response schema uses the same action envelope as the existing transports,
with `submit_goal`, `define_atom`, `put_rationale`, `fetch` and `fetch_many`
actions. Goal submission arguments are Goal IR. The schema fixes the user's goal
text and constrains each reference's kind and CID syntax. It no longer
enumerates catalog CIDs (cost pilot, step B). Those listings made up about two
thirds of the schema and were re-sent on every call; the rendered catalog
already lists the available references. The schema's size is independent of the
catalog's. The catalog checks every action; an out-of-catalog reference is an
immediate CAPABILITY_DENIED. Nested objects require
their declared fields and reject extras. Measurement is explicitly null or a
procedure/tolerance record; assumptions identify their chosen value, alternatives,
rationale and affected artifacts. Schema partitioning/accounting is tested, but
live missions accepted the earlier enumerated form with Claude Code on
2026-09-25; the compact form still needs a live run.

`compile_response` limits untrusted output to 1 MiB, uses strict typed decoding,
rejects duplicate/unknown fields and omitted contract fields, and enforces the
catalog independently of provider-side schema enforcement. It then invokes
`mission::compile`, preserving exact user verifiers and protected paths, followed
by the goal compiler's predicate-class, registry and liveness validation. Provider,
billing, baseline, budgets and verifier policies are runtime arguments, never
model-owned proposal fields. Invalid proposals do not change an existing seal.

The initial ATOM set may be empty. `define_atom` accepts only an existing catalog
predicate and explicitly accessible reference arguments; graph insertion checks
arity and argument types before indexing. Valid normalized ATOMs are added to the
catalog and subsequent response schemas. `put_rationale` stores nonempty UTF-8
text up to the adapter artifact limit, preserving its bytes, and returns a new
permitted reference. `fetch` reads only explicit catalog members, including newly
created objects; it does not grant unrestricted access to their reference closure.
The runtime receives created CIDs, fetched bytes or a validated seal as distinct
step outcomes. Registry mutation, arbitrary filesystem writes and FACT/EVIDENCE
creation are absent from this planner interface.

This module is the response boundary. The separate
[planner execution loop](planner-execution.md) provides budgeted dispatch, context
assembly and repair handling. Initial mission catalog preparation and connection
to the complete CLI flow remain pending. This boundary itself makes no provider
call and has no goal redefinition or API billing fallback.

Integration tests cover successful mission-bound sealing, protocol schema
accounting, immutable policy preservation, changed goals, omitted/weakened command
criteria, provider-field injection, omitted null fields, out-of-catalog live
objects, duplicate fields and oversized output. Additional tests construct command
and unresolved ATOMs, create/fetch a rationale and seal a pending assumption; wrong
argument types and out-of-catalog accesses leave no invalid ATOM indexed. Tests
use local graph fixtures.
