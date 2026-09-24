//! Harness-only twin-view construction. Source bytes and source hashes must
//! never be included in a worker task or registered in the shared CAS.
use crate::Result;
use myr_core::{Cid, ObjectRef, invalid, validate_relative_path};
use myr_graph::Graph;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub type Tree = BTreeMap<String, Vec<u8>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewEdit {
    pub path: String,
    /// Offsets in the clean worker file, never offsets in an already edited file.
    pub start: usize,
    pub end: usize,
    pub replacement: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewManifest {
    pub worker_hashes: BTreeMap<String, Cid>,
    pub source_hashes: BTreeMap<String, Cid>,
    pub view_diff: Vec<ViewEdit>,
}

pub(crate) fn validate_tree(tree: &Tree) -> Result<()> {
    let mut portable_names = BTreeSet::new();
    let mut component_names = BTreeMap::new();
    for path in tree.keys() {
        validate_relative_path(path)?;
        let mut exact_prefix = String::new();
        for component in path.split('/') {
            if !exact_prefix.is_empty() {
                exact_prefix.push('/');
            }
            exact_prefix.push_str(component);
            if component_names
                .insert(exact_prefix.to_lowercase(), exact_prefix.clone())
                .is_some_and(|previous| previous != exact_prefix)
            {
                return Err(invalid("case-colliding tree path components").into());
            }
        }
        let portable = path.to_lowercase();
        if !portable_names.insert(portable.clone()) {
            return Err(invalid("case-colliding tree paths").into());
        }
        let mut prefix = String::new();
        let parts: Vec<_> = portable.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            if tree.keys().any(|p| p.to_lowercase() == prefix) {
                return Err(invalid("file/directory tree path collision").into());
            }
        }
    }
    Ok(())
}

pub fn hashes(tree: &Tree) -> BTreeMap<String, Cid> {
    tree.iter()
        .map(|(path, bytes)| (path.clone(), myr_wire::artifact_cid(bytes)))
        .collect()
}

pub fn reconstruct(worker: &Tree, edits: &[ViewEdit]) -> Result<Tree> {
    validate_tree(worker)?;
    let mut grouped: BTreeMap<&str, Vec<&ViewEdit>> = BTreeMap::new();
    for edit in edits {
        validate_relative_path(&edit.path)?;
        let bytes = worker
            .get(&edit.path)
            .ok_or_else(|| invalid("view_diff references absent file"))?;
        if edit.start > edit.end
            || edit.end > bytes.len()
            || (edit.start == edit.end && edit.replacement.is_empty())
        {
            return Err(invalid("invalid or empty view_diff range").into());
        }
        grouped.entry(&edit.path).or_default().push(edit);
    }
    let mut source = worker.clone();
    for (path, mut ranges) in grouped {
        ranges.sort_by_key(|edit| (edit.start, edit.end));
        if ranges
            .windows(2)
            .any(|p| p[0].end > p[1].start || p[0].start == p[1].start)
        {
            return Err(invalid("overlapping or ambiguous view_diff ranges").into());
        }
        let bytes = source.get_mut(path).expect("validated file");
        for edit in ranges.into_iter().rev() {
            bytes.splice(edit.start..edit.end, edit.replacement.iter().copied());
        }
    }
    Ok(source)
}

pub fn verify(worker: &Tree, source: &Tree, manifest: &ViewManifest) -> Result<()> {
    validate_tree(worker)?;
    validate_tree(source)?;
    if hashes(worker) != manifest.worker_hashes || hashes(source) != manifest.source_hashes {
        return Err(invalid("view hash mismatch").into());
    }
    if reconstruct(worker, &manifest.view_diff)? != *source {
        return Err(invalid("differences outside declared view_diff").into());
    }
    Ok(())
}

/// Import only the worker view. The manifest returned to agents contains no
/// source hashes, redaction markers, or indication that another view exists.
pub fn import_worker(
    graph: &mut Graph,
    worker: &Tree,
) -> Result<(ObjectRef, BTreeMap<String, ObjectRef>)> {
    validate_tree(worker)?;
    let mut files = BTreeMap::new();
    for (path, bytes) in worker {
        files.insert(path.clone(), graph.register_artifact(bytes)?);
    }
    let snapshot = graph.register_artifact(&serde_json::to_vec(&files)?)?;
    for reference in files.values() {
        graph.depend(snapshot, *reference)?;
    }
    Ok((snapshot, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn edits_are_on_original_offsets_and_undeclared_changes_fail() {
        let worker = Tree::from([("src/lib.rs".into(), b"abcdef".to_vec())]);
        let edits = vec![
            ViewEdit {
                path: "src/lib.rs".into(),
                start: 1,
                end: 2,
                replacement: b"LONG".to_vec(),
            },
            ViewEdit {
                path: "src/lib.rs".into(),
                start: 4,
                end: 5,
                replacement: b"Q".to_vec(),
            },
        ];
        let source = reconstruct(&worker, &edits).unwrap();
        assert_eq!(source["src/lib.rs"], b"aLONGcdQf");
        let manifest = ViewManifest {
            worker_hashes: hashes(&worker),
            source_hashes: hashes(&source),
            view_diff: edits,
        };
        verify(&worker, &source, &manifest).unwrap();
        let mut tampered = source;
        tampered.get_mut("src/lib.rs").unwrap().push(b'x');
        let mut forged_hash = manifest.clone();
        forged_hash.source_hashes = hashes(&tampered);
        assert!(verify(&worker, &tampered, &forged_hash).is_err());
        let mut overlap = manifest.view_diff.clone();
        overlap[1].start = 1;
        assert!(reconstruct(&worker, &overlap).is_err());
    }
    #[test]
    fn path_aliases_cannot_escape_or_change_tree_shape() {
        for paths in [
            vec!["../secret"],
            vec!["a", "a/b"],
            vec!["A", "a"],
            vec!["a\\b"],
            vec!["NUL.txt"],
        ] {
            let tree = paths.into_iter().map(|p| (p.into(), vec![])).collect();
            assert!(reconstruct(&tree, &[]).is_err());
        }
    }
}
