## Appendix A — lineage-v0

Two LLM EVIDENCE objects have different lineages only if both their producer and model family differ.

### A.1 Fields

Every LLM verifier has four runtime-configuration fields mandatory for `LLM_REVIEW` EVIDENCE:

| Field | Identifies |
| --- | --- |
| `provider_id` | The model producer, not the API gateway or inference reseller |
| `family_id` | A family of models with shared training lineage |
| `checkpoint_id` | The concrete version used |
| `procedure_id` | The verification procedure: prompt, input schema, tools, review strategy |

### A.2 Rules

1. Primary lineage is `lineage_key = (provider_id, family_id)`.
2. `checkpoint_id` distinguishes instances of the same lineage but does not create independence.
3. `procedure_id` provides procedural diversity but alone does not create independent lineage.
4. Two LLM EVIDENCE objects have different lineages only if both `provider_id` and `family_id` differ.
5. If a field is missing, ambiguous, or `unknown`, the EVIDENCE is retained but does not contribute to the two-lineage requirement.
6. Fields are assigned by the runtime from frozen configuration; the LLM cannot declare or change its own lineage.
7. Multiple EVIDENCE objects with the same `lineage_key` count as one lineage.

### A.3 Cases

| Verifier pair | Different lineage |
| --- | --- |
| Different provider, different family | Yes |
| Same provider, different family | No |
| Different provider, same family | No |
| Same provider and family, different checkpoint | No |
| Same model, different procedure | No |

### A.4 Pseudocode

```text
fn different_lineage(a, b):
    require a.provider_id != UNKNOWN and a.family_id != UNKNOWN
    require b.provider_id != UNKNOWN and b.family_id != UNKNOWN
    return a.provider_id != b.provider_id
       and a.family_id   != b.family_id
```

Benchmark lineage configuration is included in the frozen official-run manifest, with each reviewer's `(provider_id, family_id)` pair; an automated test checks `different_lineage` for the two.
