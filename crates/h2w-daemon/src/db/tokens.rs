//! API tokens: `h2w_<id>_<secret>`; only the SHA-256 of the secret is stored.

use h2w_api::{Role, TokenInfo};
use rusqlite::{Connection, OptionalExtension, params};

use super::DbResult;
use crate::util::rfc3339;

/// A stored token.
#[derive(Clone, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: String,
    pub secret_hash: [u8; 32],
    pub name: String,
    pub role: Role,
    pub created_at: i64,
    pub created_by: String,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
}

impl std::fmt::Debug for TokenRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("role", &self.role)
            .field("revoked", &self.revoked_at.is_some())
            .finish_non_exhaustive()
    }
}

impl TokenRecord {
    /// Not revoked and not expired at `now`.
    #[must_use]
    pub fn is_active(&self, now: i64) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_none_or(|e| e > now)
    }

    /// The API view (never includes the hash).
    #[must_use]
    pub fn info(&self) -> TokenInfo {
        TokenInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            role: self.role,
            created_at: rfc3339(self.created_at),
            created_by: self.created_by.clone(),
            last_used_at: self.last_used_at.map(rfc3339),
            expires_at: self.expires_at.map(rfc3339),
            revoked: self.revoked_at.is_some(),
        }
    }
}

/// Parse a stored role.
#[must_use]
pub fn parse_role(s: &str) -> Option<Role> {
    match s {
        "viewer" => Some(Role::Viewer),
        "operator" => Some(Role::Operator),
        "admin" => Some(Role::Admin),
        _ => None,
    }
}

const COLUMNS: &str =
    "id, secret_hash, name, role, created_at, created_by, expires_at, revoked_at, last_used_at";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<TokenRecord> {
    let hash: Vec<u8> = r.get(1)?;
    let mut secret_hash = [0u8; 32];
    if hash.len() == 32 {
        secret_hash.copy_from_slice(&hash);
    }
    let role: String = r.get(3)?;
    Ok(TokenRecord {
        id: r.get(0)?,
        secret_hash,
        name: r.get(2)?,
        // The CHECK constraint keeps other values out; fall back to the least privilege.
        role: parse_role(&role).unwrap_or(Role::Viewer),
        created_at: r.get(4)?,
        created_by: r.get(5)?,
        expires_at: r.get(6)?,
        revoked_at: r.get(7)?,
        last_used_at: r.get(8)?,
    })
}

/// Insert a new token.
///
/// # Errors
/// SQLite errors (a duplicate id is a constraint error).
pub fn insert(conn: &Connection, t: &TokenRecord) -> DbResult<()> {
    conn.execute(
        &format!("INSERT INTO tokens ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"),
        params![
            t.id,
            &t.secret_hash[..],
            t.name,
            t.role.as_str(),
            t.created_at,
            t.created_by,
            t.expires_at,
            t.revoked_at,
            t.last_used_at
        ],
    )?;
    Ok(())
}

/// Look a token up by its public id.
///
/// # Errors
/// SQLite errors.
pub fn get(conn: &Connection, id: &str) -> DbResult<Option<TokenRecord>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM tokens WHERE id = ?1"),
            [id],
            from_row,
        )
        .optional()?)
}

/// `true` when a token with this id exists (revoked or not).
///
/// # Errors
/// SQLite errors.
pub fn exists(conn: &Connection, id: &str) -> DbResult<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM tokens WHERE id = ?1", [id], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Every token, newest first.
///
/// # Errors
/// SQLite errors.
pub fn list(conn: &Connection) -> DbResult<Vec<TokenRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM tokens ORDER BY created_at DESC, id"
    ))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Revoke a token and end its sessions. Returns `false` when it does not exist or was
/// already revoked.
///
/// # Errors
/// SQLite errors.
pub fn revoke(conn: &Connection, id: &str, now: i64) -> DbResult<bool> {
    let n = conn.execute(
        "UPDATE tokens SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
        params![id, now],
    )?;
    conn.execute("DELETE FROM sessions WHERE token_id = ?1", [id])?;
    Ok(n == 1)
}

/// Record use, at most once a minute per token (keeps hot paths write-free).
///
/// # Errors
/// SQLite errors.
pub fn touch(conn: &Connection, id: &str, now: i64) -> DbResult<()> {
    conn.execute(
        "UPDATE tokens SET last_used_at = ?2 WHERE id = ?1 AND (last_used_at IS NULL OR last_used_at < ?2 - 60)",
        params![id, now],
    )?;
    Ok(())
}

/// Active (not revoked, not expired) admin tokens.
///
/// # Errors
/// SQLite errors.
pub fn count_active_admins(conn: &Connection, now: i64) -> DbResult<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM tokens WHERE role = 'admin' AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?1)",
        [now],
        |r| r.get(0),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;

    fn rec(id: &str, role: Role) -> TokenRecord {
        TokenRecord {
            id: id.into(),
            secret_hash: [7; 32],
            name: format!("t-{id}"),
            role,
            created_at: 100,
            created_by: "test".into(),
            expires_at: None,
            revoked_at: None,
            last_used_at: None,
        }
    }

    #[test]
    fn insert_get_revoke() {
        let (_d, c) = conn();
        insert(&c, &rec("aaaaaaaa", Role::Admin)).unwrap();
        insert(&c, &rec("bbbbbbbb", Role::Viewer)).unwrap();
        assert!(insert(&c, &rec("aaaaaaaa", Role::Admin)).is_err());
        let t = get(&c, "aaaaaaaa").unwrap().unwrap();
        assert_eq!(t.secret_hash, [7; 32]);
        assert!(t.is_active(200));
        assert_eq!(count_active_admins(&c, 200).unwrap(), 1);
        assert!(revoke(&c, "aaaaaaaa", 300).unwrap());
        assert!(!revoke(&c, "aaaaaaaa", 301).unwrap());
        let t = get(&c, "aaaaaaaa").unwrap().unwrap();
        assert!(!t.is_active(400));
        assert!(t.info().revoked);
        assert_eq!(count_active_admins(&c, 400).unwrap(), 0);
        assert_eq!(list(&c).unwrap().len(), 2);
        assert!(exists(&c, "bbbbbbbb").unwrap());
        assert!(!exists(&c, "cccccccc").unwrap());
    }

    #[test]
    fn expiry_and_touch() {
        let (_d, c) = conn();
        let mut t = rec("cccccccc", Role::Operator);
        t.expires_at = Some(150);
        insert(&c, &t).unwrap();
        let t = get(&c, "cccccccc").unwrap().unwrap();
        assert!(t.is_active(149));
        assert!(!t.is_active(150));
        touch(&c, "cccccccc", 1000).unwrap();
        touch(&c, "cccccccc", 1030).unwrap();
        let t = get(&c, "cccccccc").unwrap().unwrap();
        assert_eq!(t.last_used_at, Some(1000));
        touch(&c, "cccccccc", 1061).unwrap();
        assert_eq!(
            get(&c, "cccccccc").unwrap().unwrap().last_used_at,
            Some(1061)
        );
    }

    #[test]
    fn debug_hides_the_hash() {
        let s = format!("{:?}", rec("dddddddd", Role::Admin));
        assert!(!s.contains("secret_hash"), "{s}");
    }
}
