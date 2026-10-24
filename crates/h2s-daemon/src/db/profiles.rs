//! Model profiles, stored as JSON (without the response-only `secrets` field).

use h2s_api::ModelProfile;
use rusqlite::{Connection, OptionalExtension, params};

use super::DbResult;

/// Insert or replace a profile.
///
/// # Errors
/// SQLite or JSON errors.
pub fn put(conn: &Connection, p: &ModelProfile, now: i64) -> DbResult<()> {
    let mut stored = p.clone();
    stored.secrets.clear();
    conn.execute(
        "INSERT INTO profiles (name, spec, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(name) DO UPDATE SET spec = excluded.spec, updated_at = excluded.updated_at",
        params![stored.name, serde_json::to_string(&stored)?, now],
    )?;
    Ok(())
}

/// One profile by name.
///
/// # Errors
/// SQLite or JSON errors.
pub fn get(conn: &Connection, name: &str) -> DbResult<Option<ModelProfile>> {
    let spec: Option<String> = conn
        .query_row("SELECT spec FROM profiles WHERE name = ?1", [name], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(match spec {
        Some(s) => Some(serde_json::from_str(&s)?),
        None => None,
    })
}

/// Every profile, by name.
///
/// # Errors
/// SQLite or JSON errors.
pub fn list(conn: &Connection) -> DbResult<Vec<ModelProfile>> {
    let mut stmt = conn.prepare("SELECT spec FROM profiles ORDER BY name")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for s in rows {
        out.push(serde_json::from_str(&s?)?);
    }
    Ok(out)
}

/// Delete; `false` when it did not exist.
///
/// # Errors
/// SQLite errors.
pub fn delete(conn: &Connection, name: &str) -> DbResult<bool> {
    Ok(conn.execute("DELETE FROM profiles WHERE name = ?1", [name])? == 1)
}

/// Secret NAMES a profile references (endpoint keys, Vertex key file, `extra_env`).
#[must_use]
pub fn referenced_secrets(p: &ModelProfile) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &Option<String>| {
        if let Some(s) = s
            && !out.contains(s)
        {
            out.push(s.clone());
        }
    };
    for ep in [&p.endpoint, &p.judge_endpoint, &p.codex_endpoint]
        .into_iter()
        .flatten()
    {
        add(&ep.api_key_secret);
    }
    if let Some(v) = &p.vertex {
        add(&v.credentials_secret);
    }
    for e in &p.extra_env {
        add(&e.secret);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testutil::conn;
    use h2s_api::{Endpoint, EnvVar, ProfileKind, SecretInfo};

    fn profile(name: &str) -> ModelProfile {
        serde_json::from_value(serde_json::json!({
            "name": name, "kind": "openai_compatible", "solver_model": "glm-5"
        }))
        .unwrap()
    }

    #[test]
    fn crud_strips_response_fields() {
        let (_d, c) = conn();
        let mut p = profile("local");
        p.secrets.push(SecretInfo {
            name: "X".into(),
            set: true,
            chars: Some(3),
            updated_at: None,
        });
        put(&c, &p, 1).unwrap();
        let got = get(&c, "local").unwrap().unwrap();
        assert!(got.secrets.is_empty());
        assert_eq!(got.kind, ProfileKind::OpenaiCompatible);
        p.description = "changed".into();
        put(&c, &p, 2).unwrap();
        assert_eq!(list(&c).unwrap().len(), 1);
        assert_eq!(get(&c, "local").unwrap().unwrap().description, "changed");
        assert!(delete(&c, "local").unwrap());
        assert!(!delete(&c, "local").unwrap());
    }

    #[test]
    fn referenced_secret_names_are_unique() {
        let mut p = profile("p");
        p.endpoint = Some(Endpoint {
            base_url: "http://x".into(),
            api_key_secret: Some("KEY".into()),
        });
        p.judge_endpoint = Some(Endpoint {
            base_url: "http://x".into(),
            api_key_secret: Some("KEY".into()),
        });
        p.extra_env.push(EnvVar {
            name: "A".into(),
            value: None,
            secret: Some("OTHER".into()),
        });
        assert_eq!(
            referenced_secrets(&p),
            vec!["KEY".to_owned(), "OTHER".to_owned()]
        );
    }
}
