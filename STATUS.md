# Implementation status

Updated: 2026-09-24. Goal remains active and incomplete.

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
- The full offline workspace suite passed **140 Windows tests**. These include context/schema
  accounting, journaled dispatch, task execution, and sealed role/model/billing enforcement.
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
  PARTIAL. Recovery of intermediate outputs not returned by an interrupted task
  loop remains open.
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
- Checked on Windows with Rust/Cargo 1.95.0. No Linux or minimum-Rust-version run.
- One explicitly authorized Claude Code Max smoke call passed: structured `finish`,
  model `claude-haiku-4-5`, reported input/output tokens 2992/226. An earlier
  sandboxed attempt timed out with unknown usage. No live Codex/API calls,
  full missions, or official benchmark runs. See `docs/transports.md`.
- The public `myr pilot .myr/pilot-v0` command completed successfully and retained
  `.myr/pilot-v0/report.json` plus separate stores/artifacts. Both view and oracle
  fixture gates passed. The fixture report explicitly marks simulated agents and
  development-only data. No active build/test process remains.
- Public repository: `samjanny/myr-lang`. The publication includes English source,
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
| D9 assumptions and invalidation | Append-only invalidation objects, transitive stale propagation, historical reads tested; task-bound lazy materialization tested, including atomic insertion and refusal to revive invalidated assumptions |
| D10 seeded falsehood benchmark | Not implemented; no official case-quality or outcome claims |
| `mw0.cddl`, normative appendices, FAIL registry | Draft schema, extracted appendices, closed code registry exist; independent CDDL conformance check and normative freeze pending |
| `lineage-v0` | Pairwise producer AND family independence tested, unknown fields excluded; per-role provider/model/billing configuration sealed and enforced; official benchmark model freeze pending |
| `promotion-policy-v0` | Pure policy and persistent lifecycle tests pass; runtime command/reviewer provenance and candidate acceptance checks implemented; live sandbox/provider conformance and complete orchestration pending |
| CAS put/get/exists, verified reads | Implemented and tested; bounded repository snapshot reader and new-store CLI import preserve opaque bytes and retain read policy; no source-view blobs loaded |
| Graph supports/contradicts/attests/depends_on/assumes/conflicts | Implemented; production trust boundary remains in adapter/runner |
| Fixture pilot before LLM adapter | Implemented and tested: twin-view byte-range/hash checks, shared-CAS isolation, simulated source/worker/two reviewers, graph outputs, reference/contaminated oracle fixtures; development-only |
| Two providers, identical assertion, identical CLAIM CID | Adapter test confirms same CLAIM and different ATTEST for two configured identities; live-provider acceptance still required |
| Three-level adapter validation and max two repairs | Implemented; schema/semantic failures allow two repairs, third emits runtime FAIL, policy violation fails immediately |
| Codex and Claude Code subscription backends | Implemented with auth checks and isolated bounded subprocesses; Claude Max finish smoke passed; Codex live and sandbox conformance pending |
| OpenAI and Anthropic API backends | Implemented; local HTTP mock tests pass; live API compatibility pending |
| Explicit plan/API selection, no billing fallback | Closed backend enum, producer validation, reviewer-independence and no-auto/no-fallback tests implemented; transport dispatch and auth diagnostics implemented; live conformance remains partial |
| Goal YAML, IR, G0 seal, protected paths | Compiler, immutable seal, YAML binding, catalog preparation and budgeted planner/execution integration implemented; live acceptance pending |
| Baseline + DELTA reconstruction, deterministic sandbox | Exact reconstruction, candidate provenance, Docker execution and runtime command/measurement EVIDENCE implemented; live confinement and native backends pending |
| Both DELTA codecs | Runner applies full replacement and exact single-file unified diff; opaque bytes and newline behavior tested. Adapter exposes both codecs with task-local patch/result references; live mission integration pending |
| Fixed planner/worker/two-reviewer pipeline | Prepared-catalog planner/worker/command/two-reviewer coordination, setup and CLI implemented with one budget and journal; injected tests pass; live acceptance pending |
| Five terminal states with truthful evidence | Post-seal COMPLETE/COMPLETE_WITH_ASSUMPTIONS/PARTIAL uses candidate-bound facts and completed stages; INVALID_GOAL reporting and evidence-backed UNSAT remain pending |
| `myr run`, `show`, `invalidate` | Commands implemented; run preparation and offline failure path have binary tests; successful live mission acceptance remains pending |
| Comparable prose baseline | Not implemented |
| `token-accounting-v0`, cl100k_base, frozen renderer | Compact renderer, pinned cl100k_base tokenizer, per-segment/per-call ledger, CAS byte separation and implementation fingerprints implemented; exact context assembly and private audit retention tested; exact shared/protocol schema partition tested; budgeted transport dispatch tested with injected transport; concrete baseline schema, full role integration, differential tokenizer validation and manifest freeze pending |
| 8–12 valid primary cases, category/proportion rules | Corpus and provenance/credibility pilot missing; do not pad with artificial official cases |
| source_view/worker_view, view_diff, no side channels | Development harness verifies hashes/declared edits, rejects path collisions, and excludes source bytes/hashes from shared CAS/task context; official corpus checks pending |
| Oracle PASS/HARMFUL fixtures, INVALID handling | Development oracle plus compiled behavior tests implemented; arbitrary candidate returns INVALID; official oracles still required |
| Preregistered manifest, randomized paired repetitions | Reproducible paired schedule and verification implemented; official admission/freeze, provider seed support and execution pending |
| Case-cluster bootstrap, conservative bounds, thresholds | Complete-pair statistics and CLI implemented/tested; official manifest freeze, missing-data handling and terminal assessment pending |
| Raw official data and comparative report | No runs; no PASS/FAIL/INCONCLUSIVE claim |

## Next implementation sequence

1. Complete live provider/schema and sandbox-conformance acceptance. Claude Code
   has passed only a minimal finish smoke. Preserve explicit billing choice; do
   not inspect credentials or treat fixture/mock success as live evidence.
2. Complete terminal INVALID_GOAL/UNSAT
   handling, interruption recovery and candidate lifecycle behavior.
3. Complete accounting, matched baseline, corpus checks, missing-data statistics, freezing,
   official runs, and report. Keep development evidence separate from official data.

The full objective remains active; no blocker has been declared.
The scripted pilot is not a matched live-provider baseline and
does not establish any of the official benchmark's effectiveness thresholds.
