//! Deterministic context assembly from explicit runtime-selected segments.
//! No history lookup, summarization, or implicit cross-agent context propagation.
use crate::{
    Result,
    accounting::{Ledger, ReferenceTokenizer, Segment, SegmentKind},
};
use myr_core::{ObjectRef, invalid};

pub struct PreparedContext {
    system: String,
    prompt: String,
    segments: Vec<(SegmentKind, String)>,
    cas_indices: Vec<usize>,
}

impl PreparedContext {
    /// Two LF bytes terminate each supplied prompt segment. They belong to that
    /// segment's accounting class, so separators are never hidden from counts.
    pub fn new(system: &str, prompt_segments: &[Segment<'_>]) -> Result<Self> {
        let mut segments = vec![(SegmentKind::System, system.to_owned())];
        let mut prompt = String::new();
        for segment in prompt_segments {
            if matches!(
                segment.kind,
                SegmentKind::System | SegmentKind::ProtocolSchema
            ) {
                return Err(invalid(
                    "system and protocol schema require their own context channels",
                )
                .into());
            }
            let text = format!("{}\n\n", segment.text);
            prompt.push_str(&text);
            segments.push((segment.kind, text));
        }
        Ok(Self {
            system: system.into(),
            prompt,
            segments,
            cas_indices: vec![],
        })
    }
    pub fn system(&self) -> &str {
        &self.system
    }
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// Mark successful logical CAS fetches without changing token attribution.
    /// Indices refer to the supplied prompt segments, excluding the system text.
    pub fn with_cas_prompt_segments(mut self, indices: &[usize]) -> Result<Self> {
        let mut marked = Vec::new();
        for index in indices {
            let index = index
                .checked_add(1)
                .ok_or_else(|| invalid("CAS segment index overflow"))?;
            if !self.segments.get(index).is_some_and(|(kind, _)| {
                matches!(
                    kind,
                    SegmentKind::Goal
                        | SegmentKind::DirectRepo
                        | SegmentKind::LocalTool
                        | SegmentKind::MwRender
                        | SegmentKind::CasReferenced
                        | SegmentKind::InterAgentProse
                        | SegmentKind::InterAgentArtifact
                )
            }) {
                return Err(
                    invalid("CAS segment does not refer to retrieved prompt content").into(),
                );
            }
            marked.push(index);
        }
        marked.sort_unstable();
        marked.dedup();
        self.cas_indices = marked;
        Ok(self)
    }

    /// Use the exact response schema that will be passed to the transport and
    /// the baseline's actual shared schema. Shared bytes remain in full-context
    /// budget counts; only identical shared declarations are excluded from the
    /// communication metric.
    pub fn record_with_schema(
        &self,
        tokenizer: &ReferenceTokenizer,
        ledger: &mut Ledger,
        audit_store: &myr_cas::Store,
        agent: &str,
        schemas: (&serde_json::Value, &serde_json::Value),
    ) -> Result<Vec<ObjectRef>> {
        let parts = myr_adapter::schema::accounting_parts(schemas.0, schemas.1)?;
        let extras: Vec<_> = parts
            .iter()
            .map(|p| Segment {
                kind: if p.protocol_only {
                    SegmentKind::ProtocolSchema
                } else {
                    SegmentKind::System
                },
                text: &p.text,
            })
            .collect();
        self.record(tokenizer, ledger, audit_store, agent, &extras)
    }

    /// Retain the exact counted bytes in a harness-private audit CAS, never the
    /// shared agent CAS (which must not acquire source-view context). Returns
    /// artifact references in the same order as the new ledger call's segments.
    /// External response schemas must be accounted in the same call via extras.
    pub fn record(
        &self,
        tokenizer: &ReferenceTokenizer,
        ledger: &mut Ledger,
        audit_store: &myr_cas::Store,
        agent: &str,
        extras: &[Segment<'_>],
    ) -> Result<Vec<ObjectRef>> {
        if extras
            .iter()
            .any(|s| !matches!(s.kind, SegmentKind::System | SegmentKind::ProtocolSchema))
        {
            return Err(invalid("extra context must be shared or protocol schema text").into());
        }
        let mut segments: Vec<_> = self
            .segments
            .iter()
            .map(|(kind, text)| Segment { kind: *kind, text })
            .collect();
        segments.extend(extras.iter().map(|s| Segment {
            kind: s.kind,
            text: s.text,
        }));
        // Validate classification before any ledger change or retained data.
        let mut validation = Ledger::new(ledger.pipeline());
        validation.record_call_with_cas(tokenizer, agent, &segments, &self.cas_indices)?;
        let references = segments
            .iter()
            .map(|s| {
                audit_store
                    .put_artifact(s.text.as_bytes())
                    .map_err(Into::into)
            })
            .collect::<Result<Vec<_>>>()?;
        ledger.record_call_with_cas(tokenizer, agent, &segments, &self.cas_indices)?;
        Ok(references)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::Pipeline;
    #[test]
    fn cas_provenance_is_orthogonal_to_communication_and_preserves_context_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let audit = myr_cas::Store::open(dir.path()).unwrap();
        let tokenizer = ReferenceTokenizer::new().unwrap();
        let segments = [
            Segment {
                kind: SegmentKind::Goal,
                text: "instruction",
            },
            Segment {
                kind: SegmentKind::DirectRepo,
                text: "AP8=",
            },
            Segment {
                kind: SegmentKind::MwRender,
                text: "typed object",
            },
            Segment {
                kind: SegmentKind::CasReferenced,
                text: "artifact",
            },
        ];
        let mut ordinary = Ledger::new(Pipeline::Mw0);
        let context = PreparedContext::new("system", &segments).unwrap();
        context
            .record(&tokenizer, &mut ordinary, &audit, "worker", &[])
            .unwrap();
        let original_prompt = context.prompt().to_owned();
        let context = context.with_cas_prompt_segments(&[0, 1, 2, 3, 1]).unwrap();
        let mut fetched = Ledger::new(Pipeline::Mw0);
        context
            .record(&tokenizer, &mut fetched, &audit, "worker", &[])
            .unwrap();
        assert_eq!(context.prompt(), original_prompt);
        assert_eq!(
            fetched.totals().cas_context_bytes,
            original_prompt.len() as u64
        );
        assert_eq!(fetched.totals().cas_raw_bytes, 0);
        assert_eq!(
            fetched.totals().communication_reference_tokens,
            ordinary.totals().communication_reference_tokens
        );
        assert_eq!(
            fetched.totals().context_reference_tokens,
            ordinary.totals().context_reference_tokens
        );
        let records = &fetched.calls()[0].segments;
        assert!(!records[0].cas_derived);
        assert!(records[1..].iter().all(|s| s.cas_derived));
        assert!(!records[1].communication && !records[2].communication);
        assert!(records[3].communication && records[4].communication);
        context
            .record(&tokenizer, &mut fetched, &audit, "worker", &[])
            .unwrap();
        assert_eq!(
            fetched.totals().cas_context_bytes,
            2 * original_prompt.len() as u64
        );
        assert!(
            PreparedContext::new("system", &segments)
                .unwrap()
                .with_cas_prompt_segments(&[4])
                .is_err()
        );
    }
    #[test]
    fn retained_segments_exactly_reconstruct_dispatched_strings() {
        let t = ReferenceTokenizer::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let audit = myr_cas::Store::open(dir.path()).unwrap();
        let context = PreparedContext::new(
            "system",
            &[
                Segment {
                    kind: SegmentKind::Goal,
                    text: "goal",
                },
                Segment {
                    kind: SegmentKind::MwRender,
                    text: "{\"text\":\"é\"}",
                },
                Segment {
                    kind: SegmentKind::CasReferenced,
                    text: "AP8=",
                },
            ],
        )
        .unwrap();
        let mut ledger = Ledger::new(Pipeline::Mw0);
        let refs = context
            .record(
                &t,
                &mut ledger,
                &audit,
                "worker",
                &[Segment {
                    kind: SegmentKind::ProtocolSchema,
                    text: "{}",
                }],
            )
            .unwrap();
        assert_eq!(audit.get(refs[0]).unwrap(), context.system().as_bytes());
        let reconstructed: Vec<u8> = refs[1..4]
            .iter()
            .flat_map(|r| audit.get(*r).unwrap())
            .collect();
        assert_eq!(reconstructed, context.prompt().as_bytes());
        for (r, record) in refs.iter().zip(&ledger.calls()[0].segments) {
            let bytes = audit.get(*r).unwrap();
            assert_eq!(r.cid, record.content_cid);
            assert_eq!(bytes.len() as u64, record.utf8_bytes);
            assert_eq!(
                t.count(std::str::from_utf8(&bytes).unwrap()),
                record.reference_tokens
            );
        }
        assert_eq!(ledger.totals().cas_context_bytes, 6);
        let schema = myr_adapter::schema::response_schema(myr_adapter::Role::Worker);
        let mut shared = schema.clone();
        shared["properties"]["actions"]["items"]["anyOf"]
            .as_array_mut()
            .unwrap()
            .truncate(4);
        let mut ledger = Ledger::new(Pipeline::Mw0);
        let refs = context
            .record_with_schema(&t, &mut ledger, &audit, "worker", (&schema, &shared))
            .unwrap();
        let schema_bytes: Vec<u8> = refs[4..]
            .iter()
            .flat_map(|r| audit.get(*r).unwrap())
            .collect();
        assert_eq!(schema_bytes, schema.to_string().as_bytes());
        assert_eq!(
            ledger.calls()[0]
                .segments
                .iter()
                .filter(|s| s.kind == SegmentKind::ProtocolSchema)
                .count(),
            4
        );
    }
    #[test]
    fn no_implicit_history_and_wrong_pipeline_does_not_append_call() {
        let t = ReferenceTokenizer::new().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let audit = myr_cas::Store::open(dir.path()).unwrap();
        let mut ledger = Ledger::new(Pipeline::Prose);
        let first = PreparedContext::new(
            "s",
            &[Segment {
                kind: SegmentKind::InterAgentProse,
                text: "first",
            }],
        )
        .unwrap();
        first
            .record(&t, &mut ledger, &audit, "worker", &[])
            .unwrap();
        let second = PreparedContext::new(
            "s",
            &[Segment {
                kind: SegmentKind::InterAgentProse,
                text: "second",
            }],
        )
        .unwrap();
        assert!(!second.prompt().contains("first"));
        assert!(
            second
                .record(
                    &t,
                    &mut ledger,
                    &audit,
                    "worker",
                    &[Segment {
                        kind: SegmentKind::ProtocolSchema,
                        text: "mw-only"
                    }]
                )
                .is_err()
        );
        assert_eq!(ledger.calls().len(), 1);
    }
}
