//! Appendix C reference accounting. Every injected segment is tokenized alone;
//! provider telemetry and binary CAS traffic are never communication tokens.
use crate::Result;
use myr_core::{Cid, invalid};
use serde::{Deserialize, Serialize};

pub struct ReferenceTokenizer(tiktoken_rs::CoreBPE);

impl ReferenceTokenizer {
    pub fn new() -> Result<Self> {
        tiktoken_rs::cl100k_base()
            .map(Self)
            .map_err(|_| invalid("reference tokenizer initialization failed").into())
    }
    /// Special-token-looking text remains literal untrusted text, never a token
    /// control sequence. Equivalent to encode(..., disallowed_special=()).
    pub fn count(&self, text: &str) -> u64 {
        self.0.count_ordinary(text) as u64
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Pipeline {
    Prose,
    Mw0,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SegmentKind {
    System,
    Goal,
    DirectRepo,
    LocalTool,
    InterAgentProse,
    MwRender,
    CasReferenced,
    ValidationFeedback,
    ProtocolSchema,
}

impl SegmentKind {
    fn communication(self, pipeline: Pipeline) -> Result<bool> {
        match (pipeline, self) {
            (Pipeline::Prose, Self::InterAgentProse) => Ok(true),
            (
                Pipeline::Mw0,
                Self::MwRender
                | Self::CasReferenced
                | Self::ValidationFeedback
                | Self::ProtocolSchema,
            ) => Ok(true),
            (_, Self::System | Self::Goal | Self::DirectRepo | Self::LocalTool) => Ok(false),
            _ => Err(invalid("segment class does not belong to this pipeline").into()),
        }
    }
}

pub struct Segment<'a> {
    pub kind: SegmentKind,
    pub text: &'a str,
}

#[derive(Debug, Serialize)]
pub struct SegmentRecord {
    pub kind: SegmentKind,
    pub content_cid: Cid,
    pub utf8_bytes: u64,
    pub reference_tokens: u64,
    pub communication: bool,
    pub cas_derived: bool,
}

#[derive(Debug, Serialize)]
pub struct CallRecord {
    pub agent: String,
    pub segments: Vec<SegmentRecord>,
    pub context_reference_tokens: u64,
    pub communication_reference_tokens: u64,
    pub communication_context_bytes: u64,
    pub cas_context_bytes: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct Totals {
    pub context_reference_tokens: u64,
    pub communication_reference_tokens: u64,
    pub communication_context_bytes: u64,
    pub cas_raw_bytes: u64,
    pub cas_context_bytes: u64,
}

#[derive(Debug, Serialize)]
pub struct Ledger {
    pipeline: Pipeline,
    calls: Vec<CallRecord>,
    totals: Totals,
}

fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b)
        .ok_or_else(|| invalid("accounting counter overflow").into())
}

impl Ledger {
    pub fn pipeline(&self) -> Pipeline {
        self.pipeline
    }
    pub fn new(pipeline: Pipeline) -> Self {
        Self {
            pipeline,
            calls: vec![],
            totals: Totals::default(),
        }
    }
    pub fn calls(&self) -> &[CallRecord] {
        &self.calls
    }
    pub fn totals(&self) -> &Totals {
        &self.totals
    }

    /// Call once for every actual context injection, including repeated schemas
    /// and history. Does not deduplicate identical segment content or schemas.
    pub fn record_call(
        &mut self,
        tokenizer: &ReferenceTokenizer,
        agent: &str,
        segments: &[Segment<'_>],
    ) -> Result<&CallRecord> {
        self.record_call_with_cas(tokenizer, agent, segments, &[])
    }

    pub(crate) fn record_call_with_cas(
        &mut self,
        tokenizer: &ReferenceTokenizer,
        agent: &str,
        segments: &[Segment<'_>],
        cas_indices: &[usize],
    ) -> Result<&CallRecord> {
        for index in cas_indices {
            if !segments.get(*index).is_some_and(|s| {
                matches!(
                    s.kind,
                    SegmentKind::Goal
                        | SegmentKind::DirectRepo
                        | SegmentKind::MwRender
                        | SegmentKind::CasReferenced
                        | SegmentKind::InterAgentProse
                )
            }) {
                return Err(invalid("CAS context index must refer to retrieved content").into());
            }
        }
        if agent.trim().is_empty() {
            return Err(invalid("accounting requires an agent identity").into());
        }
        let mut call = CallRecord {
            agent: agent.into(),
            segments: vec![],
            context_reference_tokens: 0,
            communication_reference_tokens: 0,
            communication_context_bytes: 0,
            cas_context_bytes: 0,
        };
        for (index, segment) in segments.iter().enumerate() {
            let communication = segment.kind.communication(self.pipeline)?;
            let reference_tokens = tokenizer.count(segment.text);
            let utf8_bytes = segment.text.len() as u64;
            call.context_reference_tokens = add(call.context_reference_tokens, reference_tokens)?;
            if communication {
                call.communication_reference_tokens =
                    add(call.communication_reference_tokens, reference_tokens)?;
                call.communication_context_bytes =
                    add(call.communication_context_bytes, utf8_bytes)?;
            }
            let cas_derived =
                segment.kind == SegmentKind::CasReferenced || cas_indices.contains(&index);
            if cas_derived {
                call.cas_context_bytes = add(call.cas_context_bytes, utf8_bytes)?;
            }
            call.segments.push(SegmentRecord {
                kind: segment.kind,
                content_cid: myr_wire::artifact_cid(segment.text.as_bytes()),
                utf8_bytes,
                reference_tokens,
                communication,
                cas_derived,
            });
        }
        let totals = Totals {
            context_reference_tokens: add(
                self.totals.context_reference_tokens,
                call.context_reference_tokens,
            )?,
            communication_reference_tokens: add(
                self.totals.communication_reference_tokens,
                call.communication_reference_tokens,
            )?,
            communication_context_bytes: add(
                self.totals.communication_context_bytes,
                call.communication_context_bytes,
            )?,
            cas_raw_bytes: self.totals.cas_raw_bytes,
            cas_context_bytes: add(self.totals.cas_context_bytes, call.cas_context_bytes)?,
        };
        self.totals = totals;
        self.calls.push(call);
        Ok(self.calls.last().expect("appended call"))
    }

    /// A read not injected into context has no communication-token effect.
    pub fn record_cas_read(&mut self, raw_bytes: u64) -> Result<()> {
        self.totals.cas_raw_bytes = add(self.totals.cas_raw_bytes, raw_bytes)?;
        Ok(())
    }
}

/// Source and dependency identity to include in the preregistered manifest.
pub fn implementation_manifest() -> serde_json::Value {
    serde_json::json!({
        "tokenizer":"cl100k_base", "library":"tiktoken-rs", "library_version":"0.12.0",
        "asset_sha256":"223921b76ee99bde995b7ff738513eef100fb51d18c93597a113bcffe865b2a7",
        "special_tokens":"ordinary_literal_text", "segments":"independent_no_cross_segment_merges",
        "renderer_source_cid":myr_wire::artifact_cid(include_bytes!("../../myr-wire/src/lib.rs")),
        "accounting_source_cid":myr_wire::artifact_cid(include_bytes!("accounting.rs")),
        "context_source_cid":myr_wire::artifact_cid(include_bytes!("context.rs")),
        "schema_source_cid":myr_wire::artifact_cid(include_bytes!("../../myr-adapter/src/schema.rs")),
        "cargo_lock_cid":myr_wire::artifact_cid(include_bytes!("../../../Cargo.lock")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn known_tokens_and_literal_special_text() {
        let t = ReferenceTokenizer::new().unwrap();
        assert_eq!(t.0.encode_ordinary("hello world"), vec![15339, 1917]);
        assert_eq!(
            t.0.encode_ordinary("Hello, world!"),
            vec![9906, 11, 1917, 0]
        );
        assert_eq!(t.count(""), 0);
        assert!(t.count("<|endoftext|>") > 1);
    }
    #[test]
    fn independent_segments_and_repeated_schemas_count_each_injection() {
        let t = ReferenceTokenizer::new().unwrap();
        let mut ledger = Ledger::new(Pipeline::Mw0);
        let segments = [
            Segment {
                kind: SegmentKind::ProtocolSchema,
                text: "a",
            },
            Segment {
                kind: SegmentKind::MwRender,
                text: "b",
            },
            Segment {
                kind: SegmentKind::System,
                text: "system",
            },
        ];
        ledger.record_call(&t, "worker", &segments).unwrap();
        assert_eq!(ledger.totals.communication_reference_tokens, 2);
        assert_eq!(t.count("ab"), 1);
        ledger.record_call(&t, "worker", &segments).unwrap();
        assert_eq!(ledger.totals.communication_reference_tokens, 4);
        assert_eq!(ledger.totals.communication_context_bytes, 4);
        assert_eq!(ledger.totals.context_reference_tokens, 6);
    }
    #[test]
    fn pipeline_classification_and_cas_raw_bytes_are_separate() {
        let t = ReferenceTokenizer::new().unwrap();
        let mut prose = Ledger::new(Pipeline::Prose);
        prose.record_cas_read(123).unwrap();
        assert_eq!(prose.totals.communication_reference_tokens, 0);
        assert!(
            prose
                .record_call(
                    &t,
                    "worker",
                    &[Segment {
                        kind: SegmentKind::MwRender,
                        text: "claim"
                    }]
                )
                .is_err()
        );
        assert!(prose.calls.is_empty());
        prose
            .record_call(
                &t,
                "worker",
                &[
                    Segment {
                        kind: SegmentKind::InterAgentProse,
                        text: "hello world",
                    },
                    Segment {
                        kind: SegmentKind::DirectRepo,
                        text: "file",
                    },
                ],
            )
            .unwrap();
        assert_eq!(prose.totals.communication_reference_tokens, 2);
        let mut mw = Ledger::new(Pipeline::Mw0);
        mw.record_cas_read(2).unwrap();
        mw.record_call(
            &t,
            "worker",
            &[Segment {
                kind: SegmentKind::CasReferenced,
                text: "AP8=",
            }],
        )
        .unwrap();
        assert_eq!(mw.totals.cas_raw_bytes, 2);
        assert_eq!(mw.totals.cas_context_bytes, 4);
        assert_eq!(mw.totals.communication_reference_tokens, t.count("AP8="));
    }
}
