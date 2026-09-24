//! Immutable filesystem content-addressed storage with verified reads.
use myr_core::{Cid, Kind, Object, ObjectRef, ValidationError};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("CAS I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid MW object: {0}")]
    Validation(#[from] ValidationError),
    #[error("CAS integrity failure for {0}")]
    Integrity(Cid),
    #[error("CAS reference kind mismatch")]
    Kind,
    #[error("CAS directories or objects must not be symbolic links")]
    Symlink,
}

#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Error> {
        fs::create_dir_all(root.as_ref())?;
        reject_link(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().canonicalize()?,
        })
    }
    pub fn put_artifact(&self, bytes: &[u8]) -> Result<ObjectRef, Error> {
        let cid = myr_wire::artifact_cid(bytes);
        self.write(cid, bytes)?;
        Ok(ObjectRef::new(Kind::Artifact, cid))
    }
    pub fn put_object(&self, object: &Object) -> Result<ObjectRef, Error> {
        let (reference, bytes) = myr_wire::identify(object)?;
        self.write(reference.cid, &bytes)?;
        Ok(reference)
    }
    /// Goals are sealed canonical bytes in the MW object hash domain. Their
    /// schema is owned by the goal compiler, not the ten message-kind codecs.
    pub fn put_goal(&self, bytes: &[u8]) -> Result<ObjectRef, Error> {
        let cid = myr_wire::message_cid(bytes);
        self.write(cid, bytes)?;
        Ok(ObjectRef::new(Kind::Goal, cid))
    }
    pub fn get(&self, reference: ObjectRef) -> Result<Vec<u8>, Error> {
        let path = self.checked_path(reference.cid)?;
        reject_link(&path)?;
        let bytes = fs::read(path)?;
        let actual = if reference.kind == Kind::Artifact {
            myr_wire::artifact_cid(&bytes)
        } else {
            myr_wire::message_cid(&bytes)
        };
        if actual != reference.cid {
            return Err(Error::Integrity(reference.cid));
        }
        if !matches!(reference.kind, Kind::Artifact | Kind::Goal)
            && myr_wire::decode(&bytes)?.kind() != reference.kind
        {
            return Err(Error::Kind);
        }
        Ok(bytes)
    }
    pub fn get_object(&self, reference: ObjectRef) -> Result<Object, Error> {
        if matches!(reference.kind, Kind::Artifact | Kind::Goal) {
            return Err(Error::Kind);
        }
        Ok(myr_wire::decode(&self.get(reference)?)?)
    }
    /// Existence includes integrity; corruption must not appear as a cache hit.
    pub fn exists(&self, reference: ObjectRef) -> Result<bool, Error> {
        match self.get(reference) {
            Ok(_) => Ok(true),
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
    fn checked_path(&self, cid: Cid) -> Result<PathBuf, Error> {
        reject_link(&self.root)?;
        let hex = hex::encode(cid.0);
        let shard = self.root.join(&hex[..2]);
        match reject_link(&shard) {
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
            result => result?,
        }
        Ok(shard.join(hex))
    }
    fn write(&self, cid: Cid, bytes: &[u8]) -> Result<(), Error> {
        let path = self.checked_path(cid)?;
        let parent = path.parent().expect("CAS path has a shard");
        fs::create_dir_all(parent)?;
        reject_link(parent)?;
        if path.try_exists()? {
            reject_link(&path)?;
            return if fs::read(&path)? == bytes {
                Ok(())
            } else {
                Err(Error::Integrity(cid))
            };
        }
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(bytes)?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(&path) {
            Ok(_) => Ok(()),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                reject_link(&path)?;
                if fs::read(&path)? == bytes {
                    Ok(())
                } else {
                    Err(Error::Integrity(cid))
                }
            }
            Err(e) => Err(Error::Io(e.error)),
        }
    }
}

fn reject_link(path: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::Symlink);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(Error::Symlink);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opaque_roundtrip_corruption_detection_and_no_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let cas = Store::open(dir.path()).unwrap();
        let data = b"\0\xff\r\n";
        let r = cas.put_artifact(data).unwrap();
        assert_eq!(cas.get(r).unwrap(), data);
        assert_eq!(cas.put_artifact(data).unwrap(), r);
        assert!(cas.exists(r).unwrap());
        let missing = ObjectRef::new(Kind::Artifact, Cid([17; 32]));
        assert!(!cas.exists(missing).unwrap());
        fs::write(cas.checked_path(r.cid).unwrap(), b"corrupt").unwrap();
        assert!(matches!(cas.get(r), Err(Error::Integrity(_))));
        assert!(matches!(cas.exists(r), Err(Error::Integrity(_))));
        assert!(matches!(cas.put_artifact(data), Err(Error::Integrity(_))));
    }
    #[test]
    fn concurrent_identical_puts_are_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let cas = Store::open(dir.path()).unwrap();
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let cas = cas.clone();
                std::thread::spawn(move || cas.put_artifact(b"shared").unwrap())
            })
            .collect();
        let refs: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert!(refs.iter().all(|r| *r == refs[0]));
        assert_eq!(cas.get(refs[0]).unwrap(), b"shared");
    }
    #[test]
    fn forged_object_kind_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let cas = Store::open(dir.path()).unwrap();
        let r = cas
            .put_object(&Object::PredicateDef(myr_core::core_predicates().remove(0)))
            .unwrap();
        assert!(matches!(
            cas.get(ObjectRef::new(Kind::Claim, r.cid)),
            Err(Error::Kind)
        ));
        assert!(matches!(
            cas.get(ObjectRef::new(Kind::Artifact, r.cid)),
            Err(Error::Integrity(_))
        ));
    }
}
