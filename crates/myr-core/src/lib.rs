//! Provider-independent semantic types. Wire encoding lives in `myr-wire`.
//! Only the trusted runtime may construct evidence and facts for graph insertion.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cid(pub [u8; 32]);

impl fmt::Display for Cid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b3:{}", hex::encode(self.0))
    }
}

impl FromStr for Cid {
    type Err = ValidationError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let raw = value
            .strip_prefix("b3:")
            .ok_or_else(|| invalid("CID must start with b3:"))?;
        if raw.len() != 64
            || raw
                .bytes()
                .any(|b| !b.is_ascii_digit() && !(b'a'..=b'f').contains(&b))
        {
            return Err(invalid("CID requires 64 lowercase hexadecimal digits"));
        }
        let mut bytes = [0; 32];
        hex::decode_to_slice(raw, &mut bytes).map_err(|_| invalid("invalid CID"))?;
        Ok(Self(bytes))
    }
}

impl Serialize for Cid {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Cid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

macro_rules! closed_enum {
    ($name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        #[repr(u8)]
        pub enum $name { $($variant = $value),+ }
        impl TryFrom<u8> for $name {
            type Error = ValidationError;
            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value { $($value => Ok(Self::$variant),)+ _ => Err(invalid(concat!("unknown ", stringify!($name)))) }
            }
        }
    }
}

closed_enum!(Kind { Artifact=0, Task=1, Claim=2, Evidence=3, Fact=4, Assumption=5, Delta=6, Fail=7, Attest=8, Atom=9, PredicateDef=10, Goal=11 });
closed_enum!(ArgType { Ref=0, Text=1, Integer=2, Boolean=3, Decimal=4 });
closed_enum!(PredicateClass { Decidable=0, Empirical=1, Heuristic=2, Unresolved=3 });
closed_enum!(Verdict { Supports=0, Contradicts=1, Inconclusive=2 });
closed_enum!(Mechanism { Deterministic=0, LlmReview=1 });
closed_enum!(DeltaCodec { Replacement=0, UnifiedDiff=1 });
closed_enum!(FailClass { Infrastructure=0, Validation=1, Goal=2, Capability=3, Budget=4, Dependency=5, Internal=6 });
closed_enum!(FailCode { ProviderUnavailable=0, SandboxUnavailable=1, IoFailure=2, InvalidAgentOutput=3, InvalidGoal=4, ConflictingObligations=5, CapabilityDenied=6, TokenBudget=7, CallBudget=8, TimeBudget=9, MissingReference=10, InvalidatedReference=11, RuntimeError=12 });
closed_enum!(MissionState { Complete=0, CompleteWithAssumptions=1, Partial=2, Unsat=3, InvalidGoal=4 });

impl FailCode {
    pub fn class(self) -> FailClass {
        use FailCode::*;
        match self {
            ProviderUnavailable | SandboxUnavailable | IoFailure => FailClass::Infrastructure,
            InvalidAgentOutput => FailClass::Validation,
            InvalidGoal | ConflictingObligations => FailClass::Goal,
            CapabilityDenied => FailClass::Capability,
            TokenBudget | CallBudget | TimeBudget => FailClass::Budget,
            MissingReference | InvalidatedReference => FailClass::Dependency,
            RuntimeError => FailClass::Internal,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    pub kind: Kind,
    pub cid: Cid,
}

impl ObjectRef {
    pub fn new(kind: Kind, cid: Cid) -> Self {
        Self { kind, cid }
    }
    pub fn require(self, kind: Kind) -> Result<(), ValidationError> {
        if self.kind == kind {
            Ok(())
        } else {
            Err(invalid(format!(
                "expected {kind:?} reference, received {:?}",
                self.kind
            )))
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub goal: ObjectRef,
    pub snapshot: ObjectRef,
}

impl Scope {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.goal.require(Kind::Goal)?;
        self.snapshot.require(Kind::Artifact)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Decimal {
    pub mantissa: i64,
    pub exponent: i64,
}

impl Decimal {
    pub fn normalize(&mut self) -> Result<(), ValidationError> {
        if self.mantissa == 0 {
            self.exponent = 0;
        } else {
            while self.mantissa % 10 == 0 {
                self.mantissa /= 10;
                self.exponent = self
                    .exponent
                    .checked_add(1)
                    .ok_or_else(|| invalid("decimal exponent overflow"))?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Argument {
    Ref(ObjectRef),
    Text(String),
    Integer(i64),
    Boolean(bool),
    Decimal(Decimal),
}

impl Argument {
    pub fn arg_type(&self) -> ArgType {
        match self {
            Self::Ref(_) => ArgType::Ref,
            Self::Text(_) => ArgType::Text,
            Self::Integer(_) => ArgType::Integer,
            Self::Boolean(_) => ArgType::Boolean,
            Self::Decimal(_) => ArgType::Decimal,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PredicateDef {
    pub name: String,
    pub version: u32,
    pub arguments: Vec<ArgType>,
    pub semantics: String,
    pub class: PredicateClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ObjectRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Atom {
    pub predicate: ObjectRef,
    pub arguments: Vec<Argument>,
}

impl Atom {
    /// Validate against the referenced definition without weakening opaque refs
    /// into interchangeable kinds for built-in predicates with artifact semantics.
    pub fn validate_arguments(&self, predicate: &PredicateDef) -> Result<(), ValidationError> {
        if self
            .arguments
            .iter()
            .map(Argument::arg_type)
            .collect::<Vec<_>>()
            != predicate.arguments
        {
            return Err(invalid("predicate argument type or arity mismatch"));
        }
        if predicate.name == "core.artifact_equals" {
            for argument in &self.arguments {
                let Argument::Ref(reference) = argument else {
                    return Err(invalid("artifact equality requires artifact references"));
                };
                reference.require(Kind::Artifact)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub atom: ObjectRef,
    pub polarity: bool,
    pub scope: Scope,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub provider_id: String,
    pub family_id: String,
    pub checkpoint_id: String,
    pub procedure_id: String,
}

impl Lineage {
    pub fn known(&self) -> bool {
        [
            &self.provider_id,
            &self.family_id,
            &self.checkpoint_id,
            &self.procedure_id,
        ]
        .iter()
        .all(|s| {
            !s.trim().is_empty()
                && !["unknown", "ambiguous"].contains(&s.trim().to_ascii_lowercase().as_str())
        })
    }
    pub fn different_from(&self, other: &Self) -> bool {
        self.known()
            && other.known()
            && self.provider_id != other.provider_id
            && self.family_id != other.family_id
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Attest {
    pub claim: ObjectRef,
    pub task: ObjectRef,
    pub agent: String,
    pub lineage: Lineage,
}

/// Runtime-recorded command invocation. No shell interpolation is implied by argv.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CommandRecord {
    pub argv: Vec<String>,
    pub cwd: String,
    pub environment: Vec<(String, String)>,
    pub tool_versions: Vec<(String, String)>,
    pub exit_code: i64,
    pub stdout: ObjectRef,
    pub stderr: ObjectRef,
    pub sandbox: ObjectRef,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub claim: ObjectRef,
    pub scope: Scope,
    pub verdict: Verdict,
    pub mechanism: Mechanism,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lineage: Option<Lineage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CommandRecord>,
    pub rationale: ObjectRef,
    pub correlation_domains: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub claim: ObjectRef,
    pub evidence: Vec<ObjectRef>,
    pub confidence_ppm: u32,
    pub policy: String,
    pub assumptions: Vec<ObjectRef>,
}

/// Invalidation is an append-only ASSUMPTION event, referencing its predecessor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assumption {
    pub scope: Scope,
    pub question: String,
    pub chosen: String,
    pub alternatives: Vec<String>,
    pub rationale: ObjectRef,
    pub artifacts: Vec<ObjectRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidates: Option<ObjectRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub scope: Scope,
    pub inputs: Vec<ObjectRef>,
    pub capabilities: Vec<String>,
    pub assumptions: Vec<ObjectRef>,
    pub obligations: Vec<ObjectRef>,
    pub instruction: ObjectRef,
    pub token_budget: u64,
    pub call_budget: u32,
    pub time_budget_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    pub scope: Scope,
    pub path: String,
    pub base: ObjectRef,
    pub patch: ObjectRef,
    pub result: ObjectRef,
    pub codec: DeltaCodec,
    pub assumptions: Vec<ObjectRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fail {
    pub class: FailClass,
    pub code: FailCode,
    pub diagnostic: ObjectRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<ObjectRef>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
pub enum Object {
    Task(Task),
    Claim(Claim),
    Evidence(Box<Evidence>),
    Fact(Fact),
    Assumption(Assumption),
    Delta(Delta),
    Fail(Fail),
    Attest(Attest),
    Atom(Atom),
    PredicateDef(PredicateDef),
}

impl Object {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Task(_) => Kind::Task,
            Self::Claim(_) => Kind::Claim,
            Self::Evidence(_) => Kind::Evidence,
            Self::Fact(_) => Kind::Fact,
            Self::Assumption(_) => Kind::Assumption,
            Self::Delta(_) => Kind::Delta,
            Self::Fail(_) => Kind::Fail,
            Self::Attest(_) => Kind::Attest,
            Self::Atom(_) => Kind::Atom,
            Self::PredicateDef(_) => Kind::PredicateDef,
        }
    }

    /// Local constraints only. Existence, capability, graph authority, and predicate
    /// argument types are checked at the trusted insertion boundary.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Task(t) => {
                t.scope.validate()?;
                t.instruction.require(Kind::Artifact)?;
                require_all(&t.assumptions, Kind::Assumption)?;
                require_all(&t.obligations, Kind::Atom)?;
                if t.call_budget == 0 || t.token_budget == 0 || t.time_budget_ms == 0 {
                    return Err(invalid("task budgets must be positive"));
                }
            }
            Self::Claim(c) => {
                c.atom.require(Kind::Atom)?;
                c.scope.validate()?;
            }
            Self::Evidence(e) => {
                e.claim.require(Kind::Claim)?;
                e.scope.validate()?;
                e.rationale.require(Kind::Artifact)?;
                match e.mechanism {
                    Mechanism::Deterministic => {
                        if e.lineage.is_some() {
                            return Err(invalid(
                                "deterministic evidence cannot declare LLM lineage",
                            ));
                        }
                        let c = e.command.as_ref().ok_or_else(|| {
                            invalid("deterministic evidence requires command provenance")
                        })?;
                        if c.argv.is_empty()
                            || c.argv[0].is_empty()
                            || c.cwd.is_empty()
                            || c.tool_versions.is_empty()
                        {
                            return Err(invalid("incomplete command provenance"));
                        }
                        c.stdout.require(Kind::Artifact)?;
                        c.stderr.require(Kind::Artifact)?;
                        c.sandbox.require(Kind::Artifact)?;
                    }
                    Mechanism::LlmReview => {
                        if e.command.is_some() {
                            return Err(invalid("LLM review cannot declare command provenance"));
                        }
                        // Unknown lineage is retained, but never supplies independence.
                        if e.lineage.is_none() {
                            return Err(invalid("LLM review requires runtime lineage fields"));
                        }
                    }
                }
            }
            Self::Fact(f) => {
                f.claim.require(Kind::Claim)?;
                require_all(&f.evidence, Kind::Evidence)?;
                require_all(&f.assumptions, Kind::Assumption)?;
                if f.evidence.is_empty()
                    || ![800_000, 900_000, 950_000, 970_000].contains(&f.confidence_ppm)
                    || f.policy != "promotion-policy-v0"
                {
                    return Err(invalid("invalid v0 fact policy or confidence"));
                }
            }
            Self::Assumption(a) => {
                a.scope.validate()?;
                a.rationale.require(Kind::Artifact)?;
                require_all(&a.artifacts, Kind::Artifact)?;
                if a.question.is_empty() || a.chosen.is_empty() || a.alternatives.is_empty() {
                    return Err(invalid(
                        "assumption requires a question, choice, and alternatives",
                    ));
                }
                if let Some(r) = a.invalidates {
                    r.require(Kind::Assumption)?;
                }
            }
            Self::Delta(d) => {
                d.scope.validate()?;
                validate_relative_path(&d.path)?;
                for r in [d.base, d.patch, d.result] {
                    r.require(Kind::Artifact)?;
                }
                require_all(&d.assumptions, Kind::Assumption)?;
            }
            Self::Fail(f) => {
                if f.code.class() != f.class {
                    return Err(invalid("FAIL class does not match registered code"));
                }
                f.diagnostic.require(Kind::Artifact)?;
                if let Some(r) = f.task {
                    r.require(Kind::Task)?;
                }
            }
            Self::Attest(a) => {
                a.claim.require(Kind::Claim)?;
                a.task.require(Kind::Task)?;
                if a.agent.is_empty() {
                    return Err(invalid("empty attesting agent"));
                }
            }
            Self::Atom(a) => {
                a.predicate.require(Kind::PredicateDef)?;
            }
            Self::PredicateDef(p) => {
                if p.version == 0 || p.semantics.is_empty() {
                    return Err(invalid("predicate requires version and semantics"));
                }
                if p.name.starts_with("core.") {
                    if !core_predicates().contains(p) {
                        return Err(invalid("core predicate vocabulary is closed"));
                    }
                } else if p.name.starts_with("mission.") && p.name.len() > 8 {
                    p.provenance
                        .ok_or_else(|| invalid("mission predicate requires provenance"))?
                        .require(Kind::Artifact)?;
                } else {
                    return Err(invalid(
                        "predicate name requires core.* or mission.* namespace",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn require_all(refs: &[ObjectRef], kind: Kind) -> Result<(), ValidationError> {
    for r in refs {
        r.require(kind)?;
    }
    Ok(())
}

/// Portable repository paths: reject traversal, alternate separators, Windows
/// streams/device paths, and aliases before applying filesystem capabilities.
pub fn validate_relative_path(path: &str) -> Result<(), ValidationError> {
    if path.is_empty() || path.contains(['\\', ':', '\0']) || path.chars().any(|c| c.is_control()) {
        return Err(invalid("invalid relative path"));
    }
    for component in path.split('/') {
        let stem = component
            .split('.')
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        if component.is_empty()
            || component == "."
            || component == ".."
            || component.ends_with([' ', '.'])
            || component.contains(['*', '?', '"', '<', '>', '|'])
            || ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(invalid("unsafe or nonportable path component"));
        }
    }
    Ok(())
}

pub fn core_predicates() -> Vec<PredicateDef> {
    vec![
        PredicateDef {
            name: "core.command_succeeds".into(),
            version: 1,
            arguments: vec![ArgType::Ref],
            semantics: "The configured command exits with status zero in the sealed snapshot."
                .into(),
            class: PredicateClass::Decidable,
            provenance: None,
        },
        PredicateDef {
            name: "core.artifact_equals".into(),
            version: 1,
            arguments: vec![ArgType::Ref, ArgType::Ref],
            semantics: "The two artifacts have identical opaque bytes.".into(),
            class: PredicateClass::Decidable,
            provenance: None,
        },
        PredicateDef {
            name: "core.code_review".into(),
            version: 1,
            arguments: vec![ArgType::Ref],
            semantics: "The artifact satisfies the configured code review procedure.".into(),
            class: PredicateClass::Heuristic,
            provenance: None,
        },
    ]
}

#[derive(Clone, Debug, thiserror::Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ValidationError(pub String);

pub fn invalid(message: impl Into<String>) -> ValidationError {
    ValidationError(message.into())
}

pub mod promotion;
