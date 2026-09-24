# Candidate-bound LLM reviews

`review::create_context` records the candidate manifest, one of the two reviewer
slots, that slot's sealed provider/model/billing configuration, and the review
instruction artifact. The canonical context has graph dependencies on candidate
and instruction. A review TASK must include this context in its reference closure
and use the same instruction. The context contains no prior agent conversation.

The task runner accepts an optional runtime-owned `review_context`. Before any
dispatch, it reopens the context and seal, verifies slot/provider/instruction/scope,
and confirms task-local access. It binds the validated reference to the adapter
session. Agent JSON cannot choose or replace this binding.

For a bound `review_claim`, the adapter registers a canonical rationale envelope
containing context, TASK, CLAIM, verdict and the model's original rationale.
Atomic graph dependencies preserve all of those references before runtime EVIDENCE
is inserted. The EVIDENCE still has runtime-assigned lineage and mechanism; it
cannot contain an agent-declared command or deterministic classification. Since
the envelope includes the candidate context, identical review text for different
candidates cannot collapse into one evidence identity. Invalidation propagates
through candidate, context, TASK, rationale envelope and EVIDENCE.

`acceptance::assess_candidate` verifies that envelope and its real dependency
edges, checks sealed provider lineage and task binding, and rejects a different
candidate. Artifact JSON alone cannot provide the required graph provenance.
The original rationale remains reachable through permitted graph references.
Legacy sessions without a context still work, but their unbound review evidence
cannot satisfy final-candidate acceptance.

The integration test uses injected provider replies through the actual task loop,
adapter and graph. It verifies two independent reviewer slots, promotion only
after the second supporting review, refusal of mismatched contexts before dispatch,
forged-envelope rejection and refusal to reuse evidence for another candidate.
This is not live-provider evidence. Full fixed-role mission orchestration and
terminal result generation are still pending.
