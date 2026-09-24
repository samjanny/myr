# Budgeted provider dispatch

`dispatch::Dispatcher` connects prepared context, schema partitioning, reference
token accounting, a mission-wide budget, private audit storage, and the explicit
four-backend transport entry point. A call specifies native and reference output
limits separately. Native provider token limits are not assumed equivalent to
the reference tokenizer.

Before transport invocation, the dispatcher validates configuration and context,
counts the exact segments, and reserves a budget permit. Budget-rejected calls do not
append an attempt or accounting entry. Prepared artifacts may remain in the
private audit store. Allowed attempts retain input segment references, raw output,
configured provider, observed model, native usage and budget outcome. Responses
remain untrusted and must pass `Session::handle` before any graph mutation.

Transport failures can leave delivery and provider usage uncertain. Attempt
records explicitly mark `transport_completed=false`; their accounting entries
describe attempted context, not confirmed provider ingestion. Such attempts must
remain distinguishable in official reporting. Unknown output usage stops further
dispatch and is never treated as zero. Over-budget completed responses are retained
but not returned for semantic application. There are no retries or billing
fallbacks. Timeout and budget exhaustion remain reasons for PARTIAL, not UNSAT.

Every dispatcher creates a private `dispatch-journal-*` directory in its audit
store. Before transport invocation, it persists a content-addressed snapshot with
the pending attempt, budget reservation, exact input references and generation
limits. After settlement it persists another snapshot containing the outcome and
accounting. Each snapshot links its predecessor. Numbered JSON pointer files are
flushed and installed atomically without overwriting existing records; the audit
CAS verifies the referenced bytes. `journal_directory()` and `last_checkpoint()`
expose those locations for reporting. A failed journal write prevents transport
invocation and disables future dispatch on that instance.

An interrupted process can therefore leave a pending record distinguishable from
a settled call. These logs are evidence, not permission to retry: restoring a
mission, reconciling uncertain provider usage and power-loss recovery still need
implementation and validation. Creating a new dispatcher starts a new journal;
it does not resume an existing mission automatically.

Tests inject a deterministic in-process transport callback to verify accounting,
request contents, timeout clamping, output retention, budget rejection, and
failure behavior, pre-send checkpoint visibility, linked settlement records, and
journal-write failure. They do not call live models. Full role orchestration,
recovery, provider-wrapper accounting, sandboxed verification and
terminal mission reporting remain pending.
