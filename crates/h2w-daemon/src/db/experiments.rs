//! Experiment metadata. The results themselves stay plain harness2 directories.

use h2w_api::ExperimentKind;
use rusqlite::{Connection, OptionalExtension, params};

use super::DbResult;

/// A stored experiment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExperimentRecord {
    pub name: String,
    pub kind: ExperimentKind,
    pub description: String,
    pub tags: Vec<String>,
    pub profile: Option<String>,
    pub created_at: i64,
    pub created_by: String,
    pub archived: bool,
}

fn kind_str(k: ExperimentKind) -> &'static str {
    match k {
        ExperimentKind::Standard => "standard",
        ExperimentKind::Demo => "demo",
    }
}

fn parse_kind(s: &str) -> ExperimentKind {
    if s == "demo" {
        ExperimentKind::Demo
    } else {
        ExperimentKind::Standard
    }
}

const COLUMNS: &str = "name, kind, description, tags, profile, created_at, created_by, archived";

fn from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ExperimentRecord> {
    let kind: String = r.get(1)?;
    let tags: String = r.get(3)?;
    let archived: i64 = r.get(7)?;
    Ok(ExperimentRecord {
        name: r.get(0)?,
        kind: parse_kind(&kind),
        description: r.get(2)?,
        tags: serde_json::from_str(&tags).unwrap_or_default(),
        profile: r.get(4)?,
        created_at: r.get(5)?,
        created_by: r.get(6)?,
        archived: archived != 0,
    })
}

/// Insert a new experiment (a duplicate name is a constraint error).
///
/// # Errors
/// SQLite or JSON errors.
pub fn insert(conn: &Connection, e: &ExperimentRecord) -> DbResult<()> {
    conn.execute(
        &format!("INSERT INTO experiments ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
        params![
            e.name,
            kind_str(e.kind),
            e.description,
            serde_json::to_string(&e.tags)?,
            e.profile,
            e.created_at,
            e.created_by,
            i64::from(e.archived)
        ],
    )?;
    Ok(())
}

/// One experiment by name.
///
/// # Errors
/// SQLite errors.
pub fn get(conn: &Connection, name: &str) -> DbResult<Option<ExperimentRecord>> {
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM experiments WHERE name = ?1"),
            [name],
            from_row,
        )
        .optional()?)
}

/// All experiments by name; archived ones only when asked.
///
/// # Errors
/// SQLite errors.
pub fn list(conn: &Connection, include_archived: bool) -> DbResult<Vec<ExperimentRecord>> {
    let sql = if include_archived {
        format!("SELECT {COLUMNS} FROM experiments ORDER BY name")
    } else {
        format!("SELECT {COLUMNS} FROM experiments WHERE archived = 0 ORDER BY name")
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Update the editable fields; `None` leaves a field unchanged. Returns the new record.
///
/// # Errors
/// SQLite or JSON errors.
pub fn update(
    conn: &Connection,
    name: &str,
    description: Option<&str>,
    tags: Option<&[String]>,
    archived: Option<bool>,
) -> DbResult<Option<ExperimentRecord>> {
    if let Some(d) = description {
        conn.execute(
            "UPDATE experiments SET description = ?2 WHERE name = ?1",
            params![name, d],
        )?;
    }
    if let Some(t) = tags {
        conn.execute(
            "UPDATE experiments SET tags = ?2 WHERE name = ?1",
            params![name, serde_json::to_string(t)?],
        )?;
    }
    if let Some(a) = archived {
        conn.execute(
            "UPDATE experiments SET archived = ?2 WHERE name = ?1",
            params![name, i64::from(a)],
        )?;
    }
    get(conn, name)
}

/// Pin a profile to an experiment that has none yet. Returns `false` when it already has one.
///
/// # Errors
/// SQLite errors.
pub fn pin_profile(conn: &Connection, name: &str, profile: &str) -> DbResult<bool> {
    let n = conn.execute(
        "UPDATE experiments SET profile = ?2 WHERE name = ?1 AND profile IS NULL",
        params![name, profile],
    )?;
    Ok(n == 1)
}

/// Names of experiments pinned to `profile`.
///
/// # Errors
/// SQLite errors.
pub fn pinning(conn: &Connection, profile: &str) -> DbResult<Vec<String>> {
    let mut stmt = conn.prepare("SELECT name FROM experiments WHERE profile = ?1 ORDER BY name")?;
    let rows = stmt.query_map([profile], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;

    fn rec(name: &str) -> ExperimentRecord {
        ExperimentRecord {
            name: name.into(),
            kind: ExperimentKind::Standard,
            description: "d".into(),
            tags: vec!["a".into()],
            profile: None,
            created_at: 1,
            created_by: "me".into(),
            archived: false,
        }
    }

    #[test]
    fn crud() {
        let (_d, c) = conn();
        insert(&c, &rec("e1")).unwrap();
        let mut demo = rec("e2");
        demo.kind = ExperimentKind::Demo;
        insert(&c, &demo).unwrap();
        assert!(insert(&c, &rec("e1")).is_err());
        assert_eq!(get(&c, "e2").unwrap().unwrap().kind, ExperimentKind::Demo);
        let tags = vec!["x".to_owned(), "y".to_owned()];
        let e = update(&c, "e1", Some("new"), Some(&tags), Some(true))
            .unwrap()
            .unwrap();
        assert_eq!(e.description, "new");
        assert_eq!(e.tags, tags);
        assert!(e.archived);
        assert_eq!(list(&c, false).unwrap().len(), 1);
        assert_eq!(list(&c, true).unwrap().len(), 2);
        assert!(pin_profile(&c, "e1", "p").unwrap());
        assert!(!pin_profile(&c, "e1", "q").unwrap());
        assert_eq!(pinning(&c, "p").unwrap(), vec!["e1".to_owned()]);
        assert!(update(&c, "zz", Some("x"), None, None).unwrap().is_none());
    }
}
