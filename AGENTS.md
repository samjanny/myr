# Project conventions

Use English for source code, comments, documentation, schemas, diagnostics, and
commit messages. Keep `myr_v0_it.md` as the original Italian specification.
Reply to the project owner in Italian.

`myr_v0.md` is the English specification. Do not silently weaken its invariants
or substitute fixture results for live-provider or official benchmark evidence.
Keep implementation progress and remaining acceptance gates in `STATUS.md`.

The runtime must support explicit selection of subscription-backed Claude Code
and Codex CLI sessions, or separately billed provider APIs. Never silently fall
back from a subscription backend to a paid API backend.
