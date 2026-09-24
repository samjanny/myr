//! Exact baseline reconstruction. No fuzzy patching, external commands, or writes.
use crate::{
    Result,
    views::{Tree, validate_tree},
};
use myr_cas::Store;
use myr_core::{Delta, DeltaCodec, Kind, Object, ObjectRef, Scope, invalid};
use myr_graph::Graph;
use std::collections::{BTreeMap, BTreeSet};

/// Load the exact referenced snapshot and reject inactive dependencies before
/// rebuilding. The caller supplies scope and policy from the sealed goal, never
/// from agent output. The result is a read-time view, not a persistent verdict.
pub fn reconstruct_live(
    graph: &Graph,
    scope: &Scope,
    delta_refs: &[ObjectRef],
    writable: &[String],
    protected: &[String],
) -> Result<Tree> {
    scope.validate()?;
    let require_live = |reference| -> Result<()> {
        if !graph.live(reference)? {
            return Err(invalid("reconstruction dependency is absent or inactive").into());
        }
        Ok(())
    };
    require_live(scope.goal)?;
    require_live(scope.snapshot)?;
    let bytes = graph.cas().get(scope.snapshot)?;
    let files: BTreeMap<String, ObjectRef> = serde_json::from_slice(&bytes)?;
    // Snapshot manifests use the exact encoding emitted by import_worker.
    // Round-trip comparison also rejects duplicate keys and ambiguous encodings.
    if serde_json::to_vec(&files)? != bytes {
        return Err(invalid("snapshot manifest is not in the runtime encoding").into());
    }
    let mut baseline = Tree::new();
    for (path, reference) in files {
        reference.require(Kind::Artifact)?;
        require_live(reference)?;
        baseline.insert(path, graph.cas().get(reference)?);
    }
    validate_tree(&baseline)?;
    let mut deltas = Vec::new();
    for reference in delta_refs {
        reference.require(Kind::Delta)?;
        require_live(*reference)?;
        let Object::Delta(delta) = graph.get(*reference)? else {
            return Err(invalid("reconstruction reference is not a DELTA").into());
        };
        for dependency in [delta.base, delta.patch, delta.result]
            .into_iter()
            .chain(delta.assumptions.iter().copied())
        {
            require_live(dependency)?;
        }
        deltas.push(delta);
    }
    reconstruct(graph.cas(), &baseline, scope, &deltas, writable, protected)
}

/// Reconstruct from one sealed baseline. The caller must establish that DELTAs
/// and their assumptions are live in the graph before invoking this function.
pub fn reconstruct(
    cas: &Store,
    baseline: &Tree,
    scope: &Scope,
    deltas: &[Delta],
    writable: &[String],
    protected: &[String],
) -> Result<Tree> {
    scope.validate()?;
    validate_tree(baseline)?;
    let mut seen = BTreeSet::new();
    let mut result = baseline.clone();
    for delta in deltas {
        Object::Delta(delta.clone()).validate()?;
        if &delta.scope != scope || !seen.insert(&delta.path) {
            return Err(invalid("DELTA scope mismatch or competing edits to one path").into());
        }
        let matches = |prefix: &String| {
            let prefix = prefix.trim_end_matches('/');
            delta.path == prefix || delta.path.starts_with(&format!("{prefix}/"))
        };
        if protected.iter().any(matches) || !writable.iter().any(matches) {
            return Err(invalid("DELTA path is outside write capabilities or protected").into());
        }
        let base = baseline
            .get(&delta.path)
            .ok_or_else(|| invalid("DELTA path absent from baseline"))?;
        if myr_wire::artifact_cid(base) != delta.base.cid || cas.get(delta.base)? != *base {
            return Err(invalid("DELTA base does not match sealed baseline").into());
        }
        let patch = cas.get(delta.patch)?;
        let bytes = match delta.codec {
            DeltaCodec::Replacement => patch,
            DeltaCodec::UnifiedDiff => apply_unified(base, &patch, &delta.path)?,
        };
        if myr_wire::artifact_cid(&bytes) != delta.result.cid || cas.get(delta.result)? != bytes {
            return Err(
                invalid("DELTA reconstructed result does not match declared artifact").into(),
            );
        }
        result.insert(delta.path.clone(), bytes);
    }
    Ok(result)
}

fn range(text: &str) -> Result<(usize, usize)> {
    let (start, count) = text.split_once(',').unwrap_or((text, "1"));
    let start: usize = start.parse().map_err(|_| invalid("invalid hunk start"))?;
    let count: usize = count.parse().map_err(|_| invalid("invalid hunk count"))?;
    let offset = if count == 0 {
        start
    } else {
        start
            .checked_sub(1)
            .ok_or_else(|| invalid("nonempty hunk starts at zero"))?
    };
    Ok((offset, count))
}

/// Single-file unified diff with exact paths, ranges, and context. File bytes
/// remain opaque, including CRLF and non-UTF-8 content. Only headers are UTF-8.
pub fn apply_unified(base: &[u8], patch: &[u8], path: &str) -> Result<Vec<u8>> {
    myr_core::validate_relative_path(path)?;
    let lines: Vec<&[u8]> = patch.split_inclusive(|b| *b == b'\n').collect();
    let header = |index: usize, marker: &str, side: &str| -> Result<()> {
        let line = lines
            .get(index)
            .ok_or_else(|| invalid("missing diff file header"))?;
        let line =
            std::str::from_utf8(line).map_err(|_| invalid("invalid diff header encoding"))?;
        let name = line
            .strip_prefix(marker)
            .and_then(|s| s.strip_suffix('\n'))
            .ok_or_else(|| invalid("invalid diff file header"))?;
        let name = name.split('\t').next().unwrap_or(name);
        if name != path && name != format!("{side}/{path}") {
            return Err(invalid("diff file header does not match DELTA path").into());
        }
        Ok(())
    };
    header(0, "--- ", "a")?;
    header(1, "+++ ", "b")?;
    let old: Vec<&[u8]> = base.split_inclusive(|b| *b == b'\n').collect();
    let mut output = Vec::new();
    let mut cursor = 0;
    let mut new_lines = 0;
    let mut i = 2;
    let mut hunks = 0;
    while i < lines.len() {
        let text = std::str::from_utf8(lines[i]).map_err(|_| invalid("invalid hunk header"))?;
        let body = text
            .strip_prefix("@@ -")
            .and_then(|s| s.split_once(" @@"))
            .map(|p| p.0)
            .ok_or_else(|| invalid("missing hunk header"))?;
        let (left, right) = body
            .split_once(" +")
            .ok_or_else(|| invalid("invalid hunk ranges"))?;
        let (offset, count) = range(left)?;
        let (new_offset, new_count) = range(right)?;
        if offset < cursor || offset > old.len() || new_offset != new_lines + offset - cursor {
            return Err(invalid("overlapping or inconsistent hunk offsets").into());
        }
        if offset > cursor && !output.is_empty() && !output.ends_with(b"\n") {
            return Err(invalid("unchanged content follows a missing final newline").into());
        }
        for line in &old[cursor..offset] {
            output.extend_from_slice(line);
        }
        new_lines += offset - cursor;
        cursor = offset;
        i += 1;
        let mut consumed = 0;
        let mut produced = 0;
        while i < lines.len() && !lines[i].starts_with(b"@@ ") {
            let line = lines[i];
            let (marker, content) = line
                .split_first()
                .ok_or_else(|| invalid("empty diff line"))?;
            if !matches!(marker, b' ' | b'-' | b'+') || !content.ends_with(b"\n") {
                return Err(invalid("invalid diff body line").into());
            }
            i += 1;
            let mut content = content;
            if lines.get(i) == Some(&b"\\ No newline at end of file\n".as_slice()) {
                content = &content[..content.len() - 1];
                i += 1;
            }
            if *marker != b'+' {
                if old.get(cursor).copied() != Some(content) {
                    return Err(
                        invalid("diff context or removed bytes do not match baseline").into(),
                    );
                }
                cursor += 1;
                consumed += 1;
            }
            if *marker != b'-' {
                if !output.is_empty() && !output.ends_with(b"\n") {
                    return Err(invalid("content follows a missing final newline").into());
                }
                output.extend_from_slice(content);
                produced += 1;
            }
        }
        if consumed != count || produced != new_count || (count == 0 && new_count == 0) {
            return Err(invalid("diff hunk line counts do not match header").into());
        }
        new_lines += produced;
        hunks += 1;
    }
    if hunks == 0 || (cursor < old.len() && !output.is_empty() && !output.ends_with(b"\n")) {
        return Err(invalid("empty diff or misplaced missing newline marker").into());
    }
    for line in &old[cursor..] {
        output.extend_from_slice(line);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_hunks_preserve_opaque_bytes_and_final_newlines() {
        assert_eq!(apply_unified(b"a\r\n\xff\nlast", b"--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\r\n-\xff\n+\xfe\n last\n\\ No newline at end of file\n", "f").unwrap(), b"a\r\n\xfe\nlast");
        assert_eq!(
            apply_unified(
                b"a\nb\nc\n",
                b"--- f\n+++ f\n@@ -0,0 +1 @@\n+x\n@@ -3 +4 @@\n-c\n+z\n",
                "f"
            )
            .unwrap(),
            b"x\na\nb\nz\n"
        );
        assert_eq!(
            apply_unified(b"a\n", b"--- f\n+++ f\n@@ -1 +0,0 @@\n-a\n", "f").unwrap(),
            b""
        );
    }
    #[test]
    fn rejects_fuzzy_context_wrong_paths_counts_and_offsets() {
        for patch in [
            "--- f\n+++ other\n@@ -1 +1 @@\n-a\n+b\n",
            "--- f\n+++ f\n@@ -1 +1 @@\n-x\n+b\n",
            "--- f\n+++ f\n@@ -1,2 +1 @@\n-a\n+b\n",
            "--- f\n+++ f\n@@ -1 +2 @@\n-a\n+b\n",
            "--- f\n+++ f\n@@ -1 +1 @@\n-a\n+b\n@@ -1 +1 @@\n-a\n+c\n",
            "--- f\n+++ f\n@@ -1 +1,2 @@\n-a\n+b\n\\ No newline at end of file\n+c\n",
        ] {
            assert!(
                apply_unified(b"a\n", patch.as_bytes(), "f").is_err(),
                "{patch}"
            );
        }
    }
    #[test]
    fn reconstruction_checks_scope_capabilities_base_and_result_without_mutation() {
        let temp = tempfile::tempdir().unwrap();
        let cas = Store::open(temp.path()).unwrap();
        let base = cas.put_artifact(b"old\n").unwrap();
        let result = cas.put_artifact(b"new\n").unwrap();
        let scope = Scope {
            goal: cas.put_goal(b"goal").unwrap(),
            snapshot: cas.put_artifact(b"snapshot").unwrap(),
        };
        let baseline = Tree::from([("f".into(), b"old\n".to_vec())]);
        let mut d = Delta {
            scope: scope.clone(),
            path: "f".into(),
            base,
            patch: result,
            result,
            codec: DeltaCodec::Replacement,
            assumptions: vec![],
        };
        let run = |d: &Delta, allowed: &[String], protected: &[String]| {
            reconstruct(
                &cas,
                &baseline,
                &scope,
                std::slice::from_ref(d),
                allowed,
                protected,
            )
        };
        assert_eq!(run(&d, &["f".into()], &[]).unwrap()["f"], b"new\n");
        assert!(run(&d, &[], &[]).is_err());
        assert!(run(&d, &["f".into()], &["f".into()]).is_err());
        d.codec = DeltaCodec::UnifiedDiff;
        d.patch = cas
            .put_artifact(b"--- f\n+++ f\n@@ -1 +1 @@\n-old\n+new\n")
            .unwrap();
        assert_eq!(run(&d, &["f".into()], &[]).unwrap()["f"], b"new\n");
        d.result = base;
        assert!(run(&d, &["f".into()], &[]).is_err());
        d.result = result;
        d.base = result;
        assert!(run(&d, &["f".into()], &[]).is_err());
        d.base = base;
        d.scope.snapshot = result;
        assert!(run(&d, &["f".into()], &[]).is_err());
        d.scope = scope.clone();
        assert!(reconstruct(&cas, &baseline, &scope, &[d.clone(), d], &["f".into()], &[]).is_err());
        assert_eq!(baseline["f"], b"old\n");
    }
}
