# Running a mission

The development CLI now connects snapshot acquisition, catalog preparation,
planning, worker execution, command verification and two independent reviewers.
Live provider compatibility and Docker confinement acceptance remain incomplete;
the command is not a claim that the specification's release gates have passed.

Prepare locally without invoking models or command verifiers:

```text
myr run myr.yaml --config runtime.json --repository path/to/repository --root path/to/new-preview --prepare-only
```

Execute with the explicitly configured providers:

```text
myr run myr.yaml --config runtime.json --repository path/to/repository --root path/to/new-run
```

Each invocation requires a new output directory. Snapshot reading happens before
that directory is created. Keep output stores outside the input repository, or
exclude prior stores with `read_policy`, to avoid importing old run data. The
source checkout is never edited; reconstructed result files remain available as
CAS artifacts through the candidate manifest.

`--prepare-only` writes `prepared.json`, a graph and shared CAS. It does not check
provider authentication, launch Docker, or prove configured tools exist. Execution
adds private audit records and `result.json`. Completed/conditionally completed
missions return exit code zero; PARTIAL returns a nonzero exit code after printing
and saving the result. Configuration or storage failures also return nonzero.

## Runtime configuration

`runtime.json` is strict JSON with these fields:

| Field | Meaning |
| --- | --- |
| `providers` | Explicit `planner`, `worker`, `reviewer_a`, `reviewer_b` ProviderConfig objects |
| `commands` | Full command verifier policies with argv, cwd, environment, tool versions, output/time limits and Docker sandbox policy |
| `writable` | Repository-relative worker write prefixes; no implicit whole-repository permission |
| `token_budget`, `call_budget`, `time_budget_ms` | Shared limits across planning and execution |
| `max_native_output_tokens`, `max_reference_output_tokens` | Per-call generation/request allowances |
| `docker_executable` | Absolute local Docker CLI path |
| `predicates` | Optional custom registry declarations: name, version, arguments, semantics, class and provenance text |
| `measurements` | Optional scalar procedures: command_index, target decimal, relation and unit; see measurements.md |
| `read_policy` | Optional snapshot exclusions and resource limits; defaults documented in snapshot acquisition |

Each required YAML verifier must match a configured command argv with cwd `.`.
Missing checks, duplicate policies, invalid registry entries and invalid provider
independence are rejected before snapshot/store preparation. This CLI currently
requires `docker_linux` command policies with a pinned local image ID; it never
substitutes another sandbox or pulls an image automatically. See
[sandbox execution](sandbox-execution.md) for the image/runtime requirements.

Each role's provider config contains `backend`, `model` and complete `lineage`.
Backend selection explicitly distinguishes subscription and API billing:

```json
{"kind":"claude-code","executable":"C:/path/to/claude.exe"}
{"kind":"codex-cli","executable":"C:/path/to/codex.exe"}
{"kind":"anthropic-api","api_key_env":"ANTHROPIC_API_KEY"}
{"kind":"openai-api","api_key_env":"OPENAI_API_KEY"}
```

These are alternative backend objects, not a complete runtime configuration.
Use installed executables and explicitly chosen model/checkpoint/family identifiers.
Both reviewers must differ in producer and family; selecting two transports to the
same model does not establish independence. API keys stay in named environment
variables, never JSON configuration. `run` consumes the selected providers' quota
or API billing; `--prepare-only` does neither. There is no automatic fallback.

Preparation registers core predicates, optional operator-supplied `mission.*`
predicates with provenance artifacts, command ATOMs, snapshot files and fixed
English worker/reviewer instructions. Custom runtime configuration itself is
retained as an immutable artifact. The planner gets an explicit catalog and can
construct further typed ATOMs. The matched prose baseline is not implemented;
schema accounting currently compares against an empty shared declaration set.

Tests exercise preparation, opaque file preservation, failure before directory
creation for missing verifiers, refusal to overwrite stores and an offline failure
run with nonexistent absolute provider paths. They do not consume model quota.
Planning failures include artifacts, evidence, active assumptions and provenance,
plus a reference to an immutable private audit copy of the report. Provenance
includes the mission/policy input, baseline files, diagnostic and any objects
created by the planner. Empty evidence means no evidence was produced, not that
the requested obligations were verified. Post-seal reports expose the same four
result fields; these are historical results, not a fresh liveness assessment.
Live empirical measurement acceptance, evidence-backed UNSAT/INVALID_GOAL reporting,
live acceptance, crash recovery and benchmark deliverables remain open.

## Exporting a candidate

Use the candidate CID in the execution report to recover its files without
modifying the source checkout:

```text
myr export-candidate b3:<candidate-manifest-hash> ../myr-candidate --root ../myr-run
```

The destination parent must exist and the destination directory must be new and
outside the mission store. The command reopens the goal seal, reconstructs the
manifest, verifies live dependencies and copies exact CAS bytes with exclusive
file creation and readback. It does not run commands or providers. Existing
directories, including empty ones, are rejected; no merge or overwrite occurs.
Permissions, executable bits and other filesystem metadata are not reconstructed
by the current byte-tree format. An I/O failure may leave a partial destination;
the command returns an error and does not report successful export.

Export is available for live candidates from PARTIAL missions as well as completed
ones. Its JSON receipt says `mission_completion_checked: false`: exporting a
candidate does not establish that the candidate satisfies the mission. Use the
execution report and its evidence to inspect that outcome. Concurrent hostile
modification of the destination directory is outside this local export operation's
confinement guarantees.
