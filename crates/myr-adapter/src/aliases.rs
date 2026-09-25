//! Model-facing short references (cost pilot, step B2). The specification
//! leaves the text form of a CID open: "text representation is not semantic".
//! A mission-wide table maps `#n` to full CIDs in first-appearance order. The
//! table is a view: graph, storage, validation and semantic records always
//! use full CIDs, and every model output is resolved before validation.
use myr_core::Cid;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Aliases {
    cids: Vec<Cid>,
    index: BTreeMap<Cid, usize>,
}

fn full_cid(text: &str) -> Option<Cid> {
    if text.len() == 67 && text.starts_with("b3:") {
        text.parse().ok()
    } else {
        None
    }
}

impl Aliases {
    /// Full CIDs in alias order: `#1` is the first entry. For private audit.
    pub fn table(&self) -> &[Cid] {
        &self.cids
    }

    pub fn alias(&mut self, cid: Cid) -> String {
        let next = self.cids.len();
        let position = *self.index.entry(cid).or_insert(next);
        if position == next {
            self.cids.push(cid);
        }
        format!("#{}", position + 1)
    }

    /// Replace every string that is exactly a full CID. Runtime record text
    /// (`content_text` of runtime artifacts) is aliased only when `records`;
    /// repository and agent-authored content is never rewritten.
    pub fn view(&mut self, value: &mut Value, records: bool) {
        match value {
            Value::String(text) => {
                if let Some(cid) = full_cid(text) {
                    *text = self.alias(cid);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.view(item, records);
                }
            }
            Value::Object(map) => {
                for (key, child) in map.iter_mut() {
                    match (key.as_str(), child) {
                        ("content_text", Value::String(text)) => {
                            if records {
                                *text = self.view_text(text);
                            }
                        }
                        ("content_base64", _) => {}
                        (_, child) => self.view(child, records),
                    }
                }
            }
            _ => {}
        }
    }

    /// Alias full CIDs occurring inside runtime-produced text.
    pub fn view_text(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(start) = rest.find("b3:") {
            let candidate = rest.get(start..start + 67);
            match candidate.and_then(full_cid) {
                Some(cid)
                    if !rest[start + 67..]
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_hexdigit()) =>
                {
                    out.push_str(&rest[..start]);
                    out.push_str(&self.alias(cid));
                    rest = &rest[start + 67..];
                }
                _ => {
                    out.push_str(&rest[..start + 3]);
                    rest = &rest[start + 3..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Resolve `#n` in reference-shaped objects (`kind` and `cid` only) to the
    /// full CID. Unknown aliases are left as they are and fail validation.
    pub fn resolve(&self, value: &mut Value) {
        match value {
            Value::Object(map) if map.len() == 2 && map.contains_key("kind") => {
                if let Some(Value::String(cid)) = map.get_mut("cid")
                    && let Some(position) = cid
                        .strip_prefix('#')
                        .and_then(|n| n.parse::<usize>().ok())
                        .and_then(|n| n.checked_sub(1))
                    && let Some(full) = self.cids.get(position)
                {
                    *cid = full.to_string();
                }
            }
            Value::Object(map) => {
                for child in map.values_mut() {
                    self.resolve(child);
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.resolve(item);
                }
            }
            _ => {}
        }
    }

    /// Resolve a raw model response. Non-JSON bytes are returned unchanged so
    /// the normal validation boundary rejects them.
    pub fn resolve_raw(&self, raw: &[u8]) -> Vec<u8> {
        match serde_json::from_slice::<Value>(raw) {
            Ok(mut value) => {
                self.resolve(&mut value);
                serde_json::to_vec(&value).unwrap_or_else(|_| raw.to_vec())
            }
            Err(_) => raw.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cid(byte: u8) -> Cid {
        Cid([byte; 32])
    }

    #[test]
    fn aliases_are_bijective_stable_and_resolve_only_references() {
        let mut aliases = Aliases::default();
        let (a, b) = (cid(1), cid(2));
        assert_eq!(aliases.alias(a), "#1");
        assert_eq!(aliases.alias(b), "#2");
        assert_eq!(aliases.alias(a), "#1");
        assert_eq!(aliases.table(), &[a, b]);

        let mut rendered = json!({"kind":"TASK","payload":{"inputs":[{"kind":"CLAIM","cid":b.to_string()}]},
            "files":{"f":{"kind":"ARTIFACT","cid":a.to_string()}}});
        aliases.view(&mut rendered, false);
        assert_eq!(rendered["payload"]["inputs"][0]["cid"], "#2");
        assert_eq!(rendered["files"]["f"]["cid"], "#1");

        // Repository and agent content is never rewritten; runtime records are.
        let text = format!("see {a} and {a}0");
        let mut item =
            json!({"reference":{"kind":"ARTIFACT","cid":a.to_string()},"content_text":text});
        aliases.view(&mut item, false);
        assert_eq!(item["content_text"], text);
        aliases.view(&mut item, true);
        assert_eq!(item["content_text"], format!("see #1 and {a}0"));

        // Only reference-shaped objects are resolved, never free text.
        let mut response = json!({"actions":[
            {"tool":"fetch","arguments":{"reference":{"kind":"ARTIFACT","cid":"#2"}}},
            {"tool":"put_rationale","arguments":{"text":"#2"}},
            {"tool":"fetch","arguments":{"reference":{"kind":"ARTIFACT","cid":"#9"}}},
            {"tool":"fetch","arguments":{"reference":{"kind":"ARTIFACT","cid":"@0"}}}]});
        aliases.resolve(&mut response);
        assert_eq!(
            response["actions"][0]["arguments"]["reference"]["cid"],
            b.to_string()
        );
        assert_eq!(response["actions"][1]["arguments"]["text"], "#2");
        assert_eq!(
            response["actions"][2]["arguments"]["reference"]["cid"],
            "#9"
        );
        assert_eq!(
            response["actions"][3]["arguments"]["reference"]["cid"],
            "@0"
        );
        assert_eq!(aliases.resolve_raw(b"not json"), b"not json");
    }
}
