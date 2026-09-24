//! Bounded repository reads before workers start. This is snapshot acquisition,
//! not an OS sandbox or an atomic filesystem snapshot of concurrent writers.
use crate::{
    Result,
    views::{Tree, validate_tree},
};
use myr_core::{invalid, validate_relative_path};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadPolicy {
    pub excluded: Vec<String>,
    pub max_entries: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
    pub max_depth: usize,
}
impl Default for ReadPolicy {
    fn default() -> Self {
        Self {
            excluded: vec![".git".into(), ".myr".into(), "target".into()],
            max_entries: 100000,
            max_file_bytes: 16 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
            max_depth: 128,
        }
    }
}

fn plain(metadata: &fs::Metadata) -> Result<()> {
    let link = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let link = {
        use std::os::windows::fs::MetadataExt;
        link || metadata.file_attributes() & 0x400 != 0
    };
    if link {
        return Err(invalid("snapshot refuses symbolic links and reparse points").into());
    }
    Ok(())
}

pub fn read(root: &Path, policy: &ReadPolicy) -> Result<Tree> {
    if policy.max_entries == 0
        || policy.max_file_bytes == 0
        || policy.max_total_bytes == 0
        || policy.max_depth == 0
    {
        return Err(invalid("snapshot limits must be positive").into());
    }
    for path in &policy.excluded {
        validate_relative_path(path.trim_end_matches('/'))?;
    }
    let metadata = fs::symlink_metadata(root)?;
    plain(&metadata)?;
    if !metadata.is_dir() {
        return Err(invalid("snapshot root must be a directory").into());
    }
    let root = root.canonicalize()?;
    let mut reader = Reader {
        policy,
        tree: Tree::new(),
        entries: 0,
        bytes: 0,
    };
    reader.walk(&root, "", 0)?;
    validate_tree(&reader.tree)?;
    Ok(reader.tree)
}

struct Reader<'a> {
    policy: &'a ReadPolicy,
    tree: Tree,
    entries: usize,
    bytes: u64,
}
impl Reader<'_> {
    fn walk(&mut self, root: &Path, relative: &str, depth: usize) -> Result<()> {
        if depth > self.policy.max_depth {
            return Err(invalid("snapshot directory depth limit exceeded").into());
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(root.join(relative))? {
            self.entries += 1;
            if self.entries > self.policy.max_entries {
                return Err(invalid("snapshot entry limit exceeded").into());
            }
            entries.push(entry?);
        }
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("snapshot path is not UTF-8"))?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            validate_relative_path(&path)?;
            let portable_path = path.to_lowercase();
            if self.policy.excluded.iter().any(|p| {
                let prefix = p.trim_end_matches('/').to_lowercase();
                portable_path == prefix || portable_path.starts_with(&format!("{prefix}/"))
            }) {
                continue;
            }
            let location = root.join(&path);
            let before = fs::symlink_metadata(&location)?;
            plain(&before)?;
            if before.is_dir() {
                self.walk(root, &path, depth + 1)?;
            } else if before.is_file() {
                let limit = self
                    .policy
                    .max_file_bytes
                    .min(self.policy.max_total_bytes.saturating_sub(self.bytes));
                if before.len() > limit {
                    return Err(invalid("snapshot byte limit exceeded").into());
                }
                let mut bytes = Vec::new();
                fs::File::open(&location)?
                    .take(limit.saturating_add(1))
                    .read_to_end(&mut bytes)?;
                let after = fs::symlink_metadata(&location)?;
                plain(&after)?;
                if bytes.len() as u64 > limit
                    || before.len() != bytes.len() as u64
                    || before.len() != after.len()
                    || before.modified()? != after.modified()?
                {
                    return Err(
                        invalid("snapshot file changed during read or exceeded limits").into(),
                    );
                }
                self.bytes += bytes.len() as u64;
                self.tree.insert(path, bytes);
            } else {
                return Err(invalid("snapshot refuses non-regular files").into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deterministic_opaque_snapshot_with_explicit_exclusions_and_limits() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::create_dir(dir.path().join("TARGET")).unwrap();
        fs::write(dir.path().join("src/a.bin"), b"\0\xff\r\n").unwrap();
        fs::write(dir.path().join("TARGET/ignored"), b"build data").unwrap();
        let policy = ReadPolicy::default();
        let tree = read(dir.path(), &policy).unwrap();
        assert_eq!(
            tree,
            Tree::from([("src/a.bin".into(), b"\0\xff\r\n".to_vec())])
        );
        assert_eq!(tree, read(dir.path(), &policy).unwrap());
        let mut limited = policy.clone();
        limited.max_file_bytes = 3;
        assert!(read(dir.path(), &limited).is_err());
        limited = policy.clone();
        limited.max_entries = 1;
        assert!(read(dir.path(), &limited).is_err());
        limited = policy;
        limited.excluded = vec!["../outside".into()];
        assert!(read(dir.path(), &limited).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file"), b"data").unwrap();
        std::os::unix::fs::symlink("file", dir.path().join("alias")).unwrap();
        assert!(read(dir.path(), &ReadPolicy::default()).is_err());
    }
}
