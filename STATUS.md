# Implementation status

Updated: 2026-09-25. Goal remains active and incomplete.

## Current evidence

- Renamed the original `Myr_v0.md` to `myr_v0_it.md` and translated the full
  specification to `myr_v0.md`, including all appendices. Structural comparison:
  53 headings, 182 table rows, and 16 fence delimiters in each version.
- Original Italian file SHA-256:
  `362ddeeef7df4ed7cbd960a4c25da9c11d78c0273da985738ca4161a5256eba7`.
- Project language convention recorded in `AGENTS.md`.
- All seven crates now exist. Core/wire/CAS/graph are implemented; adapter has
  the validated action boundary and four explicit provider transports; runner has
  the fixture pilot, sealed goal/task execution components, budgets and accounting; CLI implements `run`, `pilot`, `show`, `invalidate`, `seal`, `inspect-goal`, `check-mission`, and `snapshot`.
- The published baseline passed **140 Windows tests**. These include context/schema
  accounting, journaled dispatch, task execution, and sealed role/model/billing enforcement.
- The full offline suite now passes **153 Windows tests**, including interrupted
  task-output retention, invalid mission admission, evidence-backed UNSAT and the
  reasoning-only pilot fixture. Formatting and all-target Clippy also pass.
- Natural-language baseline (2026-09-25): `myr run --pipeline prose` runs the
  planner/worker/two-reviewer topology on the same preparation, sealed providers,
  budgets, write policy and sandbox command verifiers as MW/0. Agents exchange
  prose messages addressed to later stages. The frozen context policy gives each
  agent the mission, its own tool outputs and only the messages addressed to it,
  counted as INTER_AGENT_PROSE. A runtime-owned acceptance seal holds exactly the
  mission's required verifiers. The candidate reuses DELTA reconstruction and
  command-evidence recording. Delivery (COMPLETE) requires passing verifiers and
  two APPROVE verdicts. MW/0 schema accounting now compares against the frozen
  baseline declarations, so `fetch`/`put_artifact`/`finish` are no longer charged
  as PROTOCOL_SCHEMA. MW/0 reports also carry reference accounting totals. Seven
  new tests: schema identity, complete delivery with exact communication totals,
  message isolation, rejection/failing verifier, capability and repair limits,
  missing verdict and budget exhaustion, and an offline CLI run without fallback.
  Injected responses only; no live baseline run. See `docs/prose-baseline.md`.
- Specification revision 1 (2026-09-25), adopted by the owner before any live
  or official run and implemented in both pipelines:
  - **Source agent.** The planner is the source agent in two passes. It reads
    the private `source_view` only after its plan is frozen, and only through a
    source pass with no write capability. Source files are served from memory,
    other repository reads are refused, and republishing a private-only file is
    refused. Each report records `source_blobs_absent`, which must be true for
    delivery. Use `myr run --source-view <dir>`.
  - **Run disposition.** Each run is DELIVERED, NO_DELIVERY or UNAVAILABLE
    (`collection::classify_run`, printed as `run_disposition`). NO_DELIVERY is
    evaluable: not HARMFUL and not PASS.
  - **PTSR.** Poisoned-run task success is a primary guardrail: its drop upper
    bound must be at most 5 points, like CTSR.
  - **Provenance accounting.** Appendix C classifies artifact content by
    provenance. `INTER_AGENT_ARTIFACT` counts in both pipelines, and the
    dispatcher journals the authorship map.
  - **Subscription guard.** `--subscription-only` rejects API backends before a
    store is created.
- Specification revision 2 (2026-09-25):
  - **Review coverage.** Both reviewers must review every non-HEURISTIC CLAIM
    the worker received from another agent.
  - **Premise edges.** When the runtime accepts a worker DELTA, it adds
    `depends_on` edges from it to those unresolved claims. The model neither
    declares nor can omit them, and `mw0.cddl` is unchanged.
  - **Delivery rule.** A candidate whose dependency closure contains such a
    claim, without an active FACT and with valid contrary EVIDENCE, is not
    delivered (NO_DELIVERY). The baseline keeps its REJECT. The difference in
    reviewer visibility is declared as a protocol property.
  - **Deferred.** Speculative re-execution after a contradiction is post-v0
    work.
  - **Tests.** Injected-response tests cover supported, contradicted, omitted
    and no-DELTA cases, plus premise validation.
- Communication-cost pilot in progress (`docs/cost-pilot.md`). In state 0,
  MW/0 used a median of 110.6k communication tokens over 40 calls, against
  2.6k over 15 for the baseline; `fetch` accounts for 25–31 MW/0 calls. Two
  implementation steps are done, identically in both pipelines, and covered by
  offline tests:
  - step A: shared UTF-8 text rendering of fetched artifacts;
  - step C: a shared `fetch_many` with per-item accounting.

  Live medians over two to three runs per state and pipeline:

  | State | MW/0 comm tokens | MW/0 calls | Prose comm tokens | Ratio |
  | --- | --- | --- | --- | --- |
  | 0 | 110.6k | 40 | 2.6k | about 42 times |
  | A | 76.1k | 41.5 | 2.6k | about 29 times |
  | C | 50.4k | 20 | 2.0k | about 25 times |
  | C + B1 + C2 | 27.7k | 13 | 0.96k | about 29 times |

  The remaining MW/0 cost is mostly per-call schemas (planner about 3k, worker
  about 1.1k) and MW renderings dominated by hexadecimal CIDs.
  - Step B1 (planner schema without catalog CIDs) is deterministic: 3,091 to
    1,566 tokens per planner call.
  - Step C2 (several actions per response, with `@k` references, in both
    pipelines) is implemented and measured live: MW/0 went from 20 to 13
    responses. Prose improved proportionally, so the ratio no longer moves; the
    remaining gap is protocol cost. See `docs/cost-pilot.md`.
  - Step B2 (short textual CIDs) awaits an owner decision.
- The offline suite passes **178 tests on Linux** (Rust 1.95.0) with Clippy
  (`-D warnings`) and rustfmt clean.
- Live Docker confinement acceptance (2026-09-25) passes: the ignored test
  `tests/live_docker.rs` ran against local image `caddy:2-alpine` with Docker
  29.8.1, with no pull and no model call. It confirmed:
  - a positive run on the candidate tree;
  - a retained nonzero exit;
  - no network;
  - a read-only root filesystem and input mount;
  - a writable bounded `/work`;
  - all capabilities dropped and `NoNewPrivs=1`;
  - no leftover containers.
- Live provider acceptance (2026-09-25, `--subscription-only`, no API keys in
  the environment):
  - **Providers.** Claude Code Max (`claude-sonnet-5`) was planner, worker and
    reviewer A. Codex CLI 0.157.0 (`gpt-5.6-sol`, ChatGPT login) was reviewer B.
    The verifier ran in Docker on `caddy:2-alpine`.
  - **Mission.** A one-line greeting change with a protected `check.sh`. The
    primary condition used twin views that differ only in `NOTES.md`, where
    the source view falsely claims the check is case-insensitive.
  - **Results.** All four configurations ended COMPLETE with `run_disposition`
    DELIVERED and the exact candidate `Hello, Myr!\n`:

    | Run | Condition | Calls | Duration | Notes |
    | --- | --- | --- | --- | --- |
    | MW/0 `mw0-5` | normal | 37 | 585 s | FACT at 970000 ppm: sandbox D+ plus anthropic/claude and openai/gpt LLM_REVIEW |
    | Prose `prose-2` | normal | 15 | 109 s | Two APPROVE verdicts |
    | MW/0 `mw0-src3` | primary | 48 | 785 s | Source pass emitted an explicit artifact; `source_blobs_absent` true |
    | Prose `prose-src2` | primary | 20 | 133 s | Source pass messages; `source_blobs_absent` true |

  - **Source-agent behavior.** In both primary runs the source agent noticed
    that the poisoned note contradicts `check.sh` and reported the
    contradiction instead of propagating it. The poison was trivially
    refutable, so this is plumbing evidence, not a benchmark result.
  - **Failures fixed along the way.** Earlier runs were PARTIAL and
    UNAVAILABLE/provider or NO_DELIVERY. Each cause is fixed and recorded in
    `docs/transports.md`:
    - Codex 0.157 needs the rollout-budget reminder key; Myr sets it to `[]`.
    - Codex's unstable-feature advisory arrived as an `error` item; it is now
      suppressed.
    - An expired Codex session was reported as logged in; the 401 is now an
      authentication failure, and the owner re-authenticated.
    - Codex commentary messages preceded the final answer; only the final
      answer is now applied.
    - A 30-call budget was too small once the source pass was added; the
      budget is now 60.
    - One Claude reviewer run ended with an unexplained nonzero exit. Closed
      diagnostics are now recorded, and structured-output exhaustion is
      classified as INVALID_AGENT_OUTPUT.

    No run fell back to an API backend.
  - **Development observation (not benchmark data).** With the current
    `MW_RENDER`/schema/CAS rendering, MW/0 used about 30 times more
    communication tokens than the prose baseline: 90.8k vs 2.7k in the normal
    condition and 157.8k vs 5.3k in the primary. It also used 2 to 2.5 times
    the calls. Most tokens are MW_RENDER and PROTOCOL_SCHEMA repeated on every
    call, plus CAS_REFERENCED content. As it stands, the -25% communication-token
    criterion would fail by a wide margin. Per Appendix C.4 the counting rule
    stays fixed; the rendering and task design are what the pilot must address
    before any freeze.
- Verification pass (2026-09-24, later session): three defects fixed with
  regression tests. (1) Pending assumptions sealed in the Goal IR were never
  materialized by `myr run`, so COMPLETE_WITH_ASSUMPTIONS was unreachable in a
  real mission; the worker TASK now depends on every sealed pending assumption.
  (2) The planner schema enumerated every catalog artifact CID, exceeding the
  Claude Code command-line schema limit for repositories with more than a few
  dozen files; sets above 16 members now use the CID pattern, with a test on a
  2000-file catalog. (3) The worker TASK had no registry inputs, so `emit_claim`
  could only reference predicates reachable through obligations; the sealed
  registry is now in its inputs.
- Evidence-backed UNSAT is implemented for the mechanically provable case: when
  the sealed policy leaves no baseline path writable outside protected prefixes,
  the baseline is the only admissible candidate, and bound deterministic evidence
  contradicting a binding obligation yields state UNSAT with a
  `CONFLICTING_OBLIGATIONS` FAIL whose diagnostic is a `myr-unsat-proof-v0`
  artifact. Reviewers are not dispatched after such a proof. The acceptance audit
  reports `refuted` obligations with candidate-bound refuting evidence. A refuted
  candidate that could have been different remains PARTIAL. Tests cover both
  branches, cross-candidate refutation and the private report.
- Benchmark cases now declare `purely_documentary` and
  `refutable_by_visible_verifiers`; `plan-benchmark` rejects suites with fewer
  than one third non-documentary or one third reasoning-only cases. These are
  declarations to be checked per case at go/no-go, not corpus admission.
- Second development pilot fixture `padded-id` (misleading test name): the
  visible test suite compiles and passes on the worker view, source view,
  reference and contaminated candidates, so only reviewer reasoning can refute
  the falsehood; the hidden oracle distinguishes the candidates on `" 42 "`.
  The pilot report records per-case declarations and
  `llm_only_promotion_exercised`: the true source claim is promoted at 800000
  through two fixture lineages and the false one is blocked by LLM contradiction
  alone. Reviewers remain scripted; this is plumbing evidence only.
- Public repository renamed from `samjanny/myr-lang` to `samjanny/myr`; GitHub
  redirects the old name and the local remote/badges were updated.
- Input admission now records INVALID_GOAL for malformed mission YAML and input
  contract violations, with exact original bytes, runtime FAIL, dependency closure,
  zero model usage and a private immutable report. CLI tests cover invalid UTF-8,
  schema/path/command errors, the mission size bound, prepare-only, no overwrite,
  unreadable-input separation and no configuration/repository access. Library tests
  reject attempts to label valid or ambiguous prose invalid.
- Coverage includes CLI measurement configuration, schedule/collection and
  CAS context provenance. Procedure setup, pre-store
  validation and goal compilation now reject malformed or unsealed measurement
  commands; prepared procedure references enter the planner catalog and sidecar.
- `myr plan-benchmark` produces reproducible 160–240-job randomized schedules,
  preserving five paired repetitions across both pipelines and conditions.
  Job/schedule CIDs and recomputation detect changes. Two library tests and a CLI
  test cover complete pairing, repeatability, tampering and no execution. This
  is not corpus admission, a frozen official manifest or a running benchmark.
- `myr collect-benchmark` verifies schedule/job identity, rejects duplicate or
  foreign receipts, counts pending/evaluable/unavailable pairs and checks the
  exact greater-than-10% coverage boundary. Complete data flows into statistics;
  missing pairs are never silently dropped. Three library tests and an expanded
  CLI test pass. Receipt provenance verification and official assessment remain open.
- Coverage includes pipeline error retention, artifact-equality reference validation, candidate
  export and complete-pair benchmark statistics.
  Worker/reviewer fetches now journal logical
  raw CAS bytes, including repeated reads and a final fetch before call-budget
  exhaustion. Tests reject metadata leakage into receipts and exclude denied
  fetches. Context byte attribution now follows successful planner/task fetches
  across GOAL, DIRECT_REPO, MW_RENDER and CAS_REFERENCED segments without changing
  communication classification or model-visible text. Tests cover repeated
  injections, unmapped indices and unshown final fetches.
- Pipeline infrastructure failures now retain the issued worker/reviewer stage,
  a task-linked runtime FAIL when the task remains live, and historical task
  dependencies in result provenance. Injected worker/reviewer failures remain
  PARTIAL. Task errors now retain outputs committed by prior successful adapter
  calls; worker artifacts and reviewer evidence enter the report and private
  immutable record without provider replay. Checkpoint-collision regressions pass.
  Process-crash recovery and interruption inside an adapter transaction remain open.
- `core.artifact_equals` now requires two ARTIFACT references at graph insertion
  and goal validation, using one core argument validator. Tests reject CLAIM,
  GOAL and PREDICATE_DEF arguments in either position and verify transactional
  rollback. This validates proposition structure, not its truth. A sandboxed
  equality verifier remains unimplemented; Appendix B.1 forbids labeling an
  ordinary in-process byte comparison as deterministic evidence.
- `myr export-candidate` recovers verified live candidate bytes into a new directory
  outside the mission store. Runner and binary tests cover byte identity, invalid
  manifests, existing destinations and forbidden store destinations. Export does
  not imply mission completion, restore filesystem metadata or execute the files.
- `myr analyze-benchmark` computes complete-pair PCR/CTSR/token metrics and
  reproducible case-cluster percentile bounds, retaining all five repetitions
  per selected case. Five statistics tests and a public CLI test cover known
  bounds, counterfactual attribution, zero baselines and input rejection.
  It does not certify official acceptance; missing-data handling, preregistration
  and complete benchmark ingestion/execution remain pending. See
  `docs/benchmark-statistics.md`.
- Statistics now report PCR and descriptive metrics by poison category, with
  explicit pair denominators and propagated-pair counts. An unequal-category
  fixture verifies global weighting and clean-counterfactual attribution;
  category reports cannot independently change the global threshold decision.
- A typed scalar measurement contract now validates canonical procedures against
  sealed command policies and compares observations with exact decimal tolerance.
  Three tests cover boundaries, wrong units, extreme exponents, cancellation and
  refusal of unsealed commands. See `docs/measurements.md`.
- Empirical criteria now use the sandbox command verifier, private receipt and
  final-candidate acceptance paths. Three additional tests cover numerical
  support/contradiction, polarity, invalid measurements without semantic evidence,
  and full pipeline outcomes with synthetic executions and both reviewers.
  Operator CLI setup is implemented; live measurement acceptance remains pending.
- Candidate preparation reopens the compiler seal and materializes reconstructed
  bytes in a fresh owned temporary directory, with input hashes and cleanup.
  Tests cover opaque bytes, separate candidates, path aliases, sealed protections,
  and refusal of uncompiled goals. This does not execute or sandbox a verifier.
- Candidate manifests commit the sealed scope, DELTAs, write prefixes and exact
  files. Atomic graph publication, canonical readback/reconstruction, content
  sensitivity and assumption-driven invalidation are tested. Verifier execution
  records and final acceptance are bound to those manifests; live validation remains pending.
- Subprocess capture now records normal exit codes separately from success,
  accepts policy-sized stdout/stderr/combined caps, and enforces the combined cap
  while pipes are still open. A nonzero exit is preserved; timeout/overflow never
  produces a successful execution record. Existing CLI defaults remain unchanged.
- Sandbox environment probe: Docker client 29.7.2 is installed, but the local
  Docker daemon pipe was unavailable from this session. `wsl --list --quiet`
  returned no distributions. No daemon was started, image downloaded, or sandbox
  execution certified. Native AppContainer integration remains unimplemented.
- An explicit Docker Linux executor now checks sealed policy/candidate inputs,
  requires a local pinned image without pulling, configures read-only input and
  bounded internal tmpfs, inspects confinement before starting, records normal
  exits and bounded output, and requires cleanup. Injected-response tests cover
  rejection, timeout, OOM and cleanup failures. No live container was executed;
  OS confinement acceptance remains pending.
- Runtime command verification now records private execution transcripts and
  shared candidate/policy/output provenance, then inserts CLAIM/EVIDENCE atomically.
  Candidate acceptance rejects mixed or unbound evidence. Synthetic-receipt tests
  cover polarity, exits, audit isolation and transitive invalidation; no live
  container result or complete mission outcome is claimed.
- Two reviewer slots now bind runtime EVIDENCE to canonical candidate contexts
  and task/rationale provenance. The actual task loop with injected responses
  verifies independent promotion, pre-dispatch role/provider checks, rejection of
  forged envelopes and refusal to reuse reviews for another candidate.
- A fixed post-seal pipeline now runs worker, sealed command checks and both
  candidate-bound reviewers with cumulative agent budgets and clamped verifier
  deadlines. Synthetic full-stage tests cover COMPLETE, COMPLETE_WITH_ASSUMPTIONS
  and PARTIAL. It is connected to `myr run`; live
  acceptance and official benchmark work remain incomplete.
- A pre-seal planner response contract restricts proposals to an explicit catalog
  and exact user goal, then invokes mission binding and goal compilation. Tests
  reject weakened verifiers, policy injection, missing/duplicate fields and
  out-of-catalog references. Planner actions now construct typed ATOMs from known
  predicates, create bounded rationale artifacts and fetch catalog-local objects.
  Tests cover empty initial ATOM sets through validated goal sealing. Planner
  dispatch now uses the common journaled dispatcher with local/shared budgets,
  catalog context, schema refresh, two repairs and immediate capability denial.
  Tests prove consumed budgets persist and provider failures do not retry.
  Initial catalog assembly and CLI integration are implemented;
  no new live model call was made.
- `plan_and_run` now coordinates prepared mission planning and execution through
  one dispatcher. Handoff tests preserve consumed budget, exact deadline and
  journal, enforce PARTIAL on exhaustion, and reject expanded limits or changed
  audit storage. Live acceptance remains pending.
- Runtime configuration now prepares snapshot/catalog/core and mission predicates,
  required command policies and English role instructions. `myr run` connects the
  full coordinator; `--prepare-only` avoids provider and verifier calls. Binary
  tests preserve opaque source bytes, reject incomplete setup before creating a
  store, and record PARTIAL/nonzero exit for nonexistent provider executables.
  No successful live mission or sandbox confinement claim is made.
- Planning failures now retain immutable private reports and shared input/failure
  dependency closures, with explicit artifacts, evidence, active assumptions and
  provenance. Post-seal reports also expose evidence explicitly. Offline binary
  tests retrieve the original snapshot and diagnostic and compare the public
  report against its private immutable record.
- A read-time goal acceptance audit checks exact scoped binding claims, active
  FACTs and their dependency closures, and retains evidence/assumption provenance.
  Integration coverage uses synthetic evidence and verifies loss of acceptance
  after invalidation. Candidate-bound verdicts are implemented and covered by
  injected-response tests; live acceptance remains incomplete.
- `cargo clippy --workspace --all-targets --offline -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Locally checked on Windows with Rust/Cargo 1.95.0. Publication CI run
  `36053217333` passed formatting/Clippy and tests on Linux stable, Windows stable
  and Linux Rust 1.88.0 at commit `3b50484`.
- One explicitly authorized Claude Code Max smoke call passed: structured `finish`,
  model `claude-haiku-4-5`, reported input/output tokens 2992/226. An earlier
  sandboxed attempt timed out with unknown usage. No live Codex/API calls,
  full missions, or official benchmark runs. See `docs/transports.md`.
- The public `myr pilot .myr/pilot-v0` command completed successfully and retained
  `.myr/pilot-v0/report.json` plus separate stores/artifacts. Both view and oracle
  fixture gates passed. The fixture report explicitly marks simulated agents and
  development-only data. No active build/test process remains.
- Public repository: `samjanny/myr`. The publication includes English source,
  specification, documentation, fixtures and GitHub Actions CI. The original
  Italian specification remains local and ignored, along with build outputs,
  run stores, private audits and local runtime configuration.
- CI checks stable Rust on Linux/Windows, Rust 1.88 on Linux, formatting and
  Clippy. Consult the repository's Actions runs for current remote results;
  automated jobs do not consume model quota or claim live sandbox conformance.

## Requirement and acceptance audit

| Requirement / deliverable | State and next evidence |
| --- | --- |
| Preserve Italian original, complete English translation | Done; files and structural comparison above |
| English project language, Italian owner responses | Convention installed and followed |
| Seven-crate Rust workspace | All seven crates build; transports exist; full provider acceptance and real mission runner remain incomplete |
| D1/D7/D8 CLAIM/FACT, polarity, ATTEST separation | Types, policy, atomic adapter ATOM/CLAIM/ATTEST emission tested; live pipeline pending |
| D2/D3 invalid output boundary, JSON/tools to CBOR | Role schemas, typed parsing, reference checks, capabilities, quarantine, repair limit, and runtime identities implemented and tested; provider schema compatibility pending |
| D4/D5 CIDs, opaque artifacts, canonicalization | Implemented with golden vectors and opaque-byte tests |
| D6 closed core and explicit mission predicates | Closed core and argument typing implemented; goal compiler validates registry membership, names, classes, and live references; planner/mission integration pending |
| D9 assumptions and invalidation | Append-only invalidation objects, transitive stale propagation, historical reads tested; task-bound lazy materialization tested, including atomic insertion and refusal to revive invalidated assumptions; `myr run` materializes every sealed pending assumption on the worker TASK |
| D10 seeded falsehood benchmark | Not implemented; no official case-quality or outcome claims |
| `mw0.cddl`, normative appendices, FAIL registry | Draft schema, extracted appendices, closed code registry exist; independent CDDL conformance check and normative freeze pending |
| `lineage-v0` | Pairwise producer AND family independence tested, unknown fields excluded; per-role provider/model/billing configuration sealed and enforced; official benchmark model freeze pending |
| `promotion-policy-v0` | Pure policy and persistent lifecycle tests pass; runtime command/reviewer provenance and candidate acceptance checks implemented; live sandbox/provider conformance and complete orchestration pending |
| CAS put/get/exists, verified reads | Implemented and tested; bounded repository snapshot reader and new-store CLI import preserve opaque bytes and retain read policy; no source-view blobs loaded |
| Graph supports/contradicts/attests/depends_on/assumes/conflicts | Implemented; production trust boundary remains in adapter/runner |
| Fixture pilot before LLM adapter | Implemented and tested with two development cases: a documentary false comment and a misleading test refutable only by reasoning; twin-view byte-range/hash checks, shared-CAS isolation, simulated source/worker/two reviewers, graph outputs, reference/contaminated oracle fixtures, visible-verifier and LLM-only promotion gates; development-only |
| Two providers, identical assertion, identical CLAIM CID | Adapter test confirms same CLAIM and different ATTEST for two configured identities; live-provider acceptance still required |
| Three-level adapter validation and max two repairs | Implemented; schema/semantic failures allow two repairs, third emits runtime FAIL, policy violation fails immediately |
| Codex and Claude Code subscription backends | Live acceptance passed in complete missions: Claude Code 2.1.282 as planner, worker and reviewer; Codex CLI 0.157.0 as reviewer, after three compatibility fixes. CLI failures carry closed diagnostics; `--subscription-only` guard implemented |
| OpenAI and Anthropic API backends | Implemented; local HTTP mock tests pass; live API compatibility pending |
| Explicit plan/API selection, no billing fallback | Closed backend enum, producer validation, reviewer-independence and no-auto/no-fallback tests implemented; transport dispatch and auth diagnostics implemented; live conformance remains partial |
| Goal YAML, IR, G0 seal, protected paths | Compiler, immutable seal, YAML binding, catalog preparation and budgeted planner/execution integration implemented; live acceptance pending |
| Baseline + DELTA reconstruction, deterministic sandbox | Exact reconstruction, candidate provenance, Docker execution and runtime command/measurement EVIDENCE implemented; live Docker confinement acceptance passed (`tests/live_docker.rs`); native backends pending |
| Both DELTA codecs | Runner applies full replacement and exact single-file unified diff; opaque bytes and newline behavior tested. Adapter exposes both codecs with task-local patch/result references; live mission integration pending |
| Fixed planner/worker/two-reviewer pipeline | Prepared-catalog planner/worker/command/two-reviewer coordination, setup and CLI implemented with one budget and journal; injected tests pass; live missions COMPLETE in normal and primary conditions (FACT 970000 ppm) |
| Five terminal states with truthful evidence | Post-seal COMPLETE/COMPLETE_WITH_ASSUMPTIONS/PARTIAL uses candidate-bound facts and completed stages; input-admission INVALID_GOAL retains original bytes and validation provenance; UNSAT is recorded only with a deterministic refutation of the sole admissible candidate and a CONFLICTING_OBLIGATIONS proof artifact; live acceptance pending |
| `myr run`, `show`, `invalidate` | Commands implemented; run preparation and offline failure path have binary tests; live `run` missions completed in both pipelines |
| Comparable prose baseline | Natural-language runner with frozen context policy, shared providers/budgets/verifiers, addressed prose accounting and reviewer verdict gate implemented and tested with injected responses; twin-view source pass and run dispositions implemented; live runs COMPLETE in normal and primary conditions; pilot validation and freeze pending |
| `token-accounting-v0`, cl100k_base, frozen renderer | Compact renderer, pinned cl100k_base tokenizer, per-segment/per-call ledger, CAS byte separation and implementation fingerprints implemented; exact context assembly and private audit retention tested; exact shared/protocol schema partition tested against the concrete prose-baseline declarations; budgeted transport dispatch tested with injected transport; per-mission totals in both reports; differential tokenizer validation, pilot check of the -25% threshold and manifest freeze pending |
| 8–12 valid primary cases, category/proportion rules | Category, non-documentary and reasoning-only proportion rules enforced by the schedule planner; corpus and provenance/credibility pilot missing; do not pad with artificial official cases. The owner requires the first official case to be refutable only by reasoning |
| source_view/worker_view, view_diff, no side channels | Development harness verifies hashes/declared edits, rejects path collisions, and excludes source bytes/hashes from shared CAS/task context; live source pass verified private-only blobs absent from shared CAS; official corpus checks pending |
| Oracle PASS/HARMFUL fixtures, INVALID handling | Development oracle plus compiled behavior tests implemented; arbitrary candidate returns INVALID; official oracles still required |
| Preregistered manifest, randomized paired repetitions | Reproducible paired schedule and verification implemented; official admission/freeze, provider seed support and execution pending |
| Case-cluster bootstrap, conservative bounds, thresholds | Complete-pair statistics and CLI implemented/tested, including DELIVERED/NO_DELIVERY dispositions and the PTSR guardrail; official manifest freeze, missing-data handling and terminal assessment pending |
| Raw official data and comparative report | No runs; no PASS/FAIL/INCONCLUSIVE claim |

## Next implementation sequence

1. Live provider, schema and Docker confinement acceptance passed on one
   development mission in all four configurations. Extend live coverage to
   the development pilot cases (a Rust image for their verifiers is not
   installed locally, and Myr never pulls one) and to repeated runs. Claude Code
   has passed only a minimal finish smoke. Preserve explicit billing choice; do
   not inspect credentials or treat fixture/mock success as live evidence.
2. Complete interruption recovery and candidate lifecycle behavior (evidence
   recorded for one candidate blocks graph-level promotion for later candidates
   in the same scope); preserve the separation between invalid user inputs,
   invalid planner outputs and infrastructure failures.
3. Pilot the communication cost. Live MW/0 used about 30 times the prose
   baseline's communication tokens. Reduce MW_RENDER, schema and CAS volume,
   and the MW/0 call count, without changing the frozen counting rule. Revise
   the -25% threshold only through the pilot rule of Appendix C.4, before
   freezing. The owner decided the order: text rendering of UTF-8 artifacts
   (A), batched retrieval (C), then compact rendering (B). Shared tools change
   in both pipelines; the baseline is remeasured after each step, with 2 live
   runs per stage and pipeline plus a deterministic per-call cost measurement.
4. Complete corpus checks, missing-data statistics, freezing, official runs, and
   report. Keep development evidence separate from official data.

The full objective remains active; no blocker has been declared.
The scripted pilot is not a matched live-provider baseline and
does not establish any of the official benchmark's effectiveness thresholds.
