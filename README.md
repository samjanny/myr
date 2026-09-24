# Myr

[![CI](https://github.com/samjanny/myr-lang/actions/workflows/ci.yml/badge.svg)](https://github.com/samjanny/myr-lang/actions/workflows/ci.yml)

Myr is a Rust runtime for typed, content-addressed communication between AI
agents. It separates claims from verified facts, records explicit assumptions,
and tracks how invalidated assumptions affect evidence and candidate changes.

**Development status:** the runtime and CLI are implemented and under active
development. Tests cover the protocol, storage, graph, provider adapters,
execution pipeline and benchmark tooling. Live sandbox/provider conformance and
the official benchmark remain incomplete. See [STATUS.md](STATUS.md) for evidence
and outstanding requirements. Myr is not a new programming language: v0 uses
YAML missions, typed agent actions and canonical CBOR messages.

## Provider and billing choice

Choose a backend explicitly for each fixed role: planner, worker and two reviewers.

| Backend | Authentication and usage |
| --- | --- |
| `claude-code` | Installed Claude Code CLI with a supported subscription session |
| `codex-cli` | Installed Codex CLI with a supported subscription session |
| `anthropic-api` | Anthropic API key; separately billed API usage |
| `openai-api` | OpenAI API key; separately billed API usage |

There is no automatic fallback from subscription usage to a paid API. Both
reviewers must use different model producers and training families. Provider
support does not imply identical model capabilities or verified compatibility
with every CLI/model release. A minimal Claude Code Max call has been verified;
remaining live checks are documented in [transports](docs/transports.md).

## What is implemented

- Typed MW/0 messages, canonical encoding and BLAKE3 content identifiers.
- Immutable filesystem storage and a SQLite dependency/evidence graph.
- Claims, attestations, runtime evidence, promoted facts and invalidation.
- YAML mission validation, a sealed Goal IR and protected paths.
- A planner/worker/two-reviewer pipeline with shared call, token and time budgets.
- Candidate reconstruction from full replacements or exact unified diffs.
- Explicit Docker Linux verifier execution and scalar empirical measurements.
- Candidate export, context accounting and private execution audit records.
- Development fixtures, randomized benchmark plans, result collection and
  case-cluster bootstrap statistics. These do not establish benchmark success.

## Build and test

Requires Rust 1.88 or newer and the native build tools for your platform.

```sh
cargo build --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
```

Add `--offline` to Cargo build/test commands when dependencies are already cached.
CI runs stable Rust tests on Linux and Windows, a Linux Rust 1.88 compatibility
job, and formatting/Clippy checks. Automated tests use local fixtures and mocks;
they do not run paid model calls or live Docker acceptance tests.

## CLI

```sh
cargo run -p myr-cli -- --help
cargo run -p myr-cli -- check-mission examples/cache-migration.myr.yaml
```

Prepare a mission using your explicit runtime configuration:

```sh
cargo run -p myr-cli -- run myr.yaml --config runtime.json --repository ./project --root ./mission-preview --prepare-only
```

`--prepare-only` snapshots the repository and builds the catalog without provider
or verifier calls. To execute, omit it and choose a **new** output directory.
Execution consumes the selected subscription quota or API billing. See
[runtime configuration](docs/run.md) and [measurement configuration](docs/measurements.md).
The Docker verifier requires an available local Linux daemon and a pinned local
image; Myr does not automatically pull an image.

Other commands:

| Command | Purpose |
| --- | --- |
| `snapshot` | Import repository bytes into a new store |
| `seal`, `inspect-goal` | Compile/revalidate Goal IR against stored references |
| `show`, `invalidate` | Inspect objects or invalidate an assumption |
| `export-candidate` | Recover a live candidate into a new directory |
| `pilot` | Run the local development fixture with simulated agents |
| `plan-benchmark` | Generate a reproducible randomized job plan |
| `collect-benchmark` | Join receipts and report missing/invalid pairs |
| `analyze-benchmark` | Analyze complete paired measurements |

Use `myr <command> --help` for arguments. Exporting a candidate does not certify
mission success; planning or analyzing a benchmark does not certify an official
experiment. No completed live mission or official effectiveness result is claimed.

## Workspace and documentation

| Crate | Responsibility |
| --- | --- |
| `myr-core` | Semantic types, validation and promotion rules |
| `myr-wire` | Canonical encoding, identifiers and rendering |
| `myr-cas` | Verified immutable object storage |
| `myr-graph` | Dependency lifecycle and fact management |
| `myr-adapter` | Typed agent boundary and provider transports |
| `myr-runner` | Planning, execution, verification and experiment tooling |
| `myr-cli` | Command-line interface |

- [English specification](myr_v0.md)
- [Implementation status](STATUS.md)
- [Architecture](docs/architecture.md)
- [MW/0 schema](mw0.cddl) and [reference vectors](test-vectors/README.md)
- [Sandbox execution](docs/sandbox-execution.md)
- [Benchmark schedule](docs/benchmark-schedule.md), [collection](docs/benchmark-collection.md)
  and [statistics](docs/benchmark-statistics.md)

Published source and documentation are in English. Local run stores, private
audit data, credentials, build outputs and the archival Italian specification
are excluded from this repository.
