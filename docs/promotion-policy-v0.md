## Appendix B — promotion-policy-v0

A CLAIM becomes a FACT with favorable deterministic evidence and no contrary deterministic evidence, or with two favorable LLM lineages and no counterevidence or conflict.

The policy considers only valid, non-invalidated EVIDENCE applicable to the CLAIM's exact canonical scope. v0 does not reason across broader or narrower scopes.

### B.1 Classification

Four sets are constructed for a CLAIM `C`:

| Set | Contents |
| --- | --- |
| `D+` | Favorable deterministic EVIDENCE |
| `D-` | Contrary deterministic EVIDENCE |
| `L+` | Favorable LLM EVIDENCE |
| `L-` | Contrary LLM EVIDENCE |

EVIDENCE supporting a CLAIM with the same ATOM and scope but opposite polarity counts as contrary to `C`. ATTEST belongs to none of these sets.

The runtime assigns the EVIDENCE class. Evidence is deterministic only if produced by a command executed in a sandbox with recorded argv, environment variables, and tool versions; no agent can declare evidence deterministic.

### B.2 Polarity conflict

`CONFLICT(C)` exists when the graph contains CLAIMs with the same ATOM and scope but opposite polarities; `myr-graph` records it. An unresolved conflict prevents LLM-only promotion; deterministic EVIDENCE may resolve it.

### B.3 Rules, in precedence order

1. If `|D-| ≥ 1`, the CLAIM cannot be promoted.
2. If `|D-| = 0` and `|D+| ≥ 1`, the CLAIM can be promoted; contrary LLM EVIDENCE does not block it.
3. If `|D+| = 0` and `|D-| = 0`, the CLAIM can be promoted only if `|L-| = 0`, there is no `CONFLICT(C)`, and at least two EVIDENCE objects in `L+` have different `lineage_key` values according to lineage-v0.

### B.4 Confidence

Confidence is a deterministic policy score in parts per million, not a calibrated probability. Values are not combined through multiplication, Bayes, or independence assumptions; ATTEST and additional same-lineage EVIDENCE do not change it.

| Promotion condition | Confidence (ppm) |
| --- | --- |
| Favorable deterministic + at least two favorable LLM lineages, none contrary | 970000 |
| At least one favorable deterministic | 950000 |
| Favorable deterministic + at least one contrary LLM or nondeterministic conflict | 900000 |
| Two or more favorable LLM lineages, no counterevidence | 800000 |

### B.5 Pseudocode

```text
fn evaluate(claim):
    evidence = valid_evidence_same_scope(claim)
    D_pos = deterministic_support(evidence, claim)
    D_neg = deterministic_contradiction(evidence, claim)
    L_pos = llm_support(evidence, claim)
    L_neg = llm_contradiction(evidence, claim)

    if not empty(D_neg):
        return NOT_PROMOTABLE

    if not empty(D_pos):
        if not empty(L_neg) or unresolved_conflict(claim):
            return FACT(confidence = 900000)
        if distinct_lineages(L_pos) >= 2:
            return FACT(confidence = 970000)
        return FACT(confidence = 950000)

    if unresolved_conflict(claim) or not empty(L_neg):
        return NOT_PROMOTABLE

    if distinct_lineages(L_pos) >= 2:
        return FACT(confidence = 800000)

    return NOT_PROMOTABLE
```

### B.6 Evidence after promotion

FACT and EVIDENCE are immutable; every new EVIDENCE triggers policy reevaluation.

- If the CLAIM is no longer promotable, its FACT becomes inactive in the graph and remains as a historical object; dependent FACTs and artifacts become `stale`.
- Contrary deterministic EVIDENCE deactivates any FACT for the CLAIM.
- Contrary LLM EVIDENCE deactivates a FACT promoted only through LLMs.
- Contrary LLM EVIDENCE against a FACT supported by deterministic evidence leaves it promotable, reevaluated at 900000.
- If the EVIDENCE set or confidence changes, a new FACT is emitted; the previous one remains historical and inactive.
- A mission cannot end COMPLETE using `stale` or inactive FACTs.
