# Communication-cost pilot (development only)

The first live missions showed MW/0 using about 30 times the prose baseline's
communication tokens. This pilot finds where that cost comes from and reduces
it through implementation changes only. The Appendix C counting rule does not
change: everything that enters a model's context is counted on every call where
it enters. Per Appendix C.4 the -25% threshold can be revised only before the
freeze, and not on the basis of these development data alone.

The data here are development evidence from one toy mission. They are not
benchmark data and support no claim about the thesis.

## Method

- **Mission.** One frozen development mission: change a one-line greeting,
  with a protected `check.sh` verifier in Docker. The normal condition runs
  with no source view. The pilot uses the same `runtime.json` throughout.
- **Providers.** Claude Code Max (`claude-sonnet-5`) as planner, worker and
  reviewer A; Codex CLI (`gpt-5.6-sol`) as reviewer B; `--subscription-only`.
- **States.** Each state adds one change on top of the previous one:

  | State | Change |
  | --- | --- |
  | 0 | Published commit `6854d43` |
  | A | Text artifacts rendered as UTF-8 (`render_artifact`) |
  | C | Batched retrieval (`fetch_many`) |
  | B | Compact rendering |

  Every change touches only shared tools or rendering, identically in both
  pipelines, so the baseline is remeasured in every state.
- **Samples.** Two live runs per state and pipeline, and a third only when the
  two diverge sharply or the outcome changes qualitatively. Each state uses one
  frozen binary.
- **Recorded per run.** Communication and context reference tokens, calls,
  model actions by tool, communication tokens by segment class, CAS bytes, wall
  time and outcome.
- **C2.** A decision after C, driven by the data: if the remaining calls are
  mostly separate actions rather than reads, the next step is responses that
  carry several actions.

## Results

### State 0

| Pipeline | Run | Outcome | Calls | Comm tokens | Context tokens | Actions |
| --- | --- | --- | --- | --- | --- | --- |
| MW/0 | 1 | COMPLETE | 37 | 90,803 | 128,373 | 25 fetch, 3 put_artifact, 2 emit_delta, 1 emit_claim, 2 review_claim, 1 submit_goal, 3 finish |
| MW/0 | 2 | COMPLETE | 43 | 130,431 | 174,464 | 31 fetch, 4 put_artifact, 1 emit_delta, 1 emit_claim, 2 review_claim, 1 submit_goal, 3 finish |
| Prose | 1 | COMPLETE | 15 | 2,738 | 14,443 | 4 fetch, 3 send_message, 1 put_artifact, 1 write_file, 2 submit_review, 4 finish |
| Prose | 2 | COMPLETE | 15 | 2,493 | 14,116 | same |

- **Medians.** MW/0 has 110,617 communication tokens over 40 calls; prose has
  2,616 over 15 calls, a ratio of about 42 times.
- **MW/0 token composition.** MW_RENDER is 45–48%, CAS_REFERENCED about 31%,
  PROTOCOL_SCHEMA about 21%. Schema and TASK are re-sent on every call, and each
  fetched item is recounted on every later call.
- **Call composition.** `fetch` accounts for 25–31 of MW/0's 37–43 calls.
  Retrieval round-trips, not reviews or other actions, dominate.

### State A: text rendering

| Pipeline | Run | Outcome | Calls | Comm tokens | Context tokens | Actions |
| --- | --- | --- | --- | --- | --- | --- |
| MW/0 | 1 | COMPLETE | 42 | 81,091 | 116,544 | 30 fetch, 3 put_artifact, 2 emit_delta, 1 emit_claim, 2 review_claim, 1 submit_goal, 3 finish |
| MW/0 | 2 | COMPLETE | 41 | 71,111 | 109,593 | 29 fetch, 3 put_artifact, 2 emit_delta, 1 emit_claim, 2 review_claim, 1 submit_goal, 3 finish |
| Prose | 1 | PARTIAL (NO_DELIVERY) | 12 | 1,268 | 10,177 | Codex reviewer B answered `finish` on its first call with no verdict |
| Prose | 2 | COMPLETE | 16 | 2,625 | 14,972 | 5 fetch, 3 send_message, 1 put_artifact, 1 write_file, 2 submit_review, 4 finish |
| Prose | 3 | COMPLETE | 17 | 2,706 | 16,258 | 6 fetch, 3 send_message, 1 put_artifact, 1 write_file, 2 submit_review, 4 finish |

- **MW/0 median** fell from 110,617 to 76,101 communication tokens, -31%.
- **Where the saving came from.** CAS_REFERENCED fell from about 29–40k to
  12–14k per run.
- **What did not change.** Calls stayed at about 41 and `fetch` still accounts
  for 29–30 of them. Text rendering reduces the size of what is read, not the
  number of round-trips.
- **Prose.** The failed prose run is a model-behavior outcome (NO_DELIVERY,
  which counts as a structured-output failure). The third run completed with
  2,706 communication tokens; the median over the three runs is 2,625. The
  MW/0/prose ratio is about 29 times.

### State C: batched retrieval

| Pipeline | Run | Outcome | Calls | Comm tokens | Context tokens | Time | Actions |
| --- | --- | --- | --- | --- | --- | --- | --- |
| MW/0 | 1 | COMPLETE | 20 | 49,909 | 71,750 | 246 s | 8 fetch_many, 1 fetch, 3 put_artifact, 1 emit_delta, 1 emit_claim, 2 review_claim, 1 submit_goal, 3 finish |
| MW/0 | 2 | COMPLETE | 20 | 50,863 | 73,209 | 290 s | 9 fetch_many, 3 put_artifact, 1 emit_delta, 1 emit_claim, 2 review_claim, 1 submit_goal, 3 finish |
| Prose | 1 | COMPLETE | 13 | 2,101 | 14,181 | 117 s | 3 fetch_many, 2 send_message, 1 put_artifact, 1 write_file, 2 submit_review, 4 finish |
| Prose | 2 | COMPLETE | 14 | 1,885 | 15,556 | 174 s | 3 fetch_many, 1 fetch, 2 send_message, 1 put_artifact, 1 write_file, 2 submit_review, 4 finish |

- **MW/0.** The median fell from 76,101 to 50,386 communication tokens (-34%),
  calls halved (41.5 to 20), and wall time roughly halved.
- **Prose.** The median fell from 2,625 to 1,993 tokens. The ratio is now about
  25 times.
- **Remaining MW/0 calls.** 9–10 are reads and 11 are separate actions:
  submit_goal, three put_artifact, emit_delta, emit_claim, two review_claim and
  three finish.

Per-agent decomposition of MW/0 run 2 (communication tokens):

| Agent | Calls | MW_RENDER | PROTOCOL_SCHEMA | CAS_REFERENCED | Total |
| --- | --- | --- | --- | --- | --- |
| Planner | 2 | 1,336 | 6,144 | 339 | 7,819 |
| Worker | 6 | 6,594 | 6,576 | 1,515 | 14,685 |
| Reviewer A | 6 | 8,763 | 1,116 | 4,102 | 14,158 |
| Reviewer B | 6 | 8,781 | 1,116 | 4,127 | 14,201 |

Per-call fixed costs:
- **Planner schema.** About 3,070 tokens per call. The schema depends on the
  catalog and carries the whole Goal IR.
- **Worker schema.** About 1,096 tokens per call for the `emit_*` declarations.
- **Reviewer schema.** 186 tokens per call for `review_claim`.
- **TASK rendering.** About 300 tokens, most of them hexadecimal CIDs.

Reviewer cost is dominated by MW object renderings recounted on every call.
Most of their tokens are 64-hex CIDs.

What these numbers imply:
- **The floor.** A lower bound does not depend on the call count: each agent
  must receive its schema and TASK at least once. Even at one call per agent,
  MW/0 would count roughly 5–6k tokens on this mission against about 2k for
  prose.
- **C2.** The remaining round-trips are about half actions, so by the agreed
  criterion C2 (several actions in one response) is justified. It would mainly
  remove the separate `finish` and emit calls.
- **B.** Compact rendering should target the planner and worker schemas and the
  textual CID representation. The specification leaves the latter open ("text
  representation is not semantic").

### Step B1: planner schema without catalog CIDs (deterministic)

The planner schema no longer enumerates catalog CIDs. It constrains each
reference's kind and CID syntax, and the catalog still checks every action.
The effect needs no live run. On the pilot mission, measured with
`examples/schema_tokens` on each state's `prepared.json`, the planner schema
fell from 3,091 to 1,566 reference tokens per call (-49%). At about two planner
calls per mission, that saves about 3,000 communication tokens, roughly 6% of
state C's median.

### Step C2: several actions per response (implemented, live runs pending)

Both pipelines now use the envelope `{"actions":[...]}`, with 1–8 actions
applied in order. A reference whose `cid` is `@k` names the object of that kind
created by action `k` of the same response, so `put_artifact` can feed
`emit_delta`, `review_claim` or `write_file` in one turn. `finish` must be last.

A rejected action leaves the earlier actions of the response applied; the
normal repair and failure rules then apply to the whole response. Each applied
action produces its own receipt segment with its usual provenance class. The
MW/0 planner keeps its single-action envelope, since it makes about two calls.
Both system prompts carry the same guidance on `fetch_many`, multiple actions
and `@k`.

### State C2: B1 and several actions per response (live)

| Pipeline | Run | Outcome | Responses | Comm tokens | Context tokens | Time |
| --- | --- | --- | --- | --- | --- | --- |
| MW/0 | 1 | PARTIAL (UNAVAILABLE/infrastructure) | 13 | 27,683 | 39,204 | 457 s |
| MW/0 | 2 | COMPLETE | 13 | 28,001 | 39,229 | 276 s |
| MW/0 | 3 | COMPLETE | 13 | 27,683 | 38,865 | 158 s |
| Prose | 1 | COMPLETE | 7 | 1,024 | 7,367 | 66 s |
| Prose | 2 | COMPLETE | 8 | 891 | 8,489 | 55 s |

- **Same actions, fewer responses.** The actions emitted were the same as in
  state C, packed into fewer responses.
- **MW/0 run 1.** Every agent finished, but the verifier container timed out
  while the host was heavily loaded by unrelated work (load average about 16),
  and the executor's bounded cleanup also failed. The run was classified
  UNAVAILABLE/infrastructure, with no false success. The leftover container
  carried Myr's ownership label and was removed manually. A third run was made
  under the pilot rule. Its token counts are unaffected by the sandbox failure.
- **Medians.** MW/0 fell from 50,386 to 27,683 communication tokens (-45%) and
  from 20 to 13 responses. Prose fell from 1,993 to 958 (-52%) and from 13.5 to
  7.5 responses. The ratio stays at about 29 times.

Per-agent decomposition of MW/0 run 2 (communication tokens):

| Agent | Responses | MW_RENDER | PROTOCOL_SCHEMA | CAS_REFERENCED | Total | First response alone |
| --- | --- | --- | --- | --- | --- | --- |
| Planner | 2 | 1,336 | 3,142 | 339 | 4,817 | 2,239 |
| Worker | 3 | 2,817 | 3,450 | 498 | 6,765 | 1,449 |
| Reviewer A | 4 | 5,091 | 792 | 2,254 | 8,196 | 534 |
| Reviewer B | 4 | 5,103 | 792 | 2,269 | 8,223 | 537 |

## Findings so far

1. **Implementation, not the protocol, caused most of the absolute cost.** MW/0
   went from 110.6k to 27.7k communication tokens (-75%) and from 40 to 13
   responses, with unchanged counting rules. Base64 rendering, one-object reads
   and one-action responses were most of the cost.
2. **Shared-tool improvements help both pipelines alike.** Text rendering,
   batched reads and multi-action responses are tools shared by both
   pipelines, so the baseline improved too. The MW/0/prose ratio fell from
   about 42 times to about 25–29 times and then stopped moving. What remains is
   the cost of the protocol itself: the MW/0 schemas, typed object renderings
   with hexadecimal CIDs, and the reviewers' navigation of the reference graph.
3. **On this mission the floor is far from the threshold.** With one response
   per agent, MW/0 would still count about 4.8k tokens against about 1k for
   prose. The toy mission's messages are tiny, which favors prose. Whether the
   ratio falls with realistic artifacts, where prose messages quote code and
   grow with the task, is the question for a second pilot.
4. **The remaining lever is the text form of references (B2).** The
   specification leaves it open ("text representation is not semantic").
   Hexadecimal CIDs cost about 40 tokens each and dominate MW_RENDER, receipts
   and the reviewers' context. This change touches the protocol's text surface
   in both pipelines and needs an owner decision.

### Step B2: short references in model-facing text (implemented, live runs pending)

The specification leaves the text form of CIDs open ("text representation is
not semantic"). Each mission run keeps one table, held by its dispatcher, that
maps `#n` to full CIDs in first-appearance order:

- **One table per mission.** Every agent of the run shares it, so `#7` names
  the same object even when an agent quotes it in a message.
- **Never in shared state.** Graph, storage, validation, reports and semantic
  records always use full CIDs. The table is journaled only in the private
  dispatch journal, so retained contexts can be audited.
- **What is shortened.** Reference fields in MW renderings, receipts, catalog
  listings and repository listings. Also full CIDs inside the text of runtime
  records: the sealed goal and runtime artifacts such as candidate manifests,
  review contexts and command records. Repair messages are shortened too.
- **What is never rewritten.** Repository files and agent-authored artifacts.
- **Private source view.** Its listing and reads stay unaliased. Otherwise
  alias numbering, or a CID left in full, would hint which files differ
  between the views.
- **Resolution.** In every model response, reference-shaped objects are
  resolved from `#n` to the full CID before any validation. Free text is never
  resolved. `@k`, the reference to an earlier action of the same response,
  stays distinct, and full CIDs remain accepted.

Both pipelines use the same table mechanism and rendering rules.

### State B2: short references (live)

All runs used the host preflight (`--max-load-per-cpu 0.75`,
`--min-available-memory-mib 4096`).

| Pipeline | Run | Outcome | Responses | Comm tokens | Context tokens | Time |
| --- | --- | --- | --- | --- | --- | --- |
| MW/0 | 1 | COMPLETE | 14 | 20,802 | 29,821 | 134 s |
| MW/0 | 2 | COMPLETE | 13 | 18,835 | 27,040 | 145 s |
| Prose | 1 | COMPLETE | 7 | 584 | 6,250 | 40 s |
| Prose | 2 | COMPLETE | 7 | 583 | 6,249 | 40 s |

- **MW/0.** The median fell from 27,683 to 19,819 communication tokens (-28%).
  MW_RENDER fell from about 14k to 7.3–7.9k and CAS_REFERENCED from about 5.4k
  to 2.6–2.8k. PROTOCOL_SCHEMA (8.9–10.1k) is now the largest MW/0 component,
  at 45–49%.
- **Prose.** The median fell from 958 to 584 (-39%), because its candidate
  listings also carried full CIDs. The ratio is about 34 times.

### Summary across states (medians)

| State | MW/0 comm tokens | MW/0 responses | Prose comm tokens | Ratio |
| --- | --- | --- | --- | --- |
| 0 | 110,617 | 40 | 2,616 | about 42 times |
| A | 76,101 | 41.5 | 2,625 | about 29 times |
| C | 50,386 | 20 | 1,993 | about 25 times |
| C + B1 + C2 | 27,683 | 13 | 958 | about 29 times |
| C + B1 + C2 + B2 | 19,819 | 13.5 | 584 | about 34 times |

Implementation work removed 82% of MW/0's initial communication cost. Every
step also served the prose baseline, since the tools are shared, and on this
toy mission the ratio no longer improves. The remaining MW/0 cost is almost
entirely fixed protocol cost:
- schemas re-sent on every response (planner about 1.6k, worker about 1.1k,
  reviewers about 0.2k);
- typed renderings;
- the reviewers' reads of runtime records.

The toy mission cannot say whether that fixed cost amortizes as tasks grow.
That question belongs to the second pilot, a scale curve with realistic
artifacts.
