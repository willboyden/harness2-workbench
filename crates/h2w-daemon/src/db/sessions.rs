//! Browser sessions: a random id in an HttpOnly cookie; only its SHA-256 is stored.

use rusqlite::{Connection, OptionalExtension, params};

use super::DbResult;
use super::tokens::TokenRecord;

/// Create a session for `token_id`.
///
/// # Errors
/// SQLite errors (unknown token ids violate the foreign key).
pub fn insert(
    conn: &Connection,
    id_hash: &[u8; 32],
    token_id: &str,
    now: i64,
    expires_at: i64,
) -> DbResult<()> {
    conn.execute(
        "INSERT INTO sessions (id_hash, token_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
        params![&id_hash[..], token_id, now, expires_at],
    )?;
    Ok(())
}

/// The token behind a live session (not expired at `now`), whatever the token's state;
/// the caller checks the token is still active.
///
/// # Errors
/// SQLite errors.
pub fn lookup(conn: &Connection, id_hash: &[u8; 32], now: i64) -> DbResult<Option<TokenRecord>> {
    let token_id: Option<String> = conn
        .query_row(
            "SELECT token_id FROM sessions WHERE id_hash = ?1 AND expires_at > ?2",
            params![&id_hash[..], now],
            |r| r.get(0),
        )
        .optional()?;
    match token_id {
        Some(id) => super::tokens::get(conn, &id),
        None => Ok(None),
    }
}

/// End one session.
///
/// # Errors
/// SQLite errors.
pub fn delete(conn: &Connection, id_hash: &[u8; 32]) -> DbResult<()> {
    conn.execute("DELETE FROM sessions WHERE id_hash = ?1", [&id_hash[..]])?;
    Ok(())
}

/// Drop expired sessions; returns how many.
///
/// # Errors
/// SQLite errors.
pub fn purge_expired(conn: &Connection, now: i64) -> DbResult<usize> {
    Ok(conn.execute("DELETE FROM sessions WHERE expires_at <= ?1", [now])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;
    use crate::db::tokens;
    use h2w_api::Role;

    #[test]
    fn sessions_follow_their_token() {
        let (_d, c) = conn();
        tokens::insert(
            &c,
            &TokenRecord {
                id: "tok00001".into(),
                secret_hash: [1; 32],
                name: "n".into(),
                role: Role::Viewer,
                created_at: 0,
                created_by: "t".into(),
                expires_at: None,
                revoked_at: None,
                last_used_at: None,
            },
        )
        .unwrap();
        insert(&c, &[9; 32], "tok00001", 10, 100).unwrap();
        assert!(insert(&c, &[8; 32], "missing0", 10, 100).is_err());
        assert_eq!(lookup(&c, &[9; 32], 50).unwrap().unwrap().id, "tok00001");
        assert!(lookup(&c, &[9; 32], 100).unwrap().is_none());
        assert!(lookup(&c, &[1; 32], 50).unwrap().is_none());
        tokens::revoke(&c, "tok00001", 60).unwrap();
        assert!(lookup(&c, &[9; 32], 50).unwrap().is_none());
        insert(&c, &[7; 32], "tok00001", 10, 20).unwrap();
        assert_eq!(purge_expired(&c, 30).unwrap(), 1);
        insert(&c, &[6; 32], "tok00001", 10, 200).unwrap();
        delete(&c, &[6; 32]).unwrap();
        assert!(lookup(&c, &[6; 32], 50).unwrap().is_none());
    }
}
