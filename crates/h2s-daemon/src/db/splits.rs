//! Train/test split files created by `split` jobs.

use h2s_api::SplitInfo;
use rusqlite::{Connection, OptionalExtension, params};

use super::DbResult;

/// Record a split.
///
/// # Errors
/// SQLite or JSON errors (a duplicate name is a constraint error).
pub fn insert(conn: &Connection, s: &SplitInfo, now: i64) -> DbResult<()> {
    conn.execute(
        "INSERT INTO splits (name, info, created_at) VALUES (?1, ?2, ?3)",
        params![s.name, serde_json::to_string(s)?, now],
    )?;
    Ok(())
}

/// One split by name.
///
/// # Errors
/// SQLite or JSON errors.
pub fn get(conn: &Connection, name: &str) -> DbResult<Option<SplitInfo>> {
    let info: Option<String> = conn
        .query_row("SELECT info FROM splits WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match info {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    })
}

/// Every split, newest first.
///
/// # Errors
/// SQLite or JSON errors.
pub fn list(conn: &Connection) -> DbResult<Vec<SplitInfo>> {
    let mut stmt = conn.prepare("SELECT info FROM splits ORDER BY created_at DESC, name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for s in rows {
        out.push(serde_json::from_str(&s?)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;
    use h2s_api::Bench;

    #[test]
    fn crud() {
        let (_d, c) = conn();
        let s = SplitInfo {
            name: "half".into(),
            bench: Bench::Lab,
            subset: None,
            seed: 42,
            train_fraction: 0.5,
            train: 3,
            test: 4,
            created_at: "2026-10-23T00:00:00Z".into(),
        };
        insert(&c, &s, 1).unwrap();
        assert!(insert(&c, &s, 2).is_err());
        assert_eq!(get(&c, "half").unwrap().unwrap(), s);
        assert_eq!(list(&c).unwrap().len(), 1);
        assert!(get(&c, "nope").unwrap().is_none());
    }
}
