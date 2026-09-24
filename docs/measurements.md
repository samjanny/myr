# Numeric measurement contract

The `measurement` module defines a typed scalar measurement procedure and exact
comparison with a sealed tolerance. Empirical binding criteria now use the
command verifier's Docker execution path. The runtime records candidate, command,
procedure, exact output and private execution receipt, then evaluates the sealed
tolerance and polarity. Final acceptance rechecks that evaluation against the
private receipt. CLI preparation supports operator-defined procedures; live
sandbox acceptance remains pending.

Add an optional `measurements` array to the `myr run` configuration:

```json
{
  "measurements": [{
    "command_index": 0,
    "target": {"mantissa": 16, "exponent": 0},
    "relation": "at_most",
    "unit": "milliseconds"
  }]
}
```

This fragment extends a complete runtime configuration. `command_index` selects
a zero-based entry in its `commands` array. That command must emit the observation
format below with the configured unit. Preparation normalizes targets, rejects
duplicates and invalid indices, registers procedures with command dependencies,
and adds their CIDs to the planner catalog. `prepared.json` also lists them in
`measurement_procedures`, in configuration order. `--prepare-only` performs these
steps without launching a provider or measurement command.

Supply a matching EMPIRICAL predicate in `predicates` with explicit semantics and
provenance. The planner must create a criterion with that predicate's ATOM, a
prepared procedure reference and nonnegative tolerance. Procedure availability
alone does not make it a binding criterion. The Goal Compiler validates the
procedure and its sealed command, and normalizes the tolerance before sealing;
the same checks run whenever the goal is reopened.

A procedure is a canonical JSON artifact with these fields:

```json
{
  "format": "myr-measurement-procedure-v0",
  "command": {"kind": "ARTIFACT", "cid": "b3:<command-policy-hash>"},
  "target": {"mantissa": 16, "exponent": -3},
  "relation": "at_most",
  "unit": "seconds"
}
```

The command must already be a command policy in the goal's sealed verifier list,
with valid limits and sandbox configuration. Loading the procedure cannot add a
command or select another environment or image. The example CID is a placeholder.
Procedure JSON uses the runtime's canonical encoding and normalized target.

The measurement program's stdout is one JSON object:

```json
{
  "format": "myr-measurement-v0",
  "value": {"mantissa": 159, "exponent": -4},
  "unit": "seconds"
}
```

Unknown fields, duplicate fields, trailing non-JSON output, wrong units, wrong
formats, invalid numbers and stdout over 64 KiB are rejected. The observation's
decimal is normalized. Nonnegative tolerance is supplied by the sealed Goal IR,
not by the measurement program. Relations are:

- `at_most`: value <= target + tolerance.
- `at_least`: value >= target - tolerance.
- `within`: absolute distance from target <= tolerance.

Equality at the tolerance boundary is accepted. Predicate polarity is separate
and will be applied by the runtime producing EVIDENCE. The comparator uses exact
signed base-10 arithmetic, without floating-point conversion or allocations
proportional to exponent magnitude. Extreme exponents and i64::MIN coefficients
are covered by tests, along with cancellation and exhaustive small integer sums.

Malformed output, a wrong unit, or nonzero command exit creates no measurement
CLAIM/EVIDENCE; the pipeline records a runtime failure and remains PARTIAL. A
valid observation outside tolerance produces contradictory deterministic evidence
for a positive criterion, even if both reviewers support it. Negative criteria
reverse support/contradiction. The command record has an optional `measurement`
procedure reference and graph dependency; existing ordinary command records keep
their encoding when that field is absent.

This is a scalar contract: the configured program must compute any statistic
such as p95, using an operator-defined reproducible procedure. A parsed number
alone is not proof that the program ran, that the measurement is representative,
or that the candidate meets the mission. The runtime requires sandbox execution,
candidate-bound evidence and the rest of the pipeline before completion.
Tests use explicitly synthetic execution receipts and reviewer responses; no
live measurement or Docker confinement result has been established.
