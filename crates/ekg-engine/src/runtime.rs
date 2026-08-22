use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{CycleId, EngineError};

const MAINTENANCE_LEASE_SECONDS: i64 = 300;

#[derive(Debug, Clone)]
pub(crate) struct MaintenanceLease {
    pub database_id: Uuid,
    pub epoch: u64,
    pub owner: Uuid,
    pub request_digest: String,
    pub expires_at: i64,
}

pub(crate) struct RuntimeStore {
    conn: Connection,
    database_id: Uuid,
}

impl RuntimeStore {
    pub fn open(path: &str) -> Result<Self, EngineError> {
        Self::from_connection(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, EngineError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Self, EngineError> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE IF NOT EXISTS engine_runtime_identity (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 database_id TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS engine_active_cycles (
                 cycle_id TEXT PRIMARY KEY,
                 owner_id TEXT NOT NULL,
                 state TEXT NOT NULL CHECK (state IN ('running', 'pending_teacher')),
                 pending_json TEXT,
                 created_at INTEGER NOT NULL,
                 updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS engine_maintenance (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 epoch INTEGER NOT NULL,
                 owner_id TEXT,
                 request_digest TEXT,
                 expires_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS engine_episode_sagas (
                 episode_id TEXT PRIMARY KEY,
                 episode_json TEXT NOT NULL,
                 cycle_id TEXT,
                 owner_id TEXT,
                 pending_json TEXT,
                 created_at INTEGER NOT NULL,
                 CHECK (
                    (cycle_id IS NULL AND owner_id IS NULL AND pending_json IS NULL)
                    OR
                    (cycle_id IS NOT NULL AND owner_id IS NOT NULL AND pending_json IS NOT NULL)
                 )
             );
             CREATE TABLE IF NOT EXISTS engine_feedback_sagas (
                 feedback_id TEXT PRIMARY KEY,
                 feedback_json TEXT NOT NULL,
                 verifier_identity TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS engine_admin_authority (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 secret_digest TEXT NOT NULL
             );",
        )?;
        let database_id = conn
            .query_row(
                "SELECT database_id FROM engine_runtime_identity WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .map(|value| {
                Uuid::parse_str(&value).map_err(|error| {
                    EngineError::InvalidInput(format!(
                        "invalid durable engine database identity: {error}"
                    ))
                })
            })
            .transpose()?
            .unwrap_or_else(Uuid::new_v4);
        conn.execute(
            "INSERT OR IGNORE INTO engine_runtime_identity (singleton, database_id)
             VALUES (1, ?1)",
            params![database_id.to_string()],
        )?;
        conn.execute(
            "INSERT OR IGNORE INTO engine_maintenance
                (singleton, epoch, owner_id, request_digest, expires_at)
             VALUES (1, 0, NULL, NULL, NULL)",
            [],
        )?;
        Ok(Self { conn, database_id })
    }

    pub fn configure_or_verify_admin(&self, secret: &str) -> Result<(), EngineError> {
        if secret.trim().is_empty() {
            return Err(EngineError::InvalidInput(
                "admin secret must be non-empty".into(),
            ));
        }
        let digest = admin_secret_digest(self.database_id, secret);
        let existing = self
            .conn
            .query_row(
                "SELECT secret_digest FROM engine_admin_authority WHERE singleton = 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        match existing {
            Some(existing) if existing == digest => Ok(()),
            Some(_) => Err(EngineError::InvalidInput(
                "admin secret does not match the durable engine authority".into(),
            )),
            None => {
                self.conn.execute(
                    "INSERT INTO engine_admin_authority (singleton, secret_digest)
                     VALUES (1, ?1)",
                    params![digest],
                )?;
                Ok(())
            }
        }
    }

    pub fn begin_cycle(&mut self, cycle_id: CycleId, owner: Uuid) -> Result<(), EngineError> {
        let now = unix_time();
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        clear_expired_maintenance(&transaction, now)?;
        let maintenance_owner = transaction
            .query_row(
                "SELECT owner_id FROM engine_maintenance
                 WHERE singleton = 1 AND owner_id IS NOT NULL",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if maintenance_owner.is_some() {
            return Err(EngineError::InvalidInput(
                "a database-wide maintenance operation is active".into(),
            ));
        }
        transaction.execute(
            "INSERT INTO engine_active_cycles
                (cycle_id, owner_id, state, pending_json, created_at, updated_at)
             VALUES (?1, ?2, 'running', NULL, ?3, ?3)",
            params![cycle_id.to_string(), owner.to_string(), now],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn save_pending_cycle(
        &self,
        cycle_id: CycleId,
        owner: Uuid,
        pending_json: &str,
    ) -> Result<(), EngineError> {
        let changed = self.conn.execute(
            "UPDATE engine_active_cycles
             SET state = 'pending_teacher', pending_json = ?3, updated_at = ?4
             WHERE cycle_id = ?1 AND owner_id = ?2",
            params![
                cycle_id.to_string(),
                owner.to_string(),
                pending_json,
                unix_time()
            ],
        )?;
        if changed != 1 {
            return Err(EngineError::InvalidInput(format!(
                "cycle {cycle_id} lost its durable runtime registration"
            )));
        }
        Ok(())
    }

    /// Durably records an episode persistence saga. When a teacher
    /// continuation is supplied, staging and the running->pending transition
    /// share one IMMEDIATE transaction, so neither half can survive alone.
    pub fn stage_episode_saga(
        &self,
        episode_id: &str,
        episode_json: &str,
        pending: Option<(CycleId, Uuid, &str)>,
    ) -> Result<(), EngineError> {
        let now = unix_time();
        let transaction = self.conn.unchecked_transaction()?;
        let (cycle_id, owner_id, pending_json) = match pending {
            Some((cycle_id, owner, json)) => (
                Some(cycle_id.to_string()),
                Some(owner.to_string()),
                Some(json.to_owned()),
            ),
            None => (None, None, None),
        };
        transaction.execute(
            "INSERT OR IGNORE INTO engine_episode_sagas
                (episode_id, episode_json, cycle_id, owner_id, pending_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                episode_id,
                episode_json,
                cycle_id,
                owner_id,
                pending_json,
                now
            ],
        )?;
        let stored = transaction.query_row(
            "SELECT episode_json, cycle_id, owner_id, pending_json
             FROM engine_episode_sagas WHERE episode_id = ?1",
            params![episode_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )?;
        if stored
            != (
                episode_json.to_owned(),
                cycle_id.clone(),
                owner_id.clone(),
                pending_json.clone(),
            )
        {
            return Err(EngineError::InvalidInput(format!(
                "episode persistence saga conflict for {episode_id}"
            )));
        }
        if let (Some(cycle_id), Some(owner_id), Some(pending_json)) =
            (cycle_id, owner_id, pending_json)
        {
            let changed = transaction.execute(
                "UPDATE engine_active_cycles
                 SET state = 'pending_teacher', pending_json = ?3, updated_at = ?4
                 WHERE cycle_id = ?1 AND owner_id = ?2 AND state IN ('running', 'pending_teacher')",
                params![cycle_id, owner_id, pending_json, now],
            )?;
            if changed != 1 {
                return Err(EngineError::InvalidInput(
                    "failed-attempt saga lost its durable cycle registration".into(),
                ));
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_episode_sagas(&self) -> Result<Vec<String>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT episode_json FROM engine_episode_sagas ORDER BY created_at, episode_id",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn complete_episode_saga(&self, episode_id: &str) -> Result<(), EngineError> {
        self.conn.execute(
            "DELETE FROM engine_episode_sagas WHERE episode_id = ?1",
            params![episode_id],
        )?;
        Ok(())
    }

    pub fn stage_feedback_saga(
        &self,
        feedback_id: &str,
        feedback_json: &str,
        verifier_identity: &str,
    ) -> Result<(), EngineError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO engine_feedback_sagas
                (feedback_id, feedback_json, verifier_identity, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![feedback_id, feedback_json, verifier_identity, unix_time()],
        )?;
        let stored = self.conn.query_row(
            "SELECT feedback_json, verifier_identity FROM engine_feedback_sagas
             WHERE feedback_id = ?1",
            params![feedback_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        if stored != (feedback_json.to_owned(), verifier_identity.to_owned()) {
            return Err(EngineError::InvalidInput(format!(
                "authenticated feedback persistence saga conflict for {feedback_id}"
            )));
        }
        Ok(())
    }

    pub fn pending_feedback_sagas(&self) -> Result<Vec<(String, String)>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT feedback_json, verifier_identity FROM engine_feedback_sagas
             ORDER BY created_at, feedback_id",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn complete_feedback_saga(&self, feedback_id: &str) -> Result<(), EngineError> {
        self.conn.execute(
            "DELETE FROM engine_feedback_sagas WHERE feedback_id = ?1",
            params![feedback_id],
        )?;
        Ok(())
    }

    pub fn pending_cycles(&self) -> Result<Vec<(CycleId, String)>, EngineError> {
        let mut statement = self.conn.prepare(
            "SELECT cycle_id, pending_json FROM engine_active_cycles
             WHERE state = 'pending_teacher' ORDER BY created_at, cycle_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut pending = Vec::new();
        for row in rows {
            let (cycle_id, pending_json) = row?;
            pending.push((
                CycleId(Uuid::parse_str(&cycle_id).map_err(|error| {
                    EngineError::InvalidInput(format!(
                        "invalid durable cycle identity {cycle_id}: {error}"
                    ))
                })?),
                pending_json,
            ));
        }
        Ok(pending)
    }

    pub fn claim_pending_cycle(&self, cycle_id: CycleId, owner: Uuid) -> Result<(), EngineError> {
        let changed = self.conn.execute(
            "UPDATE engine_active_cycles SET owner_id = ?2, updated_at = ?3
             WHERE cycle_id = ?1 AND state = 'pending_teacher'",
            params![cycle_id.to_string(), owner.to_string(), unix_time()],
        )?;
        if changed != 1 {
            return Err(EngineError::InvalidInput(format!(
                "pending cycle {cycle_id} is unavailable"
            )));
        }
        Ok(())
    }

    pub fn assert_cycle_owner(&self, cycle_id: CycleId, owner: Uuid) -> Result<(), EngineError> {
        let matches_owner = self
            .conn
            .query_row(
                "SELECT 1 FROM engine_active_cycles
                 WHERE cycle_id = ?1 AND owner_id = ?2 AND state = 'pending_teacher'",
                params![cycle_id.to_string(), owner.to_string()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !matches_owner {
            return Err(EngineError::InvalidInput(format!(
                "pending cycle {cycle_id} is owned by another engine instance or is complete"
            )));
        }
        Ok(())
    }

    pub fn complete_cycle(&self, cycle_id: CycleId) -> Result<(), EngineError> {
        self.conn.execute(
            "DELETE FROM engine_active_cycles WHERE cycle_id = ?1",
            params![cycle_id.to_string()],
        )?;
        Ok(())
    }

    pub fn acquire_maintenance(
        &mut self,
        owner: Uuid,
        request_digest: &str,
    ) -> Result<MaintenanceLease, EngineError> {
        let now = unix_time();
        let expires_at = now.saturating_add(MAINTENANCE_LEASE_SECONDS);
        let transaction = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        clear_expired_maintenance(&transaction, now)?;
        let active_cycles: i64 =
            transaction.query_row("SELECT COUNT(*) FROM engine_active_cycles", [], |row| {
                row.get(0)
            })?;
        if active_cycles != 0 {
            return Err(EngineError::InvalidInput(
                "offline maintenance requires no active or pending cycles".into(),
            ));
        }
        let (epoch, current_owner) = transaction.query_row(
            "SELECT epoch, owner_id FROM engine_maintenance WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, Option<String>>(1)?)),
        )?;
        if current_owner.is_some() {
            return Err(EngineError::InvalidInput(
                "another database-wide maintenance operation is active".into(),
            ));
        }
        let epoch = epoch.saturating_add(1);
        transaction.execute(
            "UPDATE engine_maintenance
             SET epoch = ?1, owner_id = ?2, request_digest = ?3, expires_at = ?4
             WHERE singleton = 1",
            params![epoch, owner.to_string(), request_digest, expires_at],
        )?;
        transaction.commit()?;
        Ok(MaintenanceLease {
            database_id: self.database_id,
            epoch,
            owner,
            request_digest: request_digest.to_owned(),
            expires_at,
        })
    }

    pub fn validate_maintenance(&self, lease: &MaintenanceLease) -> Result<(), EngineError> {
        if lease.database_id != self.database_id || lease.expires_at < unix_time() {
            return Err(EngineError::InvalidInput(
                "offline maintenance lease is expired or belongs to another database".into(),
            ));
        }
        let stored = self.conn.query_row(
            "SELECT epoch, owner_id, request_digest, expires_at
             FROM engine_maintenance WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )?;
        if stored
            != (
                lease.epoch,
                Some(lease.owner.to_string()),
                Some(lease.request_digest.clone()),
                Some(lease.expires_at),
            )
        {
            return Err(EngineError::InvalidInput(
                "offline maintenance lease no longer matches durable state".into(),
            ));
        }
        Ok(())
    }

    pub fn maintenance_for_request(
        &self,
        request_digest: &str,
    ) -> Result<Option<MaintenanceLease>, EngineError> {
        let stored = self
            .conn
            .query_row(
                "SELECT epoch, owner_id, request_digest, expires_at
                 FROM engine_maintenance WHERE singleton = 1 AND owner_id IS NOT NULL",
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((epoch, owner, stored_digest, expires_at)) = stored else {
            return Ok(None);
        };
        if stored_digest != request_digest {
            return Ok(None);
        }
        Ok(Some(MaintenanceLease {
            database_id: self.database_id,
            epoch,
            owner: Uuid::parse_str(&owner).map_err(|error| {
                EngineError::InvalidInput(format!(
                    "invalid durable maintenance owner {owner}: {error}"
                ))
            })?,
            request_digest: stored_digest,
            expires_at,
        }))
    }

    /// Releases only a lease still owned by this Engine instance and bound to
    /// the exact completed request. Expiry is deliberately irrelevant after
    /// durable completion, but another instance's lease must never be cleared
    /// by a stale receipt retry.
    pub fn release_owned_completed_maintenance(
        &self,
        owner: Uuid,
        request_digest: &str,
    ) -> Result<(), EngineError> {
        self.conn.execute(
            "UPDATE engine_maintenance
             SET owner_id = NULL, request_digest = NULL, expires_at = NULL
             WHERE singleton = 1 AND owner_id = ?1 AND request_digest = ?2",
            params![owner.to_string(), request_digest],
        )?;
        Ok(())
    }
}

fn clear_expired_maintenance(
    transaction: &rusqlite::Transaction<'_>,
    now: i64,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "UPDATE engine_maintenance
         SET owner_id = NULL, request_digest = NULL, expires_at = NULL
         WHERE singleton = 1 AND expires_at IS NOT NULL AND expires_at < ?1",
        params![now],
    )?;
    Ok(())
}

fn unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn admin_secret_digest(database_id: Uuid, secret: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"ekg:engine-admin-secret:v1\0");
    digest.update(database_id.as_bytes());
    digest.update(secret.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_a_second_runtime_cannot_erase_a_live_running_cycle() {
        let path =
            std::env::temp_dir().join(format!("ekg-runtime-live-cycle-{}.sqlite", Uuid::new_v4()));
        let path_text = path.to_string_lossy().into_owned();
        let mut first = RuntimeStore::open(&path_text).unwrap();
        let owner = Uuid::new_v4();
        let cycle = CycleId(Uuid::new_v4());
        first.begin_cycle(cycle, owner).unwrap();

        let mut second = RuntimeStore::open(&path_text).unwrap();
        let error = second
            .acquire_maintenance(Uuid::new_v4(), "request-digest")
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("active or pending cycles"));

        first.complete_cycle(cycle).unwrap();
        drop(second);
        drop(first);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn expired_maintenance_can_be_reacquired_and_completed_release_ignores_expiry() {
        let mut runtime = RuntimeStore::in_memory().unwrap();
        let first = runtime
            .acquire_maintenance(Uuid::new_v4(), "staged-request")
            .unwrap();
        runtime
            .conn
            .execute(
                "UPDATE engine_maintenance SET expires_at = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();

        let reacquired = runtime
            .acquire_maintenance(Uuid::new_v4(), "staged-request")
            .unwrap();
        assert!(reacquired.epoch > first.epoch);
        runtime
            .conn
            .execute(
                "UPDATE engine_maintenance SET expires_at = 0 WHERE singleton = 1",
                [],
            )
            .unwrap();
        runtime
            .release_owned_completed_maintenance(reacquired.owner, "staged-request")
            .unwrap();
        assert!(
            runtime
                .maintenance_for_request("staged-request")
                .unwrap()
                .is_none()
        );
    }
}
