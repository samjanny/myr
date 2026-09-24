# Mission budget gate

`myr_runner::budget::Budget` owns one mission-wide monotonic deadline, call
limit, and reference-token limit. Every role, retry, and repair must share it.
`reserve` charges the full input and one call before dispatch, reserves output
capacity, and returns a single-use permit with the remaining timeout. Permits
belong to one budget instance and cannot be cloned or deserialized.

`settle` records output even for malformed, refused, or late responses. Unknown
complete output leaves the reservation visible and stops further dispatch with
`TOKEN_BUDGET`; it does not fabricate zero usage. Exceeding an output allowance
records the observed overrun and stops the mission. Exhaustion must ultimately
yield PARTIAL, never UNSAT.

This is a runner component, not yet an integrated mission executor. The caller
must supply exact reference-token counts from the frozen tokenizer, counting
all model context for the operational budget. Communication-token segments for
Appendix C are a separate subset. Provider-native token counts remain separate
telemetry; a native output-token setting is not a guaranteed reference-token
limit. Live CLI generation can overrun a requested cap, so settlement detects
overruns rather than claiming an absolute generation bound. Lost output or a
dropped permit prevents further calls. The reference tokenizer and segment ledger now exist in `accounting`;
persistence/restart recovery, tokenizer freeze, transport wiring, and terminal-state
handling remain pending.
