//! MW/0 canonical CBOR, domain-separated identity, and strict decoding.
//!
//! Decode then re-encode equality rejects noncanonical encodings, duplicates,
//! unknown fields, null optionals, trailing data, and non-normalized strings.
use ciborium::Value;
use myr_core::*;
use std::io::Cursor;
use unicode_normalization::UnicodeNormalization;

const OBJECT_DOMAIN: &[u8] = b"MYR\0MW0\0OBJ\0";
const BLOB_DOMAIN: &[u8] = b"MYR\0MW0\0BLOB\0";
pub const MAX_OBJECT_BYTES: usize = 16 * 1024 * 1024;

pub fn artifact_cid(bytes: &[u8]) -> Cid {
    hash(BLOB_DOMAIN, bytes)
}
pub fn message_cid(bytes: &[u8]) -> Cid {
    hash(OBJECT_DOMAIN, bytes)
}
fn hash(domain: &[u8], bytes: &[u8]) -> Cid {
    let mut hasher = blake3::Hasher::new();
    hasher.update(domain);
    hasher.update(bytes);
    Cid(*hasher.finalize().as_bytes())
}

pub fn encode(object: &Object) -> Result<Vec<u8>, ValidationError> {
    let normalized = normalize(object)?;
    let payload = match &normalized {
        Object::Task(v) => v.wire(),
        Object::Claim(v) => v.wire(),
        Object::Evidence(v) => v.wire(),
        Object::Fact(v) => v.wire(),
        Object::Assumption(v) => v.wire(),
        Object::Delta(v) => v.wire(),
        Object::Fail(v) => v.wire(),
        Object::Attest(v) => v.wire(),
        Object::Atom(v) => v.wire(),
        Object::PredicateDef(v) => v.wire(),
    };
    let bytes = cbor(&Value::Map(vec![
        (uint(0), uint(0)),
        (uint(1), normalized.kind().wire()),
        (uint(2), payload),
    ]))?;
    if bytes.len() > MAX_OBJECT_BYTES {
        return Err(invalid("MW/0 object exceeds size limit"));
    }
    Ok(bytes)
}

pub fn identify(object: &Object) -> Result<(ObjectRef, Vec<u8>), ValidationError> {
    let bytes = encode(object)?;
    Ok((ObjectRef::new(object.kind(), message_cid(&bytes)), bytes))
}

pub fn decode(bytes: &[u8]) -> Result<Object, ValidationError> {
    if bytes.len() > MAX_OBJECT_BYTES {
        return Err(invalid("MW/0 object exceeds size limit"));
    }
    let mut reader = Cursor::new(bytes);
    let value: Value =
        ciborium::from_reader(&mut reader).map_err(|e| invalid(format!("invalid CBOR: {e}")))?;
    if reader.position() != bytes.len() as u64 {
        return Err(invalid("trailing CBOR data"));
    }
    let fields = fields(&value, &[0, 1, 2])?;
    if u64::read(required(&fields, 0)?)? != 0 {
        return Err(invalid("unsupported MW version"));
    }
    let kind = Kind::read(required(&fields, 1)?)?;
    let p = required(&fields, 2)?;
    let object = match kind {
        Kind::Task => Object::Task(Task::read(p)?),
        Kind::Claim => Object::Claim(Claim::read(p)?),
        Kind::Evidence => Object::Evidence(Box::new(Evidence::read(p)?)),
        Kind::Fact => Object::Fact(Fact::read(p)?),
        Kind::Assumption => Object::Assumption(Assumption::read(p)?),
        Kind::Delta => Object::Delta(Delta::read(p)?),
        Kind::Fail => Object::Fail(Fail::read(p)?),
        Kind::Attest => Object::Attest(Attest::read(p)?),
        Kind::Atom => Object::Atom(Atom::read(p)?),
        Kind::PredicateDef => Object::PredicateDef(PredicateDef::read(p)?),
        _ => return Err(invalid("kind is not an MW/0 message")),
    };
    if encode(&object)? != bytes {
        return Err(invalid("noncanonical MW/0 encoding"));
    }
    Ok(object)
}

/// Frozen rendering path: compact, named-field JSON of the normalized object.
pub fn render(object: &Object) -> Result<String, ValidationError> {
    serde_json::to_string(&normalize(object)?).map_err(|e| invalid(e.to_string()))
}

pub fn normalize(object: &Object) -> Result<Object, ValidationError> {
    // All strings here belong to MW fields. Artifact bodies never enter this function.
    let mut value = serde_json::to_value(object).map_err(|e| invalid(e.to_string()))?;
    normalize_json(&mut value);
    let mut normalized: Object =
        serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
    match &mut normalized {
        Object::Task(v) => {
            set(&mut v.inputs)?;
            set(&mut v.capabilities)?;
            set(&mut v.assumptions)?;
            set(&mut v.obligations)?;
        }
        Object::Evidence(v) => {
            set(&mut v.correlation_domains)?;
            if let Some(c) = &mut v.command {
                pair_set(&mut c.environment)?;
                pair_set(&mut c.tool_versions)?;
            }
        }
        Object::Fact(v) => {
            set(&mut v.evidence)?;
            set(&mut v.assumptions)?;
        }
        Object::Assumption(v) => {
            set(&mut v.alternatives)?;
            set(&mut v.artifacts)?;
        }
        Object::Delta(v) => set(&mut v.assumptions)?,
        Object::Atom(v) => {
            for arg in &mut v.arguments {
                if let Argument::Decimal(d) = arg {
                    d.normalize()?;
                }
            }
        }
        _ => {}
    }
    normalized.validate()?;
    Ok(normalized)
}

fn normalize_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            *s = s.replace("\r\n", "\n").replace('\r', "\n").nfc().collect()
        }
        serde_json::Value::Array(v) => v.iter_mut().for_each(normalize_json),
        serde_json::Value::Object(m) => m.values_mut().for_each(normalize_json),
        _ => {}
    }
}

fn set<T: Wire + Clone>(values: &mut Vec<T>) -> Result<(), ValidationError> {
    let mut keyed = values
        .iter()
        .map(|v| Ok((cbor(&v.wire())?, v.clone())))
        .collect::<Result<Vec<_>, ValidationError>>()?;
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    if keyed.windows(2).any(|p| p[0].0 == p[1].0) {
        return Err(invalid("duplicate set member after normalization"));
    }
    *values = keyed.into_iter().map(|(_, v)| v).collect();
    Ok(())
}

fn pair_set(values: &mut Vec<(String, String)>) -> Result<(), ValidationError> {
    set(values)?;
    let mut names = std::collections::BTreeSet::new();
    if values
        .iter()
        .any(|(name, _)| name.is_empty() || !names.insert(name))
    {
        return Err(invalid("duplicate or empty command metadata key"));
    }
    Ok(())
}

fn cbor(value: &Value) -> Result<Vec<u8>, ValidationError> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).map_err(|e| invalid(e.to_string()))?;
    Ok(out)
}
fn uint(n: u64) -> Value {
    Value::Integer(n.into())
}
fn required<'a>(
    map: &std::collections::BTreeMap<u64, &'a Value>,
    key: u64,
) -> Result<&'a Value, ValidationError> {
    map.get(&key)
        .copied()
        .ok_or_else(|| invalid(format!("missing field {key}")))
}
fn fields<'a>(
    v: &'a Value,
    allowed: &[u64],
) -> Result<std::collections::BTreeMap<u64, &'a Value>, ValidationError> {
    let Value::Map(pairs) = v else {
        return Err(invalid("expected integer-keyed map"));
    };
    let mut result = std::collections::BTreeMap::new();
    for (key, value) in pairs {
        let key = u64::read(key)?;
        if !allowed.contains(&key) || result.insert(key, value).is_some() {
            return Err(invalid("unknown or duplicate map key"));
        }
    }
    Ok(result)
}
fn array(v: &Value, len: usize) -> Result<&[Value], ValidationError> {
    match v {
        Value::Array(v) if v.len() == len => Ok(v),
        _ => Err(invalid(format!("expected array of length {len}"))),
    }
}

trait Wire: Sized {
    fn wire(&self) -> Value;
    fn read(v: &Value) -> Result<Self, ValidationError>;
    fn absent(&self) -> bool {
        false
    }
    fn field(v: Option<&&Value>) -> Result<Self, ValidationError> {
        Self::read(v.ok_or_else(|| invalid("missing required field"))?)
    }
}

macro_rules! unsigned {
    ($($t:ty),+) => { $(impl Wire for $t {
        fn wire(&self) -> Value { uint(u64::from(*self)) }
        fn read(v: &Value) -> Result<Self, ValidationError> {
            if let Value::Integer(n) = v { (*n).try_into().map_err(|_| invalid("integer out of range")) } else { Err(invalid("expected unsigned integer")) }
        }
    })+ }
}
unsigned!(u8, u32, u64);
impl Wire for i64 {
    fn wire(&self) -> Value {
        Value::Integer((*self).into())
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        if let Value::Integer(n) = v {
            (*n).try_into().map_err(|_| invalid("integer out of range"))
        } else {
            Err(invalid("expected integer"))
        }
    }
}
impl Wire for bool {
    fn wire(&self) -> Value {
        Value::Bool(*self)
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        if let Value::Bool(b) = v {
            Ok(*b)
        } else {
            Err(invalid("expected boolean"))
        }
    }
}
impl Wire for String {
    fn wire(&self) -> Value {
        Value::Text(self.clone())
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        if let Value::Text(s) = v {
            Ok(s.clone())
        } else {
            Err(invalid("expected text"))
        }
    }
}
impl<T: Wire> Wire for Vec<T> {
    fn wire(&self) -> Value {
        Value::Array(self.iter().map(Wire::wire).collect())
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        if let Value::Array(values) = v {
            values.iter().map(T::read).collect()
        } else {
            Err(invalid("expected array"))
        }
    }
}
impl<T: Wire> Wire for Option<T> {
    fn wire(&self) -> Value {
        self.as_ref()
            .expect("absent optional must be omitted")
            .wire()
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        T::read(v).map(Some)
    }
    fn absent(&self) -> bool {
        self.is_none()
    }
    fn field(v: Option<&&Value>) -> Result<Self, ValidationError> {
        v.map(|v| T::read(v)).transpose()
    }
}
impl Wire for (String, String) {
    fn wire(&self) -> Value {
        Value::Array(vec![self.0.wire(), self.1.wire()])
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        let a = array(v, 2)?;
        Ok((String::read(&a[0])?, String::read(&a[1])?))
    }
}
impl Wire for Cid {
    fn wire(&self) -> Value {
        Value::Bytes(self.0.to_vec())
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        if let Value::Bytes(b) = v {
            Ok(Cid(b
                .as_slice()
                .try_into()
                .map_err(|_| invalid("CID requires exactly 32 bytes"))?))
        } else {
            Err(invalid("expected CID byte string"))
        }
    }
}
impl Wire for ObjectRef {
    fn wire(&self) -> Value {
        Value::Array(vec![self.kind.wire(), self.cid.wire()])
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        let a = array(v, 2)?;
        Ok(Self {
            kind: Kind::read(&a[0])?,
            cid: Cid::read(&a[1])?,
        })
    }
}
impl Wire for Decimal {
    fn wire(&self) -> Value {
        Value::Array(vec![self.mantissa.wire(), self.exponent.wire()])
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        let a = array(v, 2)?;
        Ok(Self {
            mantissa: i64::read(&a[0])?,
            exponent: i64::read(&a[1])?,
        })
    }
}
impl Wire for Argument {
    fn wire(&self) -> Value {
        let value = match self {
            Self::Ref(v) => v.wire(),
            Self::Text(v) => v.wire(),
            Self::Integer(v) => v.wire(),
            Self::Boolean(v) => v.wire(),
            Self::Decimal(v) => v.wire(),
        };
        Value::Array(vec![self.arg_type().wire(), value])
    }
    fn read(v: &Value) -> Result<Self, ValidationError> {
        let a = array(v, 2)?;
        Ok(match ArgType::read(&a[0])? {
            ArgType::Ref => Self::Ref(ObjectRef::read(&a[1])?),
            ArgType::Text => Self::Text(String::read(&a[1])?),
            ArgType::Integer => Self::Integer(i64::read(&a[1])?),
            ArgType::Boolean => Self::Boolean(bool::read(&a[1])?),
            ArgType::Decimal => Self::Decimal(Decimal::read(&a[1])?),
        })
    }
}
macro_rules! enums {
    ($($t:ty),+) => { $(impl Wire for $t {
        fn wire(&self) -> Value { (*self as u8).wire() }
        fn read(v: &Value) -> Result<Self, ValidationError> { Self::try_from(u8::read(v)?) }
    })+ }
}
enums!(
    Kind,
    ArgType,
    PredicateClass,
    Verdict,
    Mechanism,
    DeltaCodec,
    FailClass,
    FailCode
);

macro_rules! record {
    ($t:ident { $($key:literal => $field:ident),+ $(,)? }) => {
        impl Wire for $t {
            fn wire(&self) -> Value {
                let mut entries = Vec::new();
                $(if !self.$field.absent() { entries.push((uint($key), self.$field.wire())); })+
                Value::Map(entries)
            }
            fn read(v: &Value) -> Result<Self, ValidationError> {
                let m = fields(v, &[$($key),+])?;
                Ok(Self { $($field: Wire::field(m.get(&$key))?),+ })
            }
        }
    }
}
record!(Scope { 0 => goal, 1 => snapshot });
record!(PredicateDef { 0 => name, 1 => version, 2 => arguments, 3 => semantics, 4 => class, 5 => provenance });
record!(Atom { 0 => predicate, 1 => arguments });
record!(Claim { 0 => atom, 1 => polarity, 2 => scope });
record!(Lineage { 0 => provider_id, 1 => family_id, 2 => checkpoint_id, 3 => procedure_id });
record!(Attest { 0 => claim, 1 => task, 2 => agent, 3 => lineage });
record!(CommandRecord { 0 => argv, 1 => cwd, 2 => environment, 3 => tool_versions, 4 => exit_code, 5 => stdout, 6 => stderr, 7 => sandbox });
record!(Evidence { 0 => claim, 1 => scope, 2 => verdict, 3 => mechanism, 4 => lineage, 5 => command, 6 => rationale, 7 => correlation_domains });
record!(Fact { 0 => claim, 1 => evidence, 2 => confidence_ppm, 3 => policy, 4 => assumptions });
record!(Assumption { 0 => scope, 1 => question, 2 => chosen, 3 => alternatives, 4 => rationale, 5 => artifacts, 6 => invalidates });
record!(Task { 0 => scope, 1 => inputs, 2 => capabilities, 3 => assumptions, 4 => obligations, 5 => instruction, 6 => token_budget, 7 => call_budget, 8 => time_budget_ms });
record!(Delta { 0 => scope, 1 => path, 2 => base, 3 => patch, 4 => result, 5 => codec, 6 => assumptions });
record!(Fail { 0 => class, 1 => code, 2 => diagnostic, 3 => task });
