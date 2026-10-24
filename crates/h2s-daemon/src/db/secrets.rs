//! Secret values. Write-only through the API; the database file is 0600 in a 0700 dir.

use rusqlite::{Connection, OptionalExtension, params};
use zeroize::Zeroizing;

use super::DbResult;

/// Metadata of a stored secret (never the value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSecret {
    pub name: String,
    pub chars: u32,
    pub updated_at: i64,
}

/// Insert or replace a value.
///
/// # Errors
/// SQLite errors.
pub fn set(conn: &Connection, name: &str, value: &str, now: i64) -> DbResult<()> {
    conn.execute(
        "INSERT INTO secrets (name, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(name) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        params![name, value, now],
    )?;
    Ok(())
}

/// The stored value (zeroized when dropped).
///
/// # Errors
/// SQLite errors.
pub fn get(conn: &Connection, name: &str) -> DbResult<Option<Zeroizing<String>>> {
    Ok(conn
        .query_row("SELECT value FROM secrets WHERE name = ?1", [name], |r| {
            r.get::<_, String>(0)
        })
        .optional()?
        .map(Zeroizing::new))
}

/// Every stored value, for building a redactor.
///
/// # Errors
/// SQLite errors.
pub fn all_values(conn: &Connection) -> DbResult<Vec<(String, Zeroizing<String>)>> {
    let mut stmt = conn.prepare("SELECT name, value FROM secrets ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Zeroizing::new(r.get::<_, String>(1)?),
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Metadata of every stored secret.
///
/// # Errors
/// SQLite errors.
pub fn list(conn: &Connection) -> DbResult<Vec<StoredSecret>> {
    let mut stmt =
        conn.prepare("SELECT name, length(value), updated_at FROM secrets ORDER BY name")?;
    let rows = stmt.query_map([], |r| {
        let chars: i64 = r.get(1)?;
        Ok(StoredSecret {
            name: r.get(0)?,
            chars: u32::try_from(chars).unwrap_or(u32::MAX),
            updated_at: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Metadata of one stored secret.
///
/// # Errors
/// SQLite errors.
pub fn info(conn: &Connection, name: &str) -> DbResult<Option<StoredSecret>> {
    Ok(conn
        .query_row(
            "SELECT name, length(value), updated_at FROM secrets WHERE name = ?1",
            [name],
            |r| {
                let chars: i64 = r.get(1)?;
                Ok(StoredSecret {
                    name: r.get(0)?,
                    chars: u32::try_from(chars).unwrap_or(u32::MAX),
                    updated_at: r.get(2)?,
                })
            },
        )
        .optional()?)
}

/// Delete; `false` when it did not exist.
///
/// # Errors
/// SQLite errors.
pub fn delete(conn: &Connection, name: &str) -> DbResult<bool> {
    Ok(conn.execute("DELETE FROM secrets WHERE name = ?1", [name])? == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;

    #[test]
    fn crud() {
        let (_d, c) = conn();
        set(&c, "API_KEY", "héllo", 1).unwrap();
        set(&c, "API_KEY", "hello-world", 2).unwrap();
        assert_eq!(get(&c, "API_KEY").unwrap().unwrap().as_str(), "hello-world");
        let i = info(&c, "API_KEY").unwrap().unwrap();
        assert_eq!(i.chars, 11);
        assert_eq!(i.updated_at, 2);
        assert_eq!(list(&c).unwrap().len(), 1);
        assert_eq!(all_values(&c).unwrap().len(), 1);
        assert!(delete(&c, "API_KEY").unwrap());
        assert!(get(&c, "API_KEY").unwrap().is_none());
    }
}
