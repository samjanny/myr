# DELTA reconstruction

`myr_runner::delta::reconstruct` creates an in-memory candidate tree from an
immutable baseline and a set of DELTAs. It validates the object types and scope,
checks exact write-path prefixes and protected paths, rejects competing edits to
one path, and verifies base, patch, and result through CAS reads. The declared
result must exactly match the reconstructed bytes. Failure leaves the baseline
unchanged; no filesystem materialization or command execution occurs.

Replacement uses the patch artifact verbatim. Unified diff accepts a single
file, `---`/`+++` headers naming the DELTA path (optionally `a/` and `b/`), optional
tab-separated header timestamps, and standard `@@` line ranges. Hunk context,
removed bytes, line counts, and old/new offsets must match exactly. There is no
fuzzy matching. Body bytes remain opaque; CRLF, non-UTF-8 bytes, and the standard
missing-final-newline marker are covered by tests. Git metadata preambles,
renames, multi-file patches, and binary patch formats are rejected.

Paths must already exist in the baseline; full file deletion and new-file
snapshot semantics still require a specification decision. Empty content is an
existing empty file, not an implicit deletion.

`reconstruct_live` is the graph-backed entry point: it loads the baseline from
the scope's snapshot manifest and checks liveness of the goal, snapshot, every
file, DELTA, patch/result, and assumption. It rejects noncanonical or duplicate-key
snapshot manifests. Tests verify that invalidating an assumption prevents reuse
of its DELTA while keeping historical reads available. The caller must obtain
scope and policy from the sealed goal. The returned tree is a read-time view;
the final mission verdict must recheck dependencies after verifier execution.
The operating-system sandbox remains pending.

`candidate::prepare` reopens the compiler seal, reconstructs live dependencies,
and materializes the result in a fresh owned temporary directory. It takes
protected paths from the seal and write permissions from the trusted caller.
Every path is validated before creating files, including component case aliases
that could behave differently on Windows and Linux. Files use exclusive creation,
preserve opaque bytes, and are read back before returning. Input hashes remain
outside the candidate directory. Cleanup is automatic on drop, or explicit with
`close` to report errors. No existing checkout is accepted as a destination.

This step is not an OS sandbox: it executes no commands, applies no process,
network, or memory confinement, and produces no deterministic EVIDENCE. It assumes
the runtime's temporary directory is not concurrently modified by hostile local
processes. Executable permissions and other source metadata are not represented
by the current byte-only snapshot format. Input hashes describe creation-time
bytes, not the state after a verifier runs.

`candidate::prepare_recorded` additionally stores a canonical `myr-candidate-v0`
manifest committing the sealed scope, sorted DELTAs, normalized write prefixes,
and path-to-artifact map. Temporary filesystem paths are excluded from identity.
The graph registers this artifact and its dependencies in one transaction;
invalidating a DELTA assumption therefore stales the candidate record. A failed
transaction can leave immutable CAS bytes but no partially indexed manifest.

`load_manifest` checks canonical encoding, reopens the goal seal, reconstructs
from live dependencies, and compares the exact file map. It also requires graph
dependency provenance. This records preparation only: it does not attest that
a verifier ran, and the eventual sandbox executor must bind its evidence to this
specific candidate record. Files may change after preparation; the manifest does
not establish execution-time filesystem integrity.
