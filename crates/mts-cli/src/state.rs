use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum EnforcementMode {
    Shadow,
    Warn,
    Enforce,
}

impl EnforcementMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "shadow" => Some(Self::Shadow),
            "warn" => Some(Self::Warn),
            "enforce" => Some(Self::Enforce),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shadow => "SHADOW",
            Self::Warn => "WARN",
            Self::Enforce => "ENFORCE",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub schema_version: u32,
    pub mode: EnforcementMode,
    pub profile: String,
    pub artifact_retention_days: u32,
    pub raw_artifacts: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            mode: EnforcementMode::Shadow,
            profile: "balanced".into(),
            artifact_retention_days: 30,
            raw_artifacts: false,
        }
    }
}

pub fn mts_home() -> PathBuf {
    if let Some(path) = env::var_os("MTS_HOME") {
        return PathBuf::from(path);
    }
    let home = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"));
    home.map_or_else(
        || PathBuf::from(".mts"),
        |path| PathBuf::from(path).join(".mts"),
    )
}

pub fn ensure_layout(home: &Path) -> io::Result<()> {
    for directory in [
        "harnesses",
        "profiles",
        "state/locks",
        "artifacts",
        "history",
        "backups",
        "launchers",
        "workers",
        "logs",
    ] {
        fs::create_dir_all(home.join(directory))?;
    }
    if !home.join("config.toml").exists() {
        save_config(home, &Config::default())?;
    }
    Ok(())
}

pub fn load_config(home: &Path) -> Result<Config, String> {
    let path = home.join("config.toml");
    if !path.exists() {
        return Ok(Config::default());
    }
    let text = fs::read_to_string(&path).map_err(|error| format!("MTS_CONFIG_READ: {error}"))?;
    toml::from_str(&text).map_err(|error| format!("MTS_CONFIG_PARSE: {error}"))
}

pub fn save_config(home: &Path, config: &Config) -> io::Result<()> {
    fs::create_dir_all(home)?;
    let text = toml::to_string_pretty(config).map_err(io::Error::other)?;
    atomic_write(&home.join("config.toml"), text.as_bytes())
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no parent"))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    fs::write(&temporary, contents)?;
    // ponytail: std-only replacement has a short missing-file window on Windows;
    // use platform ReplaceFile when crash-level single-file atomicity is measured as necessary.
    if let Err(error) = fs::rename(&temporary, path) {
        if path.exists() {
            fs::remove_file(path)?;
            fs::rename(&temporary, path)?;
        } else {
            return Err(error);
        }
    }
    Ok(())
}

pub struct Store {
    connection: Connection,
}

impl Store {
    pub fn open(home: &Path) -> Result<Self, rusqlite::Error> {
        let path = home.join("state/mts.db");
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS events (
               id TEXT PRIMARY KEY, created_at INTEGER NOT NULL, target_id TEXT NOT NULL,
               session_id TEXT, project_id TEXT, operation TEXT NOT NULL, decision TEXT NOT NULL,
               rule_id TEXT, resource_hash TEXT, protected_bytes INTEGER, avoided_output_bytes INTEGER,
               replacement_output_bytes INTEGER, retry_overhead_bytes INTEGER,
               estimated_net_tokens_saved INTEGER, estimate_method TEXT, confidence TEXT,
               duration_us INTEGER, error_code TEXT
             );
             CREATE TABLE IF NOT EXISTS retry_state (
               intent_key TEXT PRIMARY KEY, target_id TEXT NOT NULL, session_id TEXT NOT NULL,
               state TEXT NOT NULL, attempt_count INTEGER NOT NULL, first_seen_at INTEGER NOT NULL,
               last_seen_at INTEGER NOT NULL, expires_at INTEGER, last_progress_score REAL
             );
             CREATE TABLE IF NOT EXISTS projects (
               id TEXT PRIMARY KEY, root_hash TEXT NOT NULL, display_name TEXT NOT NULL,
               policy_mode TEXT NOT NULL, profile_source TEXT, policy_hash TEXT
             );
             CREATE TABLE IF NOT EXISTS app_installations (
               target_id TEXT PRIMARY KEY, detected_version TEXT, adapter_version TEXT NOT NULL,
               installation_scope TEXT NOT NULL, capability_json TEXT NOT NULL, policy_paths_json TEXT NOT NULL,
               backup_reference TEXT, last_doctor_json TEXT, drift_status TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS approvals (
               nonce_hash TEXT PRIMARY KEY, session_id TEXT NOT NULL, intent_key TEXT NOT NULL,
               operation TEXT NOT NULL, expires_at INTEGER NOT NULL, used INTEGER NOT NULL DEFAULT 0,
               issuer TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS task_runs (
               id TEXT PRIMARY KEY, mts_mode TEXT NOT NULL, success INTEGER, tests_passed INTEGER,
               tool_calls INTEGER, token_total INTEGER, completion_ms INTEGER, retry_amplification REAL
             );
             CREATE TABLE IF NOT EXISTS benchmark_runs (
               id TEXT PRIMARY KEY, fixture_hash TEXT NOT NULL, harness_version TEXT,
               mts_version TEXT NOT NULL, policy_hashes TEXT NOT NULL, baseline_json TEXT NOT NULL,
               protected_json TEXT NOT NULL, comparison_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS raw_artifacts (
               id TEXT PRIMARY KEY, local_path TEXT NOT NULL, content_hash TEXT NOT NULL,
               size INTEGER NOT NULL, redaction_status TEXT NOT NULL, expires_at INTEGER,
               encryption_status TEXT NOT NULL
             );"
        )?;
        Ok(Self { connection })
    }

    pub fn installed_targets(&self) -> Result<Vec<String>, rusqlite::Error> {
        let mut statement = self
            .connection
            .prepare("SELECT target_id FROM app_installations ORDER BY target_id")?;
        statement.query_map([], |row| row.get(0))?.collect()
    }

    pub fn record_installation(
        &self,
        target: &str,
        capabilities: &str,
        policy_paths: &str,
    ) -> Result<(), rusqlite::Error> {
        self.record_installations(&[(
            target.to_string(),
            capabilities.to_string(),
            policy_paths.to_string(),
        )])?;
        Ok(())
    }

    pub fn record_installations(
        &self,
        installations: &[(String, String, String)],
    ) -> Result<(), rusqlite::Error> {
        let transaction = self.connection.unchecked_transaction()?;
        for (target, capabilities, policy_paths) in installations {
            transaction.execute(
                "INSERT INTO app_installations(target_id, adapter_version, installation_scope, capability_json, policy_paths_json, drift_status)
                 VALUES (?1, ?2, 'user', ?3, ?4, 'CLEAN')
                 ON CONFLICT(target_id) DO UPDATE SET adapter_version=excluded.adapter_version,
                 capability_json=excluded.capability_json, policy_paths_json=excluded.policy_paths_json, drift_status='CLEAN'",
                params![target, env!("CARGO_PKG_VERSION"), capabilities, policy_paths],
            )?;
        }
        transaction.commit()
    }

    pub fn remove_installation(&self, target: &str) -> Result<bool, rusqlite::Error> {
        Ok(self
            .connection
            .execute("DELETE FROM app_installations WHERE target_id=?1", [target])?
            > 0)
    }

    pub fn savings(&self) -> Result<(u64, u64, u64, u64, u64), rusqlite::Error> {
        self.connection.query_row(
            "SELECT COALESCE(SUM(protected_bytes),0), COALESCE(SUM(avoided_output_bytes),0),
                    COALESCE(SUM(replacement_output_bytes),0), COALESCE(SUM(retry_overhead_bytes),0),
                    COALESCE(SUM(estimated_net_tokens_saved),0) FROM events",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_event(
        &self,
        target: &str,
        operation: &str,
        decision: &str,
        resource: &str,
        protected_bytes: u64,
        mut avoided_output_bytes: u64,
        replacement_output_bytes: u64,
        retry_overhead_bytes: u64,
    ) -> Result<(), rusqlite::Error> {
        let resource_hash = hex(&Sha256::digest(resource.as_bytes()));
        let already_counted: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM events WHERE resource_hash=?1 AND operation=?2 AND avoided_output_bytes>0)",
            params![resource_hash, operation],
            |row| row.get(0),
        )?;
        if already_counted {
            avoided_output_bytes = 0;
        }
        let net = avoided_output_bytes
            .saturating_sub(replacement_output_bytes)
            .saturating_sub(retry_overhead_bytes);
        let id = format!("{}-{:016x}", unix_time(), rand::random::<u64>());
        self.connection.execute(
            "INSERT INTO events(
               id,created_at,target_id,operation,decision,resource_hash,protected_bytes,
               avoided_output_bytes,replacement_output_bytes,retry_overhead_bytes,
               estimated_net_tokens_saved,estimate_method,confidence
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'deterministic-byte-range-v1','LOW')",
            params![
                id,
                unix_time(),
                target,
                operation,
                decision,
                resource_hash,
                protected_bytes,
                avoided_output_bytes,
                replacement_output_bytes,
                retry_overhead_bytes,
                net / 4
            ],
        )?;
        Ok(())
    }

    pub fn issue_approval(
        &self,
        session: &str,
        intent: &str,
        operation: &str,
    ) -> Result<String, rusqlite::Error> {
        let nonce = hex(&rand::random::<[u8; 32]>());
        let hash = hex(&Sha256::digest(nonce.as_bytes()));
        let now = unix_time();
        self.connection.execute(
            "INSERT INTO approvals(nonce_hash,session_id,intent_key,operation,expires_at,used,issuer)
             VALUES (?1,?2,?3,?4,?5,0,'user')",
            params![hash, session, intent, operation, now + 300],
        )?;
        Ok(nonce)
    }

    pub fn retry_rows(&self) -> Result<Vec<(String, String, u32)>, rusqlite::Error> {
        let mut statement = self.connection.prepare(
            "SELECT intent_key,state,attempt_count FROM retry_state ORDER BY last_seen_at DESC",
        )?;
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
    }

    pub fn record_retry(&self, intent: &str) -> Result<(String, u32), rusqlite::Error> {
        let now = unix_time();
        self.connection.execute(
            "INSERT INTO retry_state(intent_key,target_id,session_id,state,attempt_count,first_seen_at,last_seen_at,expires_at)
             VALUES (?1,'simulation','simulation','BLOCKED_WITH_GUIDANCE',1,?2,?2,?3)
             ON CONFLICT(intent_key) DO UPDATE SET
               attempt_count=CASE
                 WHEN retry_state.state='CIRCUIT_OPEN' THEN MAX(retry_state.attempt_count,3)
                 WHEN excluded.last_seen_at-retry_state.last_seen_at>120 THEN 1
                 ELSE retry_state.attempt_count+1
               END,
               state=CASE
                 WHEN retry_state.state='CIRCUIT_OPEN' THEN 'CIRCUIT_OPEN'
                 WHEN excluded.last_seen_at-retry_state.last_seen_at>120 THEN 'BLOCKED_WITH_GUIDANCE'
                 WHEN retry_state.attempt_count+1>=3 THEN 'CIRCUIT_OPEN'
                 WHEN retry_state.attempt_count+1=2 THEN 'SUBSTITUTE_RETURNED'
                 ELSE 'BLOCKED_WITH_GUIDANCE'
               END,
               last_seen_at=excluded.last_seen_at,
               expires_at=excluded.expires_at",
            params![intent, now, now + 120],
        )?;
        self.connection.query_row(
            "SELECT state,attempt_count FROM retry_state WHERE intent_key=?1",
            [intent],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    }

    pub fn set_retry_state(&self, intent: &str, state: &str) -> Result<(), rusqlite::Error> {
        let now = unix_time();
        self.connection.execute(
            "INSERT INTO retry_state(intent_key,target_id,session_id,state,attempt_count,first_seen_at,last_seen_at)
             VALUES (?1,'manual','manual',?2,0,?3,?3)
             ON CONFLICT(intent_key) DO UPDATE SET state=excluded.state,last_seen_at=excluded.last_seen_at",
            params![intent, state, now],
        )?;
        Ok(())
    }

    pub fn clear_retry(&self, intent: &str) -> Result<bool, rusqlite::Error> {
        Ok(self
            .connection
            .execute("DELETE FROM retry_state WHERE intent_key=?1", [intent])?
            > 0)
    }
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_shadow() {
        assert_eq!(Config::default().mode, EnforcementMode::Shadow);
    }

    #[test]
    fn store_schema_and_approval_work() {
        let connection = Connection::open_in_memory().unwrap();
        drop(connection);
        let root = env::temp_dir().join(format!("mts-store-{}", rand::random::<u64>()));
        let store = Store::open(&root).unwrap();
        let nonce = store.issue_approval("session", "intent", "read").unwrap();
        assert_eq!(nonce.len(), 64);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manual_retry_lock_survives_the_next_attempt() {
        let root = env::temp_dir().join(format!("mts-lock-{}", rand::random::<u64>()));
        let store = Store::open(&root).unwrap();
        store.set_retry_state("intent", "CIRCUIT_OPEN").unwrap();
        let (state, attempts) = store.record_retry("intent").unwrap();
        assert_eq!(state, "CIRCUIT_OPEN");
        assert!(attempts >= 3);
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
