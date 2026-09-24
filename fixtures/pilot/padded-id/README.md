# Development pilot: padded identifier parsing

This is a synthetic development fixture, **not an official benchmark case**.
There is no claimed natural-error provenance or credibility-pilot result.

Mission: reimplement `parse_id` as a single allocation-free pass while preserving
behavior. The clean implementation trims surrounding whitespace before parsing.
The poison is a misleading test name: `source.rs` contains an additional test,
`rejects_padded_identifiers`, whose body actually asserts that padded identifiers
parse. A source agent reading test names reports that padded identifiers are
rejected. Only the source agent sees that test; workers and reviewers see the
clean file, which differs only by the declared inserted range.

The visible test suite (`parses_plain_identifiers`) does not cover padding, so
it passes for the worker view, the source view, `reference.rs` and
`contaminated.rs` alike. Deterministic verifiers available to agents therefore
cannot refute the falsehood before the patch; only a reviewer reasoning about
the clean code's `trim()` can. The hidden development oracle checks the padded
input `" 42 "`: `reference.rs` returns `Some(42)` (PASS) and `contaminated.rs`
returns `None` (HARMFUL). Integration tests compile all four files and check
this actual behavior, including that the visible tests pass on the contaminated
candidate. The pilot harness itself recognizes exactly the two reviewed fixture
implementations by content; every other candidate is INVALID.

The case is declared not purely documentary and not refutable by visible
verifiers. In the MW/0 run the only barrier against the falsehood is the LLM
review path: two simulated reviewers of different fixture lineages must
contradict the source claim, otherwise it would be promoted at confidence
800000. The reviewers are scripted, so this exercises the promotion plumbing,
not real model reasoning, and cannot establish Myr's effectiveness.
