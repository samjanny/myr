# Budgeted planner execution

`planning::run` executes only the planner provider selected in the runtime policy.
The caller supplies the mission, policy, explicit catalog and a dispatcher that
can continue into later stages. The function does not create or reset that
dispatcher. It also enforces local policy call/token/time limits so a dispatcher
with a larger allowance cannot expand planner authorization.

The initial context contains the original mission and a rendered catalog of
registered predicates, ATOMs and permitted artifact references. Every action
uses the catalog's current schema. Created references become tool receipts;
fetches expose artifact bytes as base64 or render typed MW objects. Repository
file reads identified by the baseline manifest use DIRECT_REPO accounting;
other artifact fetches use CAS_REFERENCED. Logical fetch byte counts are persisted
separately from injected-context tokens. This does not count every internal CAS
integrity check as agent traffic.

The common dispatcher retains exact contexts, outputs, usage and checkpoints in
private audit storage before semantic processing. Invalid schema or semantic
proposals allow two repair requests; a third invalid response creates runtime
FAIL. Out-of-catalog access stops immediately with CAPABILITY_DENIED. Provider
failure does not retry, change models or switch billing. Budget exhaustion and
inactive catalog references produce explicit FAIL records. Storage/integrity
errors propagate rather than being disguised as model mistakes.

A successful result contains the immutable goal seal and created references.
The planner's provider configuration never comes from model output. Failure does
not classify the user's mission as UNSAT or INVALID_GOAL merely because a model
could not produce a valid plan. A goal seal written immediately before deadline
exhaustion can remain historical data even though planning returns TIME_BUDGET.

Tests inject provider responses through the real dispatcher and catalog boundary.
They fetch repository content, create an ATOM, submit a mission-bound seal, verify
schema refresh and accounting, and prove the same dispatcher's consumed budget
remains consumed afterward. Additional cases cover repairs, access denial,
transport failure and a tighter local call limit. No live provider call is used.

`pipeline::plan_and_run` now connects this planner to the post-seal pipeline using
one dispatcher. Handoff tests verify six total calls for a synthetic successful
mission and PARTIAL when the shared allowance is exhausted sooner. Initial
catalog/policy preparation and `myr run` remain to be exposed.
