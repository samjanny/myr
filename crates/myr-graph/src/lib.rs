//! Persistent indexes over verified CAS objects. All graph writes are transactional.
//! The caller is the trusted adapter/compiler/runner, never the model itself.
use myr_cas::Store;
use myr_core::{
    promotion::{self, EvidenceView},
    *,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{collections::BTreeSet, path::Path};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("graph database: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("graph CAS: {0}")]
    Cas(#[from] myr_cas::Error),
    #[error("graph validation: {0}")]
    Validation(#[from] ValidationError),
    #[error("graph JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("object is absent, stale, or inactive: {0}")]
    Unavailable(Cid),
    #[error("writer is not authorized for this object kind")]
    Authority,
}
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Authority {
    Compiler,
    Runtime,
    Worker,
}

pub struct Graph {
    connection: Connection,
    cas: Store,
}

impl Graph {
    pub fn open(path: impl AsRef<Path>, cas: Store) -> Result<Self> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch("PRAGMA foreign_keys=ON;
            PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS nodes (
                cid TEXT PRIMARY KEY, kind INTEGER NOT NULL, active INTEGER NOT NULL DEFAULT 1,
                stale INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS edges (
                src TEXT NOT NULL REFERENCES nodes(cid), dst TEXT NOT NULL REFERENCES nodes(cid),
                relation TEXT NOT NULL, PRIMARY KEY(src,dst,relation));
            CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst);
            CREATE TABLE IF NOT EXISTS claims (
                cid TEXT PRIMARY KEY REFERENCES nodes(cid), atom TEXT NOT NULL,
                scope TEXT NOT NULL, polarity INTEGER NOT NULL);
            CREATE INDEX IF NOT EXISTS claims_atom ON claims(atom,scope);
            CREATE TABLE IF NOT EXISTS evidence (
                cid TEXT PRIMARY KEY REFERENCES nodes(cid), claim TEXT NOT NULL REFERENCES claims(cid));
            CREATE INDEX IF NOT EXISTS evidence_claim ON evidence(claim);
            CREATE TABLE IF NOT EXISTS facts (
                cid TEXT PRIMARY KEY REFERENCES nodes(cid), claim TEXT NOT NULL REFERENCES claims(cid));
            CREATE INDEX IF NOT EXISTS facts_claim ON facts(claim);
            CREATE TABLE IF NOT EXISTS conflicts (
                positive TEXT NOT NULL REFERENCES claims(cid), negative TEXT NOT NULL REFERENCES claims(cid),
                resolved INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(positive,negative));")?;
        Ok(Self { connection, cas })
    }

    pub fn cas(&self) -> &Store {
        &self.cas
    }

    pub fn resolve(&self, cid: Cid) -> Result<ObjectRef> {
        let kind: Option<u8> = self
            .connection
            .query_row(
                "SELECT kind FROM nodes WHERE cid=?1",
                [cid.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        Ok(ObjectRef::new(
            Kind::try_from(kind.ok_or(Error::Unavailable(cid))?)?,
            cid,
        ))
    }

    /// Register compiler/runtime-owned artifacts. Caller must keep source-view
    /// poison out of this store and perform capability checks for agent artifacts.
    pub fn register_artifact(&mut self, bytes: &[u8]) -> Result<ObjectRef> {
        let r = self.cas.put_artifact(bytes)?;
        register(&self.connection, r)?;
        Ok(r)
    }

    /// Publish a runtime artifact and all of its dependency edges atomically.
    /// Failed publication may retain immutable CAS bytes, never a partial graph.
    pub fn register_artifact_with_dependencies(
        &mut self,
        bytes: &[u8],
        dependencies: &[ObjectRef],
    ) -> Result<ObjectRef> {
        let reference = self.cas.put_artifact(bytes)?;
        let tx = self.connection.transaction()?;
        register(&tx, reference)?;
        check_exists(&tx, reference, true)?;
        for dependency in dependencies {
            check_exists(&tx, *dependency, true)?;
            self.cas.get(*dependency)?;
            if *dependency == reference
                || closure(&tx, &[*dependency], true)?.contains(&reference.cid)
            {
                return Err(invalid("dependency cycle").into());
            }
            tx.execute(
                "INSERT OR IGNORE INTO edges VALUES (?1,?2,'depends_on')",
                params![reference.cid.to_string(), dependency.cid.to_string()],
            )?;
            if dependency.kind == Kind::Assumption {
                tx.execute(
                    "INSERT OR IGNORE INTO edges VALUES (?1,?2,'assumes')",
                    params![reference.cid.to_string(), dependency.cid.to_string()],
                )?;
            }
        }
        reevaluate_all(&tx, &self.cas)?;
        tx.commit()?;
        Ok(reference)
    }

    /// The compiler passes an already validated, sealed goal representation.
    pub fn register_goal(&mut self, sealed_bytes: &[u8]) -> Result<ObjectRef> {
        let r = self.cas.put_goal(sealed_bytes)?;
        register(&self.connection, r)?;
        Ok(r)
    }

    pub fn get(&self, r: ObjectRef) -> Result<Object> {
        check_exists(&self.connection, r, false)?;
        Ok(self.cas.get_object(r)?)
    }

    pub fn live(&self, r: ObjectRef) -> Result<bool> {
        match check_exists(&self.connection, r, true) {
            Ok(()) => {
                self.cas.get(r)?;
                Ok(true)
            }
            Err(Error::Unavailable(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub fn insert(&mut self, authority: Authority, object: &Object) -> Result<ObjectRef> {
        Ok(self.insert_batch(&[(authority, object.clone())])?.remove(0))
    }

    /// An adapter action may produce ATOM, CLAIM, and runtime-owned ATTEST.
    /// Commit them together, or leave no semantic object from the action indexed.
    pub fn insert_batch(&mut self, objects: &[(Authority, Object)]) -> Result<Vec<ObjectRef>> {
        let normalized = objects
            .iter()
            .map(|(authority, object)| {
                let object = myr_wire::normalize(object)?;
                match (&object, authority) {
                    (Object::PredicateDef(_), Authority::Compiler) => {}
                    (Object::PredicateDef(_), _) | (Object::Fact(_), _) => {
                        return Err(Error::Authority);
                    }
                    (
                        Object::Evidence(_) | Object::Attest(_) | Object::Task(_),
                        Authority::Worker,
                    ) => {
                        return Err(Error::Authority);
                    }
                    (Object::Assumption(a), Authority::Worker) if a.invalidates.is_some() => {
                        return Err(Error::Authority);
                    }
                    _ => {}
                }
                Ok(object)
            })
            .collect::<Result<Vec<_>>>()?;
        let tx = self.connection.transaction()?;
        let mut refs = Vec::new();
        for object in &normalized {
            refs.push(insert_validated(&tx, &self.cas, object)?);
            if let Object::Assumption(a) = object
                && let Some(old) = a.invalidates
            {
                deactivate(&tx, old.cid)?;
            }
        }
        reevaluate_all(&tx, &self.cas)?;
        tx.commit()?;
        Ok(refs)
    }

    /// Add a runtime-owned dependency; cycles and stale dependencies are rejected.
    pub fn depend(&mut self, dependent: ObjectRef, dependency: ObjectRef) -> Result<()> {
        let tx = self.connection.transaction()?;
        check_exists(&tx, dependent, true)?;
        check_exists(&tx, dependency, true)?;
        if dependent == dependency || closure(&tx, &[dependency], true)?.contains(&dependent.cid) {
            return Err(invalid("dependency cycle").into());
        }
        tx.execute(
            "INSERT OR IGNORE INTO edges VALUES (?1,?2,'depends_on')",
            params![dependent.cid.to_string(), dependency.cid.to_string()],
        )?;
        if dependency.kind == Kind::Assumption {
            tx.execute(
                "INSERT OR IGNORE INTO edges VALUES (?1,?2,'assumes')",
                params![dependent.cid.to_string(), dependency.cid.to_string()],
            )?;
        }
        reevaluate_all(&tx, &self.cas)?;
        tx.commit()?;
        Ok(())
    }

    pub fn active_fact(&self, claim: ObjectRef) -> Result<Option<ObjectRef>> {
        claim.require(Kind::Claim)?;
        let cid: Option<String> = self.connection.query_row(
            "SELECT f.cid FROM facts f JOIN nodes n ON n.cid=f.cid WHERE f.claim=?1 AND n.active=1 AND n.stale=0",
            [claim.cid.to_string()], |row| row.get(0)).optional()?;
        cid.map(|s| Ok(ObjectRef::new(Kind::Fact, s.parse()?)))
            .transpose()
    }

    /// Runtime inspection of dependency provenance, including the root. This is
    /// not agent access authorization; agents must continue using `fetch`.
    /// Historical dependencies remain visible so callers can diagnose staleness.
    pub fn dependencies(&self, root: ObjectRef) -> Result<Vec<ObjectRef>> {
        check_exists(&self.connection, root, false)?;
        closure(&self.connection, &[root], true)?
            .into_iter()
            .map(|cid| self.resolve(cid))
            .collect()
    }

    pub fn unresolved_conflict(&self, claim: ObjectRef) -> Result<bool> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM conflicts WHERE (positive=?1 OR negative=?1) AND resolved=0)", [claim.cid.to_string()], |r| r.get(0))?)
    }

    /// Resolve access through the explicit reference closure, never unrestricted CAS.
    pub fn fetch(&self, roots: &[ObjectRef], requested: ObjectRef) -> Result<Vec<u8>> {
        for root in roots {
            check_exists(&self.connection, *root, true)?;
        }
        if !closure(&self.connection, roots, false)?.contains(&requested.cid) {
            return Err(invalid("CID outside permitted reference closure").into());
        }
        check_exists(&self.connection, requested, true)?;
        Ok(self.cas.get(requested)?)
    }
}

fn register(db: &Connection, r: ObjectRef) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO nodes(cid,kind) VALUES (?1,?2)",
        params![r.cid.to_string(), r.kind as u8],
    )?;
    check_exists(db, r, false)
}

fn check_exists(db: &Connection, r: ObjectRef, live: bool) -> Result<()> {
    let row: Option<(u8, bool, bool)> = db
        .query_row(
            "SELECT kind,active,stale FROM nodes WHERE cid=?1",
            [r.cid.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match row {
        Some((kind, active, stale)) if kind == r.kind as u8 && (!live || (active && !stale)) => {
            Ok(())
        }
        _ => Err(Error::Unavailable(r.cid)),
    }
}

/// Reflection is restricted to the strongly typed object, never arbitrary model
/// JSON. Every reference has exactly the two fields `kind` and `cid`.
fn references(object: &Object) -> Result<Vec<ObjectRef>> {
    fn visit(v: &serde_json::Value, refs: &mut Vec<ObjectRef>) -> Result<()> {
        match v {
            serde_json::Value::Object(m)
                if m.len() == 2 && m.contains_key("kind") && m.contains_key("cid") =>
            {
                refs.push(serde_json::from_value(v.clone())?)
            }
            serde_json::Value::Object(m) => {
                for v in m.values() {
                    visit(v, refs)?;
                }
            }
            serde_json::Value::Array(a) => {
                for v in a {
                    visit(v, refs)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut refs = Vec::new();
    visit(&serde_json::to_value(object)?, &mut refs)?;
    refs.sort();
    refs.dedup();
    Ok(refs)
}

fn insert_validated(db: &Connection, cas: &Store, object: &Object) -> Result<ObjectRef> {
    object.validate()?;
    let refs = references(object)?;
    let invalidates = if let Object::Assumption(a) = object {
        a.invalidates
    } else {
        None
    };
    for r in &refs {
        check_exists(db, *r, Some(*r) != invalidates)?;
        cas.get(*r)?;
    }
    match object {
        Object::Atom(a) => {
            let Object::PredicateDef(p) = cas.get_object(a.predicate)? else {
                return Err(invalid("invalid predicate reference").into());
            };
            a.validate_arguments(&p)?;
        }
        Object::Evidence(e) => {
            let Object::Claim(c) = cas.get_object(e.claim)? else {
                return Err(invalid("evidence requires claim").into());
            };
            if e.scope != c.scope {
                return Err(invalid("evidence scope must equal claim scope").into());
            }
        }
        Object::Assumption(a) => {
            if let Some(old) = a.invalidates {
                let Object::Assumption(previous) = cas.get_object(old)? else {
                    return Err(invalid("invalidation requires assumption").into());
                };
                if previous.scope != a.scope {
                    return Err(invalid("invalidation scope mismatch").into());
                }
            }
        }
        _ => {}
    }
    let r = cas.put_object(object)?;
    register(db, r)?;
    for reference in refs {
        let relation = if Some(reference) == invalidates {
            "invalidates"
        } else {
            "depends_on"
        };
        db.execute(
            "INSERT OR IGNORE INTO edges VALUES (?1,?2,?3)",
            params![r.cid.to_string(), reference.cid.to_string(), relation],
        )?;
    }
    match object {
        Object::Claim(c) => {
            db.execute(
                "INSERT OR IGNORE INTO claims VALUES (?1,?2,?3,?4)",
                params![
                    r.cid.to_string(),
                    c.atom.cid.to_string(),
                    serde_json::to_string(&c.scope)?,
                    c.polarity
                ],
            )?;
            let mut stmt = db.prepare(
                "SELECT cid,polarity FROM claims WHERE atom=?1 AND scope=?2 AND polarity!=?3",
            )?;
            let others = stmt
                .query_map(
                    params![
                        c.atom.cid.to_string(),
                        serde_json::to_string(&c.scope)?,
                        c.polarity
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for (other, _) in others {
                let (pos, neg) = if c.polarity {
                    (r.cid.to_string(), other)
                } else {
                    (other, r.cid.to_string())
                };
                db.execute(
                    "INSERT OR IGNORE INTO conflicts(positive,negative) VALUES (?1,?2)",
                    params![pos, neg],
                )?;
            }
        }
        Object::Evidence(e) => {
            db.execute(
                "INSERT OR IGNORE INTO evidence VALUES (?1,?2)",
                params![r.cid.to_string(), e.claim.cid.to_string()],
            )?;
            let relation = match e.verdict {
                Verdict::Supports => "supports",
                Verdict::Contradicts => "contradicts",
                Verdict::Inconclusive => "inconclusive",
            };
            db.execute(
                "INSERT OR IGNORE INTO edges VALUES (?1,?2,?3)",
                params![r.cid.to_string(), e.claim.cid.to_string(), relation],
            )?;
        }
        Object::Fact(f) => {
            db.execute(
                "INSERT OR IGNORE INTO facts VALUES (?1,?2)",
                params![r.cid.to_string(), f.claim.cid.to_string()],
            )?;
        }
        Object::Attest(a) => {
            db.execute(
                "INSERT OR IGNORE INTO edges VALUES (?1,?2,'attests')",
                params![r.cid.to_string(), a.claim.cid.to_string()],
            )?;
        }
        _ => {}
    }
    Ok(r)
}

fn closure(db: &Connection, roots: &[ObjectRef], dependencies_only: bool) -> Result<BTreeSet<Cid>> {
    let mut seen = BTreeSet::new();
    let mut pending: Vec<Cid> = roots.iter().map(|r| r.cid).collect();
    while let Some(cid) = pending.pop() {
        if !seen.insert(cid) {
            continue;
        }
        let mut stmt =
            db.prepare("SELECT dst FROM edges WHERE src=?1 AND (?2=0 OR relation='depends_on')")?;
        for value in stmt.query_map(params![cid.to_string(), dependencies_only], |row| {
            row.get::<_, String>(0)
        })? {
            pending.push(value?.parse()?);
        }
    }
    Ok(seen)
}

fn deactivate(db: &Connection, cid: Cid) -> Result<()> {
    db.execute("UPDATE nodes SET active=0 WHERE cid=?1", [cid.to_string()])?;
    db.execute("WITH RECURSIVE affected(cid) AS (
        SELECT src FROM edges WHERE dst=?1 AND relation='depends_on'
        UNION SELECT e.src FROM edges e JOIN affected a ON e.dst=a.cid WHERE e.relation='depends_on')
        UPDATE nodes SET stale=1 WHERE cid IN (SELECT cid FROM affected)", [cid.to_string()])?;
    Ok(())
}

fn reevaluate_all(db: &Connection, cas: &Store) -> Result<()> {
    let mut stmt = db.prepare("SELECT cid FROM claims")?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut groups = BTreeSet::new();
    for id in ids {
        let Object::Claim(c) = cas.get_object(ObjectRef::new(Kind::Claim, id.parse()?))? else {
            unreachable!()
        };
        if groups.insert((c.atom.cid, serde_json::to_string(&c.scope)?)) {
            reevaluate_atom(db, cas, &c)?;
        }
    }
    Ok(())
}

fn reevaluate_atom(db: &Connection, cas: &Store, target: &Claim) -> Result<()> {
    let scope = serde_json::to_string(&target.scope)?;
    let mut stmt = db.prepare("SELECT c.cid,n.active,n.stale FROM claims c JOIN nodes n ON c.cid=n.cid WHERE c.atom=?1 AND c.scope=?2 ORDER BY c.cid")?;
    let rows = stmt
        .query_map(params![target.atom.cid.to_string(), scope], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut claims = Vec::new();
    for (id, active, stale) in rows {
        let r = ObjectRef::new(Kind::Claim, id.parse()?);
        let Object::Claim(c) = cas.get_object(r)? else {
            unreachable!()
        };
        claims.push((r, c, active && !stale));
    }
    let mut evidence = Vec::new();
    for (r, c, claim_live) in &claims {
        let mut stmt = db.prepare("SELECT e.cid FROM evidence e JOIN nodes n ON e.cid=n.cid WHERE e.claim=?1 AND n.active=1 AND n.stale=0 ORDER BY e.cid")?;
        let ids = stmt
            .query_map([r.cid.to_string()], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for id in ids {
            let er = ObjectRef::new(Kind::Evidence, id.parse()?);
            let Object::Evidence(e) = cas.get_object(er)? else {
                unreachable!()
            };
            if *claim_live {
                evidence.push((er, e, c));
            }
        }
    }
    let both = claims.iter().any(|(_, c, live)| *live && c.polarity)
        && claims.iter().any(|(_, c, live)| *live && !c.polarity);
    let views: Vec<_> = evidence
        .iter()
        .map(|(_, e, c)| EvidenceView {
            evidence: e,
            claim: c,
            active: true,
        })
        .collect();
    // Deterministic evidence resolves a polarity conflict only when one side is
    // promotable and the other is blocked. Contradictory deterministic results
    // leave both sides blocked and the conflict unresolved.
    let resolved = both
        && claims.iter().any(|(_, c, live)| {
            *live && promotion::evaluate(c, &views, false).is_some_and(|ppm| ppm >= 900_000)
        });
    for (r, _, _) in &claims {
        db.execute(
            "UPDATE conflicts SET resolved=?1 WHERE positive=?2 OR negative=?2",
            params![resolved, r.cid.to_string()],
        )?;
    }
    for (r, c, live) in &claims {
        let score = if *live {
            promotion::evaluate(c, &views, both && !resolved)
        } else {
            None
        };
        let mut old_stmt = db.prepare(
            "SELECT f.cid FROM facts f JOIN nodes n ON f.cid=n.cid WHERE f.claim=?1 AND n.active=1",
        )?;
        let old = old_stmt
            .query_map([r.cid.to_string()], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let new_fact = if let Some(confidence_ppm) = score {
            let roots: Vec<_> = std::iter::once(*r)
                .chain(evidence.iter().map(|(r, _, _)| *r))
                .collect();
            let mut assumptions = Vec::new();
            for cid in closure(db, &roots, true)? {
                let kind: u8 = db.query_row(
                    "SELECT kind FROM nodes WHERE cid=?1",
                    [cid.to_string()],
                    |row| row.get(0),
                )?;
                if kind == Kind::Assumption as u8 {
                    assumptions.push(ObjectRef::new(Kind::Assumption, cid));
                }
            }
            Some(Object::Fact(Fact {
                claim: *r,
                evidence: evidence.iter().map(|(r, _, _)| *r).collect(),
                confidence_ppm,
                policy: "promotion-policy-v0".into(),
                assumptions,
            }))
        } else {
            None
        };
        let new_ref = new_fact
            .as_ref()
            .map(myr_wire::identify)
            .transpose()?
            .map(|(r, _)| r);
        for old in old {
            let cid: Cid = old.parse()?;
            if new_ref.is_none_or(|r| r.cid != cid) {
                deactivate(db, cid)?;
            }
        }
        if let Some(fact) = new_fact {
            let normalized = myr_wire::normalize(&fact)?;
            let r = insert_validated(db, cas, &normalized)?;
            db.execute(
                "UPDATE nodes SET active=1,stale=0 WHERE cid=?1",
                [r.cid.to_string()],
            )?;
        }
    }
    Ok(())
}
