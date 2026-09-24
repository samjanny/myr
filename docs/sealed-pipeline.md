# Fixed post-seal pipeline

`pipeline::run_sealed` is a consuming execution API: it runs the explicitly sealed
provider transports and can consume subscription quota or paid API usage. It is
not an inspection operation. `myr run` connects preparation, planning and this pipeline.

Given an existing valid compiler seal, instruction references, writable paths and
selected unresolved decisions, it issues a worker TASK, processes its action loop,
reconstructs its DELTAs into a recorded candidate, executes sealed command criteria,
then issues two fresh reviewer TASKs for the same candidate. Both reviewer slots
use the provider/model/billing selection fixed by the goal seal. Each reviewer
must produce bound EVIDENCE for every binding CLAIM and finish its task. A bare
`finish` is insufficient. No agent receives another agent's conversation history.

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
produce COMPLETE_WITH_ASSUMPTIONS. Other outcomes are PARTIAL. Tool failures and
missing evidence never imply UNSAT. Invalid seals return errors before provider
execution; INVALID_GOAL and evidence-backed UNSAT reports remain planner/compiler
and terminal-orchestration work.

The private immutable report retains stage outputs, command evidence references,
failures, acceptance, artifacts, active assumptions, provenance, budget usage and
the private dispatch checkpoint. It is a historical observation, not a persistent
promise that references remain live. Raw infrastructure errors can abort reporting
when even the graph/audit store is unavailable. Concurrent finalization and crash
recovery are not implemented.

If a task loop returns an infrastructure error while the stores remain usable,
the report retains an unfinished stage and a runtime FAIL linked to the issued
TASK (when still live). Historical task dependencies remain in report provenance,
including dependencies of an inactive task. This preserves issued-task context;
it does not reconstruct unreturned intermediate outputs from an interrupted loop.

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
