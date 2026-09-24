# Goal acceptance audit

`myr_runner::acceptance::assess` reopens the compiler seal and checks every binding
criterion using its exact atom, polarity, goal, and snapshot. It computes CLAIM
identity without inserting any object. Each criterion needs a currently active
FACT; the audit reads and verifies the entire dependency closure through CAS and
checks that no dependency is stale or inactive. Storage corruption is an error.

The report retains obligation status, active FACT references, conflict diagnostics,
evidence, supporting active assumptions, referenced artifacts, and dependency
provenance. Nonbinding criteria cannot substitute for missing obligations.
Conflicts are reported without overriding the promotion policy: deterministic
support can still establish a FACT in the presence of LLM disagreement according
to Appendix B. Failed or missing obligations do not establish UNSAT.

An obligation without an active FACT is `refuted` when live deterministic
EVIDENCE contradicts it in its exact scope: a `CONTRADICTS` verdict on the sealed
CLAIM or a `SUPPORTS` verdict on the same ATOM with opposite polarity (Appendix
B.1). The refuting evidence references and their dependency closure are reported.
LLM evidence never refutes; it can only block LLM-only promotion. In a candidate
audit only refuting evidence bound to that candidate is retained, so contrary
evidence recorded for another candidate leaves this one merely unproven.
`refuted_by_deterministic_evidence` reports a refutation of one candidate; the
pipeline decides whether it also proves the obligations incompatible.

`all_binding_proven` is a necessary acceptance gate, not a terminal mission state.
The caller must additionally establish that evidence concerns the final candidate,
finish required pipeline stages, collect assumptions and outputs from those stages,
and recheck liveness before issuing a result. An agent's `finish` is not evidence.
No COMPLETE, UNSAT, or INVALID_GOAL result is manufactured by this audit. Graph
inspection assumes a single runtime writer; concurrent mission finalization still
needs transaction-level coordination. Returned reports are read-time observations.

The integration test uses synthetic runtime evidence, not an executed sandbox.
It covers missing claims, wrong polarity, exact scope separation, supporting
evidence and assumptions, then invalidation that removes acceptance while keeping
historical evidence readable.

`assess_candidate` additionally requires runtime command provenance for the exact
candidate manifest, or [bound LLM reviews](candidate-reviews.md). Mixed-candidate
or unbound evidence yields `unbound_evidence`
even when the ordinary scoped graph has an active FACT. See
[runtime command evidence](command-evidence.md) for the private transcript checks
and remaining terminal orchestration work.
