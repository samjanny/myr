## Appendix C — token-accounting-v0

Inter-agent communication is counted with one reference tokenizer, `cl100k_base`, applied identically to both pipelines and all providers. *Written by ChatGPT.*

The library version, tokenizer assets, and their hashes are recorded in the frozen manifest. Native provider counts are cost telemetry only.

### C.1 Segment classification

The runtime classifies every segment inserted into an LLM's context before calling the provider.

| Segment | Meaning | Counts in baseline | Counts in Myr |
| --- | --- | --- | --- |
| `SYSTEM` | Shared system prompt | No | No |
| `GOAL` | Initial mission | No | No |
| `DIRECT_REPO` | Files read directly by the agent itself | No | No |
| `LOCAL_TOOL` | The agent's own tool output | No | No |
| `INTER_AGENT_PROSE` | Other agents' messages, including file excerpts, tool output, instructions, or conclusions copied into them | Yes | — |
| `MW_RENDER` | TASK, CLAIM, FACT, EVIDENCE, ASSUMPTION representation inserted into context | — | Yes |
| `CAS_REFERENCED` | Artifact content retrieved because an inter-agent object references it and inserted into context | — | Yes |
| `VALIDATION_FEEDBACK` | Validator messages correcting invalid output | — | Yes |
| `PROTOCOL_SCHEMA` | Schemas and tool definitions required only by MW/0 | — | Yes |

Each baseline agent receives only inter-agent messages addressed to it, without automatic summaries.

### C.2 CAS retrieval

Each retrieval has two counters: `cas_raw_bytes` (read from CAS) and `cas_context_bytes` (inserted into context). An artifact retrieved but not shown to the model produces no communication tokens. If a binary artifact is rendered as text or Base64, tokens are counted on the inserted representation.

### C.3 Bytes and tokens

Each counted segment records `utf8_bytes` and `reference_tokens = len(cl100k_base.encode(segment))`. Each segment is tokenized separately, without BPE merges across adjacent segments.

```text
baseline_comm_tokens = Σ tokens(INTER_AGENT_PROSE)

myr_comm_tokens      = Σ tokens(MW_RENDER)
                     + Σ tokens(CAS_REFERENCED)
                     + Σ tokens(VALIDATION_FEEDBACK)
                     + Σ tokens(PROTOCOL_SCHEMA)
```

The primary metric is `communication_reference_tokens`; `communication_context_bytes` is always also reported as a tokenizer-independent check. Reduction is calculated on per-mission medians. Counting schemas and validation feedback prevents hiding the cost of the structured protocol.

### C.4 Tool schemas and rendering

Schemas are counted symmetrically, with a rule fixed now rather than after seeing pilot data.

| Schema | Baseline | Myr |
| --- | --- | --- |
| Tools shared by both pipelines (read, edit, shell, …) | Does not count | Does not count |
| Myr-only tools (`emit_claim`, `emit_assumption`, `emit_delta`, `emit_fail`, `review_claim`) | — | Counts on every call where it enters context |

Counting every call reflects the protocol's real cost. The pilot only checks whether the –25% threshold is achievable under this rule: if not, the threshold is revised before freezing, never the counting rule.

`MW_RENDER` is produced by a single rendering function, frozen and hashed in the manifest: compact JSON with short but readable field names, without tables or prose reformulation. Integer keys would save tokens but risk impairing model comprehension; the pilot checks rendering's effect on both tokens and CTSR.

### C.5 Separate telemetry

Recorded but excluded from the primary metric: `provider_input_tokens`, `provider_output_tokens`, `provider_cached_tokens`, `provider_cost`, `cas_raw_bytes`, `mw_wire_bytes` (CBOR bytes exchanged between runtime components), and `prose_wire_bytes` (UTF-8 bytes in baseline messages).
