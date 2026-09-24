# Goal IR compilation

`myr_runner::goal::compile` validates a structured planner proposal against
already registered predicate definitions and atoms. It then stores an immutable
`myr-goal-ir-v0` envelope as a GOAL object. The envelope contains the goal text,
registry, acceptance atoms and polarity, pending assumptions, baseline manifest,
protected paths, verifier policy artifacts, and mission budgets. Changing any
of these inputs changes the content-addressed seal. Set-like registry and policy
fields are sorted before serialization. The envelope is a development format;
normative schema freeze and cross-language vectors remain pending.

`SealPolicy.providers` contains explicit configurations for `planner`, `worker`,
`reviewer_a`, and `reviewer_b`, including backend/billing choice, model and lineage.
All configurations must validate, and the two reviewers must differ in both
producer and family. These fields are part of the goal seal. The task runner
requires its selected role slot, model and backend to match that sealed record.
`fixtures/providers-test-only.json` contains deliberately nonexistent executable
and model values for injected-transport tests; it is not a live configuration.

Validation rejects unknown or duplicate predicate names, unregistered atoms,
inactive references, modified definitions of closed core predicates, empty goals,
duplicate or opposing acceptance atoms, and nonpositive budgets. A mission must
have at least one binding criterion. Heuristics cannot be binding; empirical
criteria require a measurement artifact and nonnegative tolerance. Unresolved
predicates require explicit pending assumptions with a choice, alternatives,
rationale, and affected artifacts. Compilation does not silently pick a choice
or create active ASSUMPTION objects before a dependent task needs them.

`goal::load` reopens a GOAL without writes. It checks the format version, exact
compiler encoding, and every current dependency using the same validation as
compilation. A still-readable seal whose verifier policy is now inactive is
rejected. Tests include stale dependencies and noncanonical/unknown envelopes.

The resulting `SealedGoal` exposes shared references and a scope, with no mutation
API. This is a trusted in-process boundary, not protection against a process that
can edit the store. Agents cannot invoke compiler authority through the adapter.

`issue_task` reloads the sealed goal, checks that requested obligations are
binding criteria, and materializes only explicitly selected pending assumptions.
Assumptions and the TASK are inserted in one graph transaction. The TASK records
the decision atoms as inputs for provenance. Invalidating a selected assumption
makes its dependent task inactive; issuing that same decision again fails rather
than reviving it. Tasks with no such dependency remain usable. A revised choice
requires a new goal compilation. Capability declarations and input selection are
runtime-owned; this API is not exposed as a model action. Task budget fields are
upper bounds and must be combined with the shared mission budget gate.

Tests cover seal sensitivity for every input category, class restrictions,
registry membership, conflicting criteria, and pending assumptions. YAML input,
planner output-schema integration and full execution remain pending.

Verifier policy artifacts now use the strict `VerifierPolicy` schema. Command
policies declare argv, relative cwd, the exact environment, tool versions,
timeout, output limit, and sandbox requirements. Validation rejects networking,
zero resource limits, missing executable versions, ambiguous environment keys,
and timeouts longer than the mission budget. Review policies declare a live
procedure artifact and known lineage. `core.command_succeeds` criteria must name
a command policy included in the seal. This validates declarations only: actual
sandbox availability, enforced resource limits, observed tool versions, and
the two-reviewer execution topology still require runtime implementation.
