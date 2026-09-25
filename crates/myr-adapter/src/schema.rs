//! One response contract shared by every transport. Role-specific schemas do not
//! expose tools for creating EVIDENCE, FACT, ATTEST, identities, or lineage.
use crate::Role;
use serde_json::{Value, json};

pub struct SchemaPart {
    pub protocol_only: bool,
    pub text: String,
}

/// Partition the exact compact schema against the actual shared-tool baseline.
/// Only byte-identical action declarations are exempt. A tool name alone never
/// establishes that its arguments, reference kinds, or descriptions are shared.
pub fn accounting_parts(
    schema: &Value,
    shared_schema: &Value,
) -> Result<Vec<SchemaPart>, myr_core::ValidationError> {
    // Multi-action envelopes list alternatives under `actions.items`; the
    // planner's single-action envelope under `action`.
    let pointer_of = |value: &Value| {
        [
            "/properties/actions/items/anyOf",
            "/properties/action/anyOf",
        ]
        .into_iter()
        .find(|p| value.pointer(p).is_some_and(Value::is_array))
    };
    let pointer = pointer_of(schema)
        .ok_or_else(|| myr_core::invalid("missing schema action alternatives"))?;
    let shared_pointer = pointer_of(shared_schema)
        .ok_or_else(|| myr_core::invalid("missing shared schema action alternatives"))?;
    let actions = schema
        .pointer(pointer)
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| myr_core::invalid("missing schema action alternatives"))?;
    let shared = shared_schema
        .pointer(shared_pointer)
        .and_then(Value::as_array)
        .ok_or_else(|| myr_core::invalid("missing shared schema action alternatives"))?;
    let declarations: Vec<String> = actions.iter().map(Value::to_string).collect();
    let shared_declarations: std::collections::BTreeSet<String> =
        shared.iter().map(Value::to_string).collect();
    let joined = declarations.join(",");
    let serialized = schema.to_string();
    let offset = serialized
        .find(&joined)
        .ok_or_else(|| myr_core::invalid("schema serialization mismatch"))?;
    let mut envelope = schema.clone();
    let mut shared_envelope = shared_schema.clone();
    *envelope.pointer_mut(pointer).expect("checked pointer") = json!([]);
    *shared_envelope
        .pointer_mut(shared_pointer)
        .expect("checked pointer") = json!([]);
    let envelope_is_protocol = envelope != shared_envelope;
    let mut parts = vec![SchemaPart {
        protocol_only: envelope_is_protocol,
        text: serialized[..offset].into(),
    }];
    for (index, declaration) in declarations.into_iter().enumerate() {
        let protocol_only = !shared_declarations.contains(&declaration);
        // Each array separator belongs to its following declaration.
        parts.push(SchemaPart {
            protocol_only,
            text: if index == 0 {
                declaration
            } else {
                format!(",{declaration}")
            },
        });
    }
    parts.push(SchemaPart {
        protocol_only: envelope_is_protocol,
        text: serialized[offset + joined.len()..].into(),
    });
    Ok(parts)
}

fn record(properties: Value) -> Value {
    let required: Vec<_> = properties
        .as_object()
        .expect("schema properties")
        .keys()
        .cloned()
        .collect();
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn action(name: &str, properties: Value) -> Value {
    record(json!({"tool":{"type":"string","enum":[name]},"arguments":record(properties)}))
}
fn reference(kind: Option<&str>) -> Value {
    let kinds = kind.map(|s| json!([s])).unwrap_or_else(|| {
        json!([
            "ARTIFACT",
            "TASK",
            "CLAIM",
            "EVIDENCE",
            "FACT",
            "ASSUMPTION",
            "DELTA",
            "FAIL",
            "ATTEST",
            "ATOM",
            "PREDICATE_DEF",
            "GOAL"
        ])
    });
    record(
        json!({"kind":{"type":"string","enum":kinds},"cid":{"type":"string","pattern":"^(b3:[0-9a-f]{64}|@[0-7]|#[1-9][0-9]*)$"}}),
    )
}
/// Multi-action response envelope shared by both pipelines (step C2). The
/// 1..=8 bound is enforced at runtime, not in the schema.
fn envelope(actions: Vec<Value>) -> Value {
    record(json!({"actions":{"type":"array","items":{"anyOf":actions}}}))
}
fn list(items: Value) -> Value {
    json!({"type":"array","items":items})
}
fn text() -> Value {
    json!({"type":"string"})
}

/// Declarations offered identically by both pipelines. They are generated once
/// so Myr and the prose baseline cannot drift apart byte by byte (Appendix C.4).
fn shared_actions() -> Vec<Value> {
    vec![
        action("fetch", json!({"reference":reference(None)})),
        action("fetch_many", json!({"references":list(reference(None))})),
        action("put_artifact", json!({"content_base64":text()})),
        action("finish", json!({})),
    ]
}

/// Fixed-role slots addressed by prose-baseline messages, in pipeline order.
pub const PROSE_RECIPIENTS: [&str; 3] = ["worker", "reviewer_a", "reviewer_b"];

/// Frozen response schema of the natural-language baseline. It keeps the shared
/// declarations and adds only baseline tools: a whole-file edit, prose messages
/// to later pipeline stages, and a reviewer's final verdict. `recipients` must
/// be a subset of [`PROSE_RECIPIENTS`] chosen by the runtime for the role.
pub fn prose_response_schema(role: Role, recipients: &[&str]) -> Value {
    let mut actions = shared_actions();
    if !recipients.is_empty() {
        actions.push(action(
            "send_message",
            json!({"to":list(json!({"type":"string","enum":recipients})),"text":text()}),
        ));
    }
    match role {
        Role::Worker => actions.push(action(
            "write_file",
            json!({"path":text(),"content_ref":reference(Some("ARTIFACT"))}),
        )),
        Role::Reviewer => actions.push(action(
            "submit_review",
            json!({"verdict":{"type":"string","enum":["APPROVE","REJECT"]},"message":text()}),
        )),
        Role::Planner => {}
    }
    envelope(actions)
}

/// Union of every prose-baseline declaration. Myr passes it as the shared
/// schema: only byte-identical declarations are exempt from PROTOCOL_SCHEMA.
pub fn prose_shared_schema() -> Value {
    let mut actions = Vec::new();
    for (role, recipients) in [
        (Role::Planner, &PROSE_RECIPIENTS[..]),
        (Role::Worker, &PROSE_RECIPIENTS[1..]),
        (Role::Reviewer, &[][..]),
    ] {
        for declaration in prose_response_schema(role, recipients)["properties"]["actions"]["items"]
            ["anyOf"]
            .as_array()
            .expect("prose schema actions")
        {
            if !actions.contains(declaration) {
                actions.push(declaration.clone());
            }
        }
    }
    envelope(actions)
}

pub fn response_schema(role: Role) -> Value {
    let mut actions = shared_actions();
    if role == Role::Reviewer {
        actions.push(action("review_claim", json!({"claim_ref":reference(Some("CLAIM")),"verdict":{"type":"string","enum":["SUPPORTS","CONTRADICTS","INCONCLUSIVE"]},"rationale_ref":reference(Some("ARTIFACT"))})));
    } else {
        let values = [
            ("ref", reference(None)),
            ("text", text()),
            ("integer", json!({"type":"integer"})),
            ("boolean", json!({"type":"boolean"})),
            (
                "decimal",
                record(json!({"mantissa":{"type":"integer"},"exponent":{"type":"integer"}})),
            ),
        ]
        .into_iter()
        .map(|(tag, value)| record(json!({"type":{"type":"string","enum":[tag]},"value":value})))
        .collect::<Vec<_>>();
        actions.push(action("emit_claim", json!({"predicate_ref":reference(Some("PREDICATE_DEF")),"arguments":list(json!({"anyOf":values})),"polarity":{"type":"boolean"}})));
        actions.push(action("emit_assumption", json!({"question":text(),"chosen":text(),"alternatives":list(text()),"rationale_ref":reference(Some("ARTIFACT")),"artifacts":list(reference(Some("ARTIFACT")))})));
        actions.push(action("emit_delta", json!({"path":text(),"base_ref":reference(Some("ARTIFACT")),"patch_ref":reference(Some("ARTIFACT")),"codec":{"type":"string","enum":["REPLACEMENT","UNIFIED_DIFF"]},"result_ref":reference(Some("ARTIFACT")),"assumptions":list(reference(Some("ASSUMPTION")))})));
        actions.push(action("emit_fail", json!({"code":{"type":"string","enum":["PROVIDER_UNAVAILABLE","SANDBOX_UNAVAILABLE","IO_FAILURE","INVALID_AGENT_OUTPUT","INVALID_GOAL","CONFLICTING_OBLIGATIONS","CAPABILITY_DENIED","TOKEN_BUDGET","CALL_BUDGET","TIME_BUDGET","MISSING_REFERENCE","INVALIDATED_REFERENCE","RUNTIME_ERROR"]},"diagnostic_ref":reference(Some("ARTIFACT"))})));
    }
    envelope(actions)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accounting_partition_covers_exact_bytes_and_requires_identical_declarations() {
        let schema = response_schema(Role::Worker);
        let mut shared = schema.clone();
        shared["properties"]["actions"]["items"]["anyOf"]
            .as_array_mut()
            .unwrap()
            .truncate(4);
        let parts = accounting_parts(&schema, &shared).unwrap();
        assert_eq!(
            parts.iter().map(|p| p.text.as_str()).collect::<String>(),
            schema.to_string()
        );
        assert_eq!(parts.iter().filter(|p| p.protocol_only).count(), 4);
        assert!(!parts[1].protocol_only);
        // Same tool name with a changed signature is no longer a shared schema.
        shared["properties"]["actions"]["items"]["anyOf"][0]["description"] = "different".into();
        let parts = accounting_parts(&schema, &shared).unwrap();
        assert!(parts[1].protocol_only);
        assert!(!parts[2].protocol_only);
        shared["description"] = "different envelope".into();
        let parts = accounting_parts(&schema, &shared).unwrap();
        assert!(parts[0].protocol_only);
        assert!(parts.last().unwrap().protocol_only);
        assert!(accounting_parts(&json!({}), &shared).is_err());
    }

    #[test]
    fn prose_baseline_shares_exactly_the_common_declarations_with_myr() {
        let shared = prose_shared_schema();
        for role in [Role::Worker, Role::Reviewer] {
            let schema = response_schema(role);
            let parts = accounting_parts(&schema, &shared).unwrap();
            // Envelope and the three common tools are exempt; every MW/0
            // declaration (emit_*, review_claim) remains protocol schema.
            assert!(!parts[0].protocol_only && !parts.last().unwrap().protocol_only);
            let exempt: Vec<_> = parts[1..parts.len() - 1]
                .iter()
                .map(|p| p.protocol_only)
                .collect();
            let expected_protocol = if role == Role::Worker { 4 } else { 1 };
            assert_eq!(exempt[..4], [false, false, false, false]);
            assert_eq!(exempt[4..].len(), expected_protocol);
            assert!(exempt[4..].iter().all(|p| *p));
        }
        // Inside the baseline pipeline nothing is protocol schema.
        let worker = prose_response_schema(Role::Worker, &PROSE_RECIPIENTS[1..]);
        assert!(
            accounting_parts(&worker, &worker)
                .unwrap()
                .iter()
                .all(|p| !p.protocol_only)
        );
        let names: Vec<_> = shared["properties"]["actions"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["properties"]["tool"]["enum"][0].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "fetch",
                "fetch_many",
                "put_artifact",
                "finish",
                "send_message",
                "send_message",
                "write_file",
                "submit_review"
            ]
        );
    }
}
