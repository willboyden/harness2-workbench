//! The audit log: a SHA-256 hash chain. It shows edits by anyone who cannot rewrite the whole
//! chain; whoever can write the data directory can (SECURITY.md).
//!
//! `hash = sha256(prev_hash || canonical JSON of the record without "hash")`, hex encoded.
//! The canonical JSON is built here field by field (keys sorted, absent optionals omitted,
//! no whitespace) so it never depends on serde_json's map ordering features. The first
//! record's `prev_hash` is 64 zeros. Records are append-only; `verify` re-walks the chain
//! and reports the first record whose sequence, link or hash does not match.

use h2w_api::{AuditEvent, AuditOutcome, AuditVerification, Page};
use rusqlite::{Connection, OptionalExtension, params};

use super::DbResult;

/// `prev_hash` of the first record.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Longest `detail` stored (longer text is cut).
pub const DETAIL_MAX: usize = 2000;

/// What to record. Never put secret values or request bodies here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub actor: String,
    pub action: String,
    pub target: String,
    pub outcome: AuditOutcome,
    pub ip: Option<String>,
    pub request_id: Option<String>,
    pub detail: Option<String>,
}

impl AuditEntry {
    /// A new entry without request context.
    #[must_use]
    pub fn new(
        actor: impl Into<String>,
        action: impl Into<String>,
        target: impl Into<String>,
        outcome: AuditOutcome,
    ) -> AuditEntry {
        AuditEntry {
            actor: actor.into(),
            action: action.into(),
            target: target.into(),
            outcome,
            ip: None,
            request_id: None,
            detail: None,
        }
    }

    /// Attach a detail sentence.
    #[must_use]
    pub fn detail(mut self, d: impl Into<String>) -> AuditEntry {
        self.detail = Some(d.into());
        self
    }
}

fn outcome_str(o: AuditOutcome) -> &'static str {
    match o {
        AuditOutcome::Success => "success",
        AuditOutcome::Denied => "denied",
        AuditOutcome::Error => "error",
    }
}

fn parse_outcome(s: &str) -> AuditOutcome {
    match s {
        "success" => AuditOutcome::Success,
        "denied" => AuditOutcome::Denied,
        _ => AuditOutcome::Error,
    }
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_owned())
}

/// Canonical JSON of `ev` without its `hash` (sorted keys, optionals omitted).
#[must_use]
pub fn canonical_json(ev: &AuditEvent) -> String {
    let mut fields: Vec<(&str, String)> = vec![
        ("action", json_str(&ev.action)),
        ("actor", json_str(&ev.actor)),
        ("outcome", json_str(outcome_str(ev.outcome))),
        ("prev_hash", json_str(&ev.prev_hash)),
        ("seq", ev.seq.to_string()),
        ("target", json_str(&ev.target)),
        ("ts", json_str(&ev.ts)),
    ];
    if let Some(d) = &ev.detail {
        fields.push(("detail", json_str(d)));
    }
    if let Some(ip) = &ev.ip {
        fields.push(("ip", json_str(ip)));
    }
    if let Some(r) = &ev.request_id {
        fields.push(("request_id", json_str(r)));
    }
    fields.sort_by(|a, b| a.0.cmp(b.0));
    let body: Vec<String> = fields
        .into_iter()
        .map(|(k, v)| format!("{}:{v}", json_str(k)))
        .collect();
    format!("{{{}}}", body.join(","))
}

/// The chain hash of `ev` (its own `hash` field is ignored).
#[must_use]
pub fn compute_hash(ev: &AuditEvent) -> String {
    let mut data = ev.prev_hash.as_bytes().to_vec();
    data.extend_from_slice(canonical_json(ev).as_bytes());
    crate::util::sha256_hex(&data)
}

const COLUMNS: &str =
    "seq, ts, actor, action, target, outcome, ip, request_id, detail, prev_hash, hash";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEvent> {
    let seq: i64 = r.get(0)?;
    let outcome: String = r.get(5)?;
    Ok(AuditEvent {
        seq: u64::try_from(seq).unwrap_or(0),
        ts: r.get(1)?,
        actor: r.get(2)?,
        action: r.get(3)?,
        target: r.get(4)?,
        outcome: parse_outcome(&outcome),
        ip: r.get(6)?,
        request_id: r.get(7)?,
        detail: r.get(8)?,
        prev_hash: r.get(9)?,
        hash: r.get(10)?,
    })
}

/// Append one record. Call inside an IMMEDIATE transaction ([`super::write_tx`]) so the
/// read of the chain head and the insert cannot interleave with another writer.
///
/// # Errors
/// SQLite errors.
pub fn append(conn: &Connection, e: &AuditEntry) -> DbResult<AuditEvent> {
    let head: Option<(i64, String)> = conn
        .query_row(
            "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (prev_seq, prev_hash) = head.unwrap_or((0, GENESIS.to_owned()));
    let clean =
        |s: &str, max: usize| crate::util::truncate_chars(&s.replace(['\r', '\n'], " "), max);
    let mut ev = AuditEvent {
        seq: u64::try_from(prev_seq + 1).unwrap_or(0),
        ts: crate::util::now_rfc3339(),
        actor: clean(&e.actor, 200),
        action: clean(&e.action, 100),
        target: clean(&e.target, 300),
        outcome: e.outcome,
        ip: e.ip.as_deref().map(|s| clean(s, 64)),
        request_id: e.request_id.as_deref().map(|s| clean(s, 128)),
        detail: e.detail.as_deref().map(|s| clean(s, DETAIL_MAX)),
        prev_hash,
        hash: String::new(),
    };
    ev.hash = compute_hash(&ev);
    conn.execute(
        &format!(
            "INSERT INTO audit ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
        ),
        params![
            prev_seq + 1,
            ev.ts,
            ev.actor,
            ev.action,
            ev.target,
            outcome_str(ev.outcome),
            ev.ip,
            ev.request_id,
            ev.detail,
            ev.prev_hash,
            ev.hash
        ],
    )?;
    Ok(ev)
}

/// Append in its own transaction.
///
/// # Errors
/// SQLite errors.
pub fn append_tx(conn: &mut Connection, e: &AuditEntry) -> DbResult<AuditEvent> {
    super::write_tx(conn, |tx| append(tx, e))
}

/// Newest first; `before` is the exclusive upper bound on `seq`.
///
/// # Errors
/// SQLite errors.
pub fn list(conn: &Connection, limit: u32, before: Option<i64>) -> DbResult<Page<AuditEvent>> {
    let limit = limit.clamp(1, 1000);
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM audit WHERE seq < ?1 ORDER BY seq DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(
        params![before.unwrap_or(i64::MAX), i64::from(limit) + 1],
        from_row,
    )?;
    let mut items: Vec<AuditEvent> = rows.collect::<Result<_, _>>()?;
    let more = items.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    items.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_cursor = if more {
        items
            .last()
            .and_then(|e| i64::try_from(e.seq).ok())
            .map(super::cursor::encode)
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}

/// Re-walk the whole chain.
///
/// # Errors
/// SQLite errors.
pub fn verify(conn: &Connection) -> DbResult<AuditVerification> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM audit ORDER BY seq ASC"))?;
    let mut rows = stmt.query([])?;
    let mut expected_seq: u64 = 1;
    let mut prev_hash = GENESIS.to_owned();
    let mut entries: u64 = 0;
    let mut first_bad = None;
    while let Some(row) = rows.next()? {
        let ev = from_row(row)?;
        entries += 1;
        if first_bad.is_none()
            && (ev.seq != expected_seq || ev.prev_hash != prev_hash || compute_hash(&ev) != ev.hash)
        {
            first_bad = Some(ev.seq);
        }
        expected_seq = ev.seq.saturating_add(1);
        prev_hash = ev.hash;
    }
    Ok(AuditVerification {
        ok: first_bad.is_none(),
        entries,
        first_bad_seq: first_bad,
    })
}

/// `(seq, hash)` of the newest record (an anchor worth logging outside the database).
///
/// # Errors
/// SQLite errors.
pub fn head(conn: &Connection) -> DbResult<Option<(u64, String)>> {
    Ok(conn
        .query_row(
            "SELECT seq, hash FROM audit ORDER BY seq DESC LIMIT 1",
            [],
            |r| {
                let s: i64 = r.get(0)?;
                Ok((u64::try_from(s).unwrap_or(0), r.get(1)?))
            },
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;
    use proptest::prelude::*;

    fn entry(n: usize) -> AuditEntry {
        AuditEntry::new(
            "admin",
            "token.create",
            format!("tok{n}"),
            AuditOutcome::Success,
        )
        .detail("created \"quoted\"\nline")
    }

    #[test]
    fn chain_verifies_and_detects_tampering() {
        let (_d, mut c) = conn();
        for i in 0..5 {
            append_tx(&mut c, &entry(i)).unwrap();
        }
        let v = verify(&c).unwrap();
        assert!(v.ok);
        assert_eq!(v.entries, 5);
        assert_eq!(head(&c).unwrap().unwrap().0, 5);

        // Change a field without fixing the hash.
        c.execute("UPDATE audit SET actor = 'mallory' WHERE seq = 3", [])
            .unwrap();
        let v = verify(&c).unwrap();
        assert!(!v.ok);
        assert_eq!(v.first_bad_seq, Some(3));
    }

    #[test]
    fn deletion_and_recomputed_hashes_are_detected() {
        let (_d, mut c) = conn();
        for i in 0..4 {
            append_tx(&mut c, &entry(i)).unwrap();
        }
        c.execute("DELETE FROM audit WHERE seq = 2", []).unwrap();
        let v = verify(&c).unwrap();
        assert_eq!(v.first_bad_seq, Some(3));

        // Rewriting a record and its own hash still breaks the next link.
        let (_d2, mut c2) = conn();
        for i in 0..4 {
            append_tx(&mut c2, &entry(i)).unwrap();
        }
        let mut ev = list(&c2, 10, None)
            .unwrap()
            .items
            .into_iter()
            .find(|e| e.seq == 2)
            .unwrap();
        ev.actor = "mallory".into();
        let h = compute_hash(&ev);
        c2.execute(
            "UPDATE audit SET actor = ?1, hash = ?2 WHERE seq = 2",
            params![ev.actor, h],
        )
        .unwrap();
        assert_eq!(verify(&c2).unwrap().first_bad_seq, Some(3));
    }

    #[test]
    fn paging_is_newest_first() {
        let (_d, mut c) = conn();
        for i in 0..7 {
            append_tx(&mut c, &entry(i)).unwrap();
        }
        let p1 = list(&c, 3, None).unwrap();
        assert_eq!(
            p1.items.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![7, 6, 5]
        );
        let cur = super::super::cursor::decode(p1.next_cursor.as_deref().unwrap()).unwrap();
        let p2 = list(&c, 3, Some(cur)).unwrap();
        assert_eq!(
            p2.items.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![4, 3, 2]
        );
        let cur = super::super::cursor::decode(p2.next_cursor.as_deref().unwrap()).unwrap();
        let p3 = list(&c, 3, Some(cur)).unwrap();
        assert_eq!(p3.items.len(), 1);
        assert!(p3.next_cursor.is_none());
    }

    #[test]
    fn canonical_json_is_sorted_and_compact() {
        let ev = AuditEvent {
            seq: 1,
            ts: "2026-10-23T00:00:00Z".into(),
            actor: "a".into(),
            action: "x".into(),
            target: "t".into(),
            outcome: AuditOutcome::Denied,
            ip: Some("127.0.0.1".into()),
            request_id: None,
            detail: None,
            prev_hash: GENESIS.into(),
            hash: "ignored".into(),
        };
        assert_eq!(
            canonical_json(&ev),
            format!(
                "{{\"action\":\"x\",\"actor\":\"a\",\"ip\":\"127.0.0.1\",\"outcome\":\"denied\",\"prev_hash\":\"{GENESIS}\",\"seq\":1,\"target\":\"t\",\"ts\":\"2026-10-23T00:00:00Z\"}}"
            )
        );
    }

    proptest! {
        #[test]
        fn hash_depends_on_every_field(actor in ".{0,20}", detail in proptest::option::of(".{0,20}")) {
            let base = AuditEvent {
                seq: 9, ts: "t".into(), actor, action: "a".into(),
                target: "x".into(), outcome: AuditOutcome::Success, ip: None,
                request_id: None, detail, prev_hash: GENESIS.into(), hash: String::new(),
            };
            let mut other = base.clone();
            other.actor.push('!');
            prop_assert_ne!(compute_hash(&base), compute_hash(&other));
            let mut other = base.clone();
            other.seq = 10;
            prop_assert_ne!(compute_hash(&base), compute_hash(&other));
        }
    }
}
