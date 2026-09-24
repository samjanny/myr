# Myr v0 implementation design

The complete specification is [myr_v0.md](../myr_v0.md). The Italian original is
preserved separately. This document resolves implementation details left open
by the specification; it does not relax D1–D10 or benchmark gates.

## Components and trust

The seven target crates are `myr-core`, `myr-wire`, `myr-cas`, `myr-graph`,
`myr-adapter`, `myr-runner`, and `myr-cli`. They build upward in that order.
The first three implement semantic values, canonical identity, and opaque storage.
SQLite is an index of CAS objects; it does not replace immutable history.

Agent output must cross schema, reference/type/scope, and capability validation
before insertion. Provider JSON cannot set runtime identities, lineage,
evidence mechanism, or FACT confidence. The runtime constructs ATTEST and
EVIDENCE. Only the promotion engine constructs FACT. Raw rejected output is
quarantined outside graph indexes, including failed repair attempts.

An object reference is `[kind, 32-byte CID]` on the wire. A scope holds the sealed
goal reference and the exact snapshot artifact reference. Predicate arguments
are tagged ordered values. Record fields have ascending integer keys defined in
`mw0.cddl`. Set fields are normalized, encoded, sorted, and checked for duplicates.
The strict decoder accepts only bytes identical to canonical re-encoding.

`core.command_succeeds`, `core.artifact_equals`, and `core.code_review` form the
initial closed vocabulary. A mission predicate requires an artifact recording
its provenance and is registered by the compiler before sealing. Class and
semantics are part of its identity. Predicate names cannot silently change type.

An assumption invalidation is another ASSUMPTION object with `invalidates`
referencing the original. SQLite liveness is derived state: immutable objects
remain readable for history while dependents become stale. A changed evidence
set creates a new FACT and retires the old one, even if its confidence is equal.

## Provider and billing selection

The adapter exposes four explicit backends with the same validated response
contract:

| Backend | Execution | Credentials and billing |
| --- | --- | --- |
| `codex-cli` | Official `codex exec` with structured output | User's authenticated Codex CLI account; subscription mode must verify account authentication and exclude API-key routing |
| `claude-code` | Official `claude -p` with JSON Schema output | User's Claude Code subscription login; reject API/cloud credential routing in subscription mode |
| `openai-api` | HTTPS Responses API | Explicit API key from a named environment variable; separately billed |
| `anthropic-api` | HTTPS Messages API | Explicit API key from a named environment variable; separately billed |

Backend selection is per role, with two reviewer producers and families that
both differ. Transport choice does not determine lineage: a gateway is not the
model producer. Model/checkpoint and family are explicit configuration values,
not guessed from a model name. No automatic subscription-to-API fallback.
Unknown usage/cost values remain unknown rather than being reported as zero.

CLI invocation must use argument arrays and stdin, explicit timeouts, bounded
output, no conversation reuse, and a controlled working directory. Disable
built-in file/shell/network/MCP tools so all model actions pass through Myr's
capability boundary. Account login stays with the official CLI; Myr must not
extract OAuth tokens or repurpose them as API credentials. Claude `--bare` cannot
be used for the subscription backend because it skips subscription credentials.
CLI backend tests must verify installed flag/auth behavior before a live run.

Official references inspected on 2026-09-24:

- [Codex non-interactive execution](https://developers.openai.com/codex/noninteractive)
- [Codex authentication](https://developers.openai.com/codex/auth)
- [Claude Code programmatic execution](https://code.claude.com/docs/en/headless)
- [Claude Code authentication](https://code.claude.com/docs/en/authentication)

## Runner and benchmark

The compiler seals the goal, acceptance atoms, baseline snapshot, protected paths,
and verifier configuration before worker execution. A fixed planner/worker/two
reviewer pipeline shares only explicit references. Deterministic verifiers run
against reconstructed snapshots with recorded argv, environment, versions, and
logs. A temporary checkout alone is not an OS security sandbox; the runner must
use an actual confinement backend or fail with SANDBOX_UNAVAILABLE.

The CLI will expose run/show/invalidate and provider diagnostics. Fixture mode is
an explicit development backend. Passing fixture tests is not evidence of live
subscription/API integration or successful official benchmark execution.

The prose baseline and MW pipeline share budgets, role topology, tools, providers,
and acceptance criteria. Only the communication protocol changes. Official cases
must satisfy the source/worker view isolation and oracle gates. Do not fabricate
natural-case provenance or claim an official result from synthetic fixtures.
Report missing prerequisites explicitly; keep the full scope in STATUS.md.
