//! Secret values: the database, or `H2W_SECRET_<NAME>` environment variables, which take
//! precedence and never touch disk. Values never leave this module except into a job's
//! child environment, a generated 0600 file, or a redactor.

use std::collections::BTreeMap;
use std::sync::Arc;

use h2w_api::SecretInfo;
use zeroize::Zeroizing;

use crate::config::ENV_SECRET_PREFIX;
use crate::db::{Db, DbResult};
use crate::util::rfc3339;

/// Reads one environment variable (injected so tests need not touch the process env).
pub type SecretEnv = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Secret lookup with environment precedence.
#[derive(Clone)]
pub struct SecretStore {
    db: Db,
    env: SecretEnv,
    /// Names supplied through the environment (scanned once at startup).
    env_names: Arc<Vec<String>>,
}

impl std::fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretStore")
            .field("env_names", &self.env_names)
            .finish_non_exhaustive()
    }
}

/// Names of `H2W_SECRET_*` variables in the process environment (values are not read).
#[must_use]
pub fn scan_process_env() -> Vec<String> {
    let mut names: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| k.into_string().ok())
        .filter_map(|k| k.strip_prefix(ENV_SECRET_PREFIX).map(str::to_owned))
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();
    names
}

impl SecretStore {
    /// A store reading the real process environment.
    #[must_use]
    pub fn from_process_env(db: Db) -> SecretStore {
        SecretStore {
            db,
            env: Arc::new(|n| std::env::var(n).ok()),
            env_names: Arc::new(scan_process_env()),
        }
    }

    /// A store with an injected environment.
    #[must_use]
    pub fn with_env(db: Db, env: SecretEnv, env_names: Vec<String>) -> SecretStore {
        SecretStore {
            db,
            env,
            env_names: Arc::new(env_names),
        }
    }

    fn env_value(&self, name: &str) -> Option<Zeroizing<String>> {
        (self.env)(&format!("{ENV_SECRET_PREFIX}{name}"))
            .filter(|v| !v.is_empty())
            .map(Zeroizing::new)
    }

    /// `true` when the environment supplies `name` (a stored value is then shadowed).
    #[must_use]
    pub fn from_env(&self, name: &str) -> bool {
        self.env_value(name).is_some()
    }

    /// The effective value.
    ///
    /// # Errors
    /// Database errors.
    pub async fn get(&self, name: &str) -> DbResult<Option<Zeroizing<String>>> {
        if let Some(v) = self.env_value(name) {
            return Ok(Some(v));
        }
        let n = name.to_owned();
        self.db.call(move |c| crate::db::secrets::get(c, &n)).await
    }

    /// Effective values of `names` (missing names are absent from the map).
    ///
    /// # Errors
    /// Database errors.
    pub async fn values_for(
        &self,
        names: &[String],
    ) -> DbResult<BTreeMap<String, Zeroizing<String>>> {
        let mut out = BTreeMap::new();
        for n in names {
            if let Some(v) = self.get(n).await? {
                out.insert(n.clone(), v);
            }
        }
        Ok(out)
    }

    /// Every known value (database and environment), for a server-wide redactor.
    ///
    /// # Errors
    /// Database errors.
    pub async fn all_values(&self) -> DbResult<Vec<(String, Zeroizing<String>)>> {
        let mut out: BTreeMap<String, Zeroizing<String>> = self
            .db
            .call(|c| crate::db::secrets::all_values(c))
            .await?
            .into_iter()
            .collect();
        for n in self.env_names.iter() {
            if let Some(v) = self.env_value(n) {
                out.insert(n.clone(), v);
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Metadata for one name.
    ///
    /// # Errors
    /// Database errors.
    pub async fn info(&self, name: &str) -> DbResult<SecretInfo> {
        if let Some(v) = self.env_value(name) {
            return Ok(SecretInfo {
                name: name.to_owned(),
                set: true,
                chars: Some(u32::try_from(v.chars().count()).unwrap_or(u32::MAX)),
                updated_at: None,
            });
        }
        let n = name.to_owned();
        let stored = self
            .db
            .call(move |c| crate::db::secrets::info(c, &n))
            .await?;
        Ok(match stored {
            Some(s) => SecretInfo {
                name: s.name,
                set: true,
                chars: Some(s.chars),
                updated_at: Some(rfc3339(s.updated_at)),
            },
            None => SecretInfo {
                name: name.to_owned(),
                set: false,
                chars: None,
                updated_at: None,
            },
        })
    }

    /// Metadata of every stored or environment secret plus `referenced` names (shown as
    /// unset when nothing provides them), sorted by name.
    ///
    /// # Errors
    /// Database errors.
    pub async fn list(&self, referenced: &[String]) -> DbResult<Vec<SecretInfo>> {
        let stored = self.db.call(|c| crate::db::secrets::list(c)).await?;
        let mut names: Vec<String> = stored.iter().map(|s| s.name.clone()).collect();
        names.extend(self.env_names.iter().cloned());
        names.extend(referenced.iter().cloned());
        names.sort();
        names.dedup();
        let mut out = Vec::with_capacity(names.len());
        for n in names {
            out.push(self.info(&n).await?);
        }
        Ok(out)
    }
}

/// A secret lookup over a snapshot of values (handed to `h2w_core::env::build`).
#[derive(Default)]
pub struct SecretSnapshot(pub BTreeMap<String, Zeroizing<String>>);

impl std::fmt::Debug for SecretSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

impl h2w_core::env::SecretLookup for SecretSnapshot {
    fn secret(&self, name: &str) -> Option<String> {
        self.0.get(name).map(|v| v.as_str().to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn environment_wins_and_values_stay_inside() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("s.db")).unwrap();
        db.call(|c| crate::db::secrets::set(c, "A_KEY", "from-db-value", 5))
            .await
            .unwrap();
        db.call(|c| crate::db::secrets::set(c, "B_KEY", "shadowed-value", 5))
            .await
            .unwrap();
        let env: SecretEnv = Arc::new(|n| match n {
            "H2W_SECRET_B_KEY" => Some("from-env-value!".to_owned()),
            "H2W_SECRET_C_KEY" => Some("env-only-value".to_owned()),
            _ => None,
        });
        let s = SecretStore::with_env(db, env, vec!["B_KEY".into(), "C_KEY".into()]);
        assert_eq!(
            s.get("A_KEY").await.unwrap().unwrap().as_str(),
            "from-db-value"
        );
        assert_eq!(
            s.get("B_KEY").await.unwrap().unwrap().as_str(),
            "from-env-value!"
        );
        assert!(s.get("D_KEY").await.unwrap().is_none());
        assert!(s.from_env("B_KEY"));
        let info = s.info("B_KEY").await.unwrap();
        assert!(info.set && info.updated_at.is_none());
        assert_eq!(info.chars, Some(15));
        let list = s.list(&["Z_NEEDED".into()]).await.unwrap();
        let names: Vec<_> = list.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["A_KEY", "B_KEY", "C_KEY", "Z_NEEDED"]);
        assert!(!list[3].set);
        let all = s.all_values().await.unwrap();
        assert_eq!(all.len(), 3);
        let json = serde_json::to_string(&list).unwrap();
        assert!(!json.contains("from-db-value") && !json.contains("from-env-value"));
        let snap = SecretSnapshot(s.values_for(&["A_KEY".into()]).await.unwrap());
        assert!(!format!("{snap:?}").contains("from-db"));
    }
}
