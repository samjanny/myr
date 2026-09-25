# Fixed post-seal pipeline

`pipeline::run_sealed` is a consuming execution API: it runs the explicitly sealed
provider transports and can consume subscription quota or paid API usage. It is
not an inspection operation. `myr run` connects preparation, planning and this pipeline.

Given an existing valid compiler seal, instruction references and writable paths,
it issues a worker TASK, processes its action loop,
reconstructs its DELTAs into a recorded candidate, executes sealed command criteria,
then issues two fresh reviewer TASKs for the same candidate. Both reviewer slots
use the provider/model/billing selection fixed by the goal seal. Each reviewer
must produce bound EVIDENCE for every binding CLAIM and finish its task. A bare
`finish` is insufficient. No agent receives another agent's conversation history.

With a configured source view (primary benchmark condition), a planner source
TASK runs after the seal and before the worker. It has no capabilities or
obligations. Its session serves the private view from memory and refuses every
other repository artifact. It also refuses to republish a private-only file.
The CLAIMs, ATTESTs, assumption proposals and artifacts it emits become inputs
of the worker and both reviewer TASKs. Completion then requires four finished
stages and `source_blobs_absent`.

Revision 2 delivery rule. The source-pass CLAIMs whose predicate is not
HEURISTIC become the report's `received_claims`, and also the worker's
`premises`. When the task loop accepts a DELTA, it adds a `depends_on` edge
from that DELTA to every premise. Task setup rejects premises that are not
CLAIMs in the task's inputs.

Both reviewers must review every received claim, as for binding claims; an
omission is INVALID_AGENT_OUTPUT. After the reviews, the pipeline walks the
candidate's dependency closure. A non-HEURISTIC CLAIM there, without an active
FACT and with valid contrary EVIDENCE, is listed in `blocked_claims`. Valid
contrary EVIDENCE is a live CONTRADICTS, or a live SUPPORTS on the opposite
polarity (Appendix B.1). Any blocked claim prevents COMPLETE even when every
binding obligation is proven, and the run disposition becomes NO_DELIVERY.

Tests cover four cases:
- supported: the claim is promoted and the candidate delivered;
- contradicted: delivery is blocked and no FAIL is recorded;
- omitted review: INVALID_AGENT_OUTPUT;
- contradicted with no DELTA: the baseline candidate is delivered, because its
  closure contains no premise.

They also confirm that HEURISTIC claims never block.

The worker TASK depends on every pending assumption sealed in the Goal IR: those
choices materialize as ASSUMPTION objects when the worker is issued, because the
worker produces the artifacts they affect. Its inputs also include the sealed
predicate registry, so `emit_claim` can use any registered predicate; the rest
of its reference closure comes from the scope (baseline files) and obligations.

Standalone `run_sealed` owns a new dispatcher. `run_continuing` instead borrows
the planner's existing dispatcher and rejects expanded limits, a changed audit
store or an incompatible accounting pipeline. Both enforce cumulative
call/reference-token limits across their stages. Verifier deadlines are
clamped to the remaining mission duration; cleanup retains its separate bounded
emergency allowance. Deadline exhaustion prevents completion even if all facts
would otherwise be accepted. The standalone API excludes planning; continuing
execution preserves planning's consumed calls/tokens, exact deadline and journal.

`pipeline::plan_and_run` coordinates both phases for a prepared mission catalog
and runtime policy. It returns either planning failure with its consumed budget
and audit checkpoint, or the pipeline outcome plus planner-created references.
It uses one dispatcher throughout and never substitutes another billing backend.
The `setup` module and CLI now prepare the repository/mission catalog before
invoking this coordinator.

The result is COMPLETE only when all three agent stages finish, both reviews cover
every binding claim, no execution failure is recorded, the deadline holds, and
candidate-bound acceptance proves every obligation. Materialized active assumptions
produce COMPLETE_WITH_ASSUMPTIONS. Tool failures and missing evidence never imply
UNSAT. Invalid seals return errors before provider execution; INVALID_GOAL is an
input-admission report.

UNSAT requires evidence of incompatible obligations. The pipeline recognizes one
mechanically provable form: the sealed policy leaves no baseline path writable
outside protected prefixes (DELTAs cannot create paths), so the sealed baseline
is the only admissible candidate, and runtime deterministic EVIDENCE bound to
that candidate contradicts a binding obligation. Appendix B.3 rule 1 makes such a
refutation final, so the reviewers are not dispatched. The pipeline records a
`CONFLICTING_OBLIGATIONS` FAIL whose diagnostic is a `myr-unsat-proof-v0`
artifact depending on the candidate, the refuted atoms and the refuting evidence,
and the report state is UNSAT. A refuted candidate that could have been different
remains PARTIAL: no unsatisfiability follows from one failed attempt. Sandbox,
provider and budget failures never produce this state.

The private immutable report retains stage outputs, command evidence references,
failures, acceptance, artifacts, active assumptions, provenance, budget usage and
the private dispatch checkpoint. It is a historical observation, not a persistent
promise that references remain live. Raw infrastructure errors can abort reporting
when even the graph/audit store is unavailable. Concurrent finalization and crash
recovery are not implemented.

If a task loop returns an infrastructure error while the stores remain usable,
the report retains an unfinished stage and a runtime FAIL linked to the issued
TASK (when still live). Historical task dependencies remain in report provenance,
including dependencies of an inactive task. A task interruption error also carries
successful emissions from earlier loop iterations; those outputs enter the stage,
result contents and private report without replaying provider requests. Recovery
after process termination or an interrupted adapter transaction remains open.

Tests drive the real task loop, adapter, graph, candidate reconstruction, command
recording and acceptance with synthetic provider/executor responses. They cover
unconditional/conditional completion, invalid worker output, sandbox failure and
reviewers that finish without reviewing. No live provider or Docker execution is
performed by these tests.

Remaining integration includes empirical configuration setup, candidate iteration lifecycle,
live sandbox/provider acceptance, and the specification's benchmark deliverables.
Empirical binding criteria now execute the sealed measurement command and compare
its typed scalar result with the sealed tolerance. Invalid measurements remain
PARTIAL; valid counterevidence prevents reviewer agreement from completing the
mission. Execution/provenance tests use injected receipts; live validation remains.
