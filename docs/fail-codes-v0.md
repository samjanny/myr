# FAIL code registry v0

Numeric assignments are part of MW/0. Display names are closed enums. The
specification's `capability_denied` spelling denotes `CAPABILITY_DENIED` below.
Unknown codes and mismatched classes fail validation. Diagnostic text is an
opaque artifact reference, never a free-form code.

| Code | Name | Class |
| --- | --- | --- |
| 0 | PROVIDER_UNAVAILABLE | INFRASTRUCTURE (0) |
| 1 | SANDBOX_UNAVAILABLE | INFRASTRUCTURE (0) |
| 2 | IO_FAILURE | INFRASTRUCTURE (0) |
| 3 | INVALID_AGENT_OUTPUT | VALIDATION (1) |
| 4 | INVALID_GOAL | GOAL (2) |
| 5 | CONFLICTING_OBLIGATIONS | GOAL (2) |
| 6 | CAPABILITY_DENIED | CAPABILITY (3) |
| 7 | TOKEN_BUDGET | BUDGET (4) |
| 8 | CALL_BUDGET | BUDGET (4) |
| 9 | TIME_BUDGET | BUDGET (4) |
| 10 | MISSING_REFERENCE | DEPENDENCY (5) |
| 11 | INVALIDATED_REFERENCE | DEPENDENCY (5) |
| 12 | RUNTIME_ERROR | INTERNAL (6) |
