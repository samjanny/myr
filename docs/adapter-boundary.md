# Adapter boundary

`myr-adapter` implements a shared model-output contract, independent of whether
the transport is Codex CLI, Claude Code, OpenAI API, or Anthropic API.
See [transport evidence](transports.md) for implementation and live-test limits.

A response contains exactly one action:

```json
{"action":{"tool":"emit_claim","arguments":{"predicate_ref":{"kind":"PREDICATE_DEF","cid":"b3:<64 lowercase hex digits>"},"arguments":[],"polarity":true}}}
```

The CID in this example is a placeholder. `schema::response_schema(role)` returns
the role-specific JSON Schema. The Rust parser rejects unknown fields and tools;
semantic validation checks reference access, kinds, predicate arguments, scope,
and baseline mapping. Policy validation checks roles, write capabilities, and
protected paths. A model cannot supply agent identity, lineage, EVIDENCE, FACT,
ATTEST, scope, or task identity.

Planner/worker actions: `emit_claim`, `emit_assumption`, `emit_delta`, `emit_fail`.
Reviewer action: `review_claim`. Shared runtime actions: `fetch`, `put_artifact`,
`finish`. Artifact bodies cross the JSON boundary as canonical Base64 so line
endings and arbitrary bytes remain opaque. `emit_delta` requires `path`,
`base_ref`, `patch_ref`, `result_ref`, `codec`, and `assumptions`. The codec is
`REPLACEMENT` or `UNIFIED_DIFF`; replacement requires identical patch and result
references. Both artifacts must be accessible to the session. Emission records
a proposed DELTA; the runner checks actual patch application and result bytes
before using it in a verifier candidate.

`emit_claim` creates ATOM, CLAIM, and runtime-owned ATTEST in one SQLite
transaction. A failed member rolls back the entire group; unindexed CAS objects
cannot be fetched through graph access. Two provider identities produce the same
CLAIM CID and different ATTEST CIDs. `review_claim` becomes runtime-issued
LLM_REVIEW EVIDENCE with configuration-owned lineage, never model-declared lineage.

Schema/semantic errors preserve raw output under `quarantine/<artifact-hash>`
without indexing it. Two repair responses are allowed; the third error emits
runtime FAIL `INVALID_AGENT_OUTPUT`. A successful action resets the repair count
for the next action. Capability violations immediately emit `CAPABILITY_DENIED`
without repair. Ended sessions reject further actions. SQL/CAS infrastructure
errors are returned to the runner rather than mislabeled as model errors.

Runtime-owned session roots start at the TASK and add objects produced by that
session. A CID outside the explicit reference closure is a semantic error even
when it physically exists in CAS. A delta must reference the sealed baseline
artifact for its path and accessible patch/result artifacts. Protected paths take
precedence over write capabilities.

These are in-process trust boundaries, not operating-system isolation. Live
transport confinement conformance, mission call/token budgets, full JSON Schema
provider compatibility, and the sandboxed command verifier still require
implementation or integration testing. Transport timeouts are already enforced.
