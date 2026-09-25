# Reference token accounting

`myr_runner::accounting` implements the Appendix C segment categories and
per-injection ledger. `ReferenceTokenizer` uses exactly `tiktoken-rs` 0.12.0
with its embedded `cl100k_base` asset; runtime tokenization needs no network.
The dependency is MIT licensed. Its source and API are documented in the
[upstream repository](https://github.com/zurawiki/tiktoken-rs) and
[CoreBPE documentation](https://docs.rs/tiktoken-rs/0.12.0/tiktoken_rs/struct.CoreBPE.html).

The downloaded crate's vocabulary SHA-256 was checked locally:
`223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7`.
`implementation_manifest()` reports the dependency version, vocabulary hash,
special-token policy, renderer/accounting source CIDs and Cargo.lock CID for
inclusion in the frozen benchmark manifest. This metadata is not itself a
preregistered benchmark freeze.

Each injected segment records its content CID, UTF-8 bytes, reference-token
count and communication classification. Segments are tokenized independently;
repeated history and schema injections are counted again on every call. Special
token spellings in untrusted text are encoded as ordinary literal text, equivalent
to Python `encode(text, disallowed_special=())`, not interpreted as control tokens.
This explicit rule must remain fixed across both pipelines.

The prose ledger accepts INTER_AGENT_PROSE and INTER_AGENT_ARTIFACT as
communication. MW/0 accepts INTER_AGENT_ARTIFACT, MW_RENDER, CAS_REFERENCED,
VALIDATION_FEEDBACK and PROTOCOL_SCHEMA. Artifact content is classified by
provenance (specification revision 1). The mission dispatcher records the
first agent to author each artifact. `Dispatcher::classify_artifact` returns:

- DIRECT_REPO for initial repository bytes, however they are fetched;
- LOCAL_TOOL for the agent's own artifacts;
- INTER_AGENT_ARTIFACT for another agent's artifacts;
- CAS_REFERENCED for other referenced runtime or protocol records.

The authorship map is journaled. Both exclude
SYSTEM, GOAL, DIRECT_REPO and LOCAL_TOOL from communication while retaining their
full-context counts for budgets. Mixing pipeline-only categories is rejected
without appending a partial call. CAS reads increment only raw bytes; inserted
CAS representations count their actual UTF-8 or Base64 text bytes and tokens.
Provider-native usage and cost stay outside this ledger.

Planner, worker and reviewer fetches record logical raw CAS bytes once per
successful retrieval, independently of later model dispatch. The adapter reports
the original byte length before Base64 or object rendering through runtime-only
metadata; it is not serialized into the agent receipt. Repeated fetches count
again. Rejected fetches and internal integrity/provenance reads do not count as
agent retrieval traffic. The dispatcher journals each raw-byte update, including
when the task exhausts its call budget before displaying the retrieved content.
`cas_context_bytes` covers CAS_REFERENCED segments plus explicit fetch-result
segments marked by the planner/task loops, including fetched instructions,
repository bytes and MW objects. The retained segment's `cas_derived` flag is
orthogonal to its communication class: fetched GOAL and DIRECT_REPO content
remains excluded from communication tokens. Marks neither change dispatched
bytes nor inject metadata into model context. Full rendered receipts and their
two-LF separators count as context bytes. Repeated history injections count again;
a fetch followed by budget exhaustion before dispatch contributes only raw bytes.
Internal provenance reads and initial runtime-generated catalog/TASK renderings
are not classified as explicit agent fetch traffic.

`PreparedContext` now assembles system/prompt strings from explicitly selected
segments. Each prompt segment includes its two-LF separator in the counted bytes.
It adds no implicit history or summaries. Recording retains exact segment bytes
in a harness-private audit CAS and returns references matching the ledger CIDs.
This audit CAS must be distinct from the shared agent store, so source-agent
context does not become a shared side channel. External schema fragments can be
recorded in the same call as shared SYSTEM or Myr-only PROTOCOL_SCHEMA segments.
`record_with_schema` derives that partition from the actual response schema and
the baseline's shared schema using `schema::accounting_parts`. Only byte-identical
action declarations are exempt; a matching tool name is insufficient. The JSON
envelope is exempt only when identical, and each comma belongs to its following
action. Concatenating all retained fragments reproduces the compact schema
exactly. The shared schema is `schema::prose_shared_schema()`, the union of the
[prose baseline](prose-baseline.md) declarations. Only `fetch`, `fetch_many`,
`put_artifact` and `finish` are byte-identical to MW/0 declarations and
therefore exempt.

Fetched artifacts are rendered by `myr_adapter::render_artifact`, which both
pipelines share (cost-pilot step A). Bytes that are valid UTF-8 without NUL
appear as `content_text`, and anything else as `content_base64`. This is a
rendering choice, not an accounting rule: tokens are counted on exactly the
inserted representation. A `fetch_many` action (step C) returns the same items,
with the same authorization, as individual fetches. Each item is its own context
segment with its own provenance class and CAS-derived flag, and logical CAS
bytes are recorded per item.

Tests cover known token IDs, literal special-token text, prevention of merges
across segment boundaries, repeated schemas, pipeline classification, and binary
CAS representation accounting, exact retained context reconstruction and absence
of implicit history and exact shared-versus-protocol schema partitioning.
Full tokenizer differential validation, pilot validation and freeze of the
baseline schema, live acceptance of runner/budget integration, and the official frozen manifest remain pending.
