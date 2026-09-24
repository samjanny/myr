# Single-task execution

`task::run` executes one runtime-owned TASK through the selected provider and
the shared mission dispatcher. It checks provider/session lineage equality,
constructs the role-specific schema, and creates a task-local budget in addition
to the shared mission budget. Every response passes `Session::handle`; successful
emissions are returned as references, semantic mistakes receive bounded repair
feedback, and capability failures terminate immediately.

Before dispatch, the loop reopens the sealed goal, requires matching task scope,
checks that task budgets do not exceed sealed mission limits, and permits only
sealed binding criteria as obligations. Baseline and protected paths are loaded
from the seal, overriding those fields in SessionConfig. This prevents a caller
from weakening sealed protections when constructing the session. Task-loop tests
now use compiled seals rather than placeholder GOAL bytes.
Execution also names a fixed role slot. Its role and complete provider configuration
must equal the sealed role configuration, preventing model or billing changes at
dispatch. Session lineage remains configuration-owned.

The context begins with the TASK rendering and accumulates only this task's
actions and receipts. Created-object receipts include their CIDs so a following
action can reference them. Fetch results are classified by the runtime as goal
instruction, direct baseline content, referenced artifact content, or MW object
rendering. Repair feedback is included in communication accounting. Task liveness
is checked before dispatch and again before applying a response.

`finished=true` means the agent emitted `finish`, not that the mission meets its
acceptance criteria. Budget and provider failures produce runtime FAIL objects.
If the task itself is stale, the diagnostic records its reference without adding
a live dependency on that stale task. No failed task becomes an UNSAT judgment.

Tests exercise the full dispatcher/session loop with injected transport responses:
artifact creation followed by a reference receipt and finish, three malformed
responses triggering repair exhaustion, and a task call limit that stops requests
while the mission budget still has capacity. No live model calls were made.

When a loop error escapes after successful emissions, `Error::Interrupted`
retains the task reference, committed output references and original error.
The pipeline includes those outputs and their dependencies in its PARTIAL report,
including its immutable private copy. It neither replays provider requests nor
applies an output whose dispatch checkpoint failed. Errors before any successful
emission keep their original variant. Checkpoint-collision tests cover a worker
artifact and candidate-bound reviewer evidence.

The fixed planner/worker/two-reviewer pipeline, CAS read/context accounting and
candidate-bound final acceptance are implemented; see `sealed-pipeline.md`.
Durable recovery after process termination and live provider/sandbox acceptance
remain pending. In-memory error retention does not cover a crash during an
adapter transaction or replace an atomic persistent task-output journal.
