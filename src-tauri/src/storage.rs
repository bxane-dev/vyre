use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedRelayManifest {
    pub generation: u64,
    pub signer_key_id: String,
    pub accepted_at: String,
    /// The signed source envelope. Callers must re-verify this before reuse.
    pub envelope_json: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub id: i64,
    pub at: String,
    pub game: String,
    pub mode: String,
    pub before_json: String,
    pub after_json: String,
    pub change: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameSessionRecord {
    pub id: i64,
    pub at: String,
    pub game: String,
    pub pid: u32,
    pub frame_count: usize,
    pub average_fps: f64,
    pub one_percent_low: f64,
    pub point_one_percent_low: f64,
    pub average_frame_time_ms: f64,
    pub p95_frame_time_ms: f64,
    pub frame_time_std_dev_ms: f64,
    pub frame_time_spikes: usize,
    pub dropped_frames: usize,
    pub average_cpu_busy_ms: Option<f64>,
    pub average_gpu_time_ms: Option<f64>,
    pub average_display_latency_ms: Option<f64>,
    pub capture_seconds: u32,
    pub csv_path: String,
    pub analysis_version: u8,
}

#[derive(Clone)]
pub struct JournalEntry {
    pub pid: u32,
    pub started_at: u64,
    pub original_priority: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayJournalPhase {
    Prepared,
    TunnelReady,
    RoutesActive,
    Restoring,
}

impl RelayJournalPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::TunnelReady => "tunnel_ready",
            Self::RoutesActive => "routes_active",
            Self::Restoring => "restoring",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "prepared" => Ok(Self::Prepared),
            "tunnel_ready" => Ok(Self::TunnelReady),
            "routes_active" => Ok(Self::RoutesActive),
            "restoring" => Ok(Self::Restoring),
            _ => Err("Relay restore journal contains an unknown phase.".into()),
        }
    }
}

#[derive(Clone)]
pub struct RelayJournalEntry {
    pub session_id: String,
    pub phase: RelayJournalPhase,
    /// Opaque, DPAPI-protected restore snapshot. The app never parses it.
    pub protected_restore_blob: Vec<u8>,
    pub last_error: Option<String>,
}

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let connection = Connection::open(path).map_err(|err| err.to_string())?;
    connection.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE IF NOT EXISTS custom_games (id INTEGER PRIMARY KEY, name TEXT NOT NULL, exe TEXT NOT NULL UNIQUE);
        CREATE TABLE IF NOT EXISTS game_profiles (exe TEXT PRIMARY KEY, game TEXT NOT NULL, mode TEXT NOT NULL CHECK(mode IN ('Safe','Competitive')));
        CREATE TABLE IF NOT EXISTS app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS sessions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, game TEXT NOT NULL, mode TEXT NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL, change TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS frame_sessions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, game TEXT NOT NULL, pid INTEGER NOT NULL, frame_count INTEGER NOT NULL, average_fps REAL NOT NULL, one_percent_low REAL NOT NULL, point_one_percent_low REAL NOT NULL, average_frame_time_ms REAL NOT NULL DEFAULT 0, p95_frame_time_ms REAL NOT NULL DEFAULT 0, frame_time_std_dev_ms REAL NOT NULL DEFAULT 0, frame_time_spikes INTEGER NOT NULL DEFAULT 0, dropped_frames INTEGER NOT NULL DEFAULT 0, average_cpu_busy_ms REAL, average_gpu_time_ms REAL, average_display_latency_ms REAL, capture_seconds INTEGER NOT NULL, csv_path TEXT NOT NULL, analysis_version INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE IF NOT EXISTS change_journal (pid INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, original_priority INTEGER NOT NULL, at TEXT NOT NULL, reason TEXT NOT NULL, game TEXT NOT NULL, mode TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS relay_manifest_state (id INTEGER PRIMARY KEY CHECK(id = 1), generation INTEGER NOT NULL, signer_key_id TEXT NOT NULL, accepted_at TEXT NOT NULL, envelope_json TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS relay_session_journal (session_id TEXT PRIMARY KEY, phase TEXT NOT NULL CHECK(phase IN ('prepared','tunnel_ready','routes_active','restoring')), protected_restore_blob BLOB NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL, last_error TEXT); ")
        .map_err(|err| err.to_string())?;
    let columns = {
        let mut statement = connection
            .prepare("PRAGMA table_info(frame_sessions)")
            .map_err(|err| err.to_string())?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(|err| err.to_string())?;
        rows.collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(|err| err.to_string())?
    };
    for (name, definition) in [
        ("average_frame_time_ms", "REAL NOT NULL DEFAULT 0"),
        ("p95_frame_time_ms", "REAL NOT NULL DEFAULT 0"),
        ("frame_time_std_dev_ms", "REAL NOT NULL DEFAULT 0"),
        ("frame_time_spikes", "INTEGER NOT NULL DEFAULT 0"),
        ("dropped_frames", "INTEGER NOT NULL DEFAULT 0"),
        ("average_cpu_busy_ms", "REAL"),
        ("average_gpu_time_ms", "REAL"),
        ("average_display_latency_ms", "REAL"),
        ("analysis_version", "INTEGER NOT NULL DEFAULT 0"),
    ] {
        if !columns.contains(name) {
            connection
                .execute_batch(&format!(
                    "ALTER TABLE frame_sessions ADD COLUMN {name} {definition}"
                ))
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(connection)
}

/// Persist the encrypted restore snapshot before any future tunnel or route side effect.
pub fn begin_relay_session(
    db: &Connection,
    session_id: &str,
    protected_restore_blob: &[u8],
) -> Result<(), String> {
    if !valid_relay_session_id(session_id)
        || protected_restore_blob.is_empty()
        || protected_restore_blob.len() > 64 * 1024
    {
        return Err("Relay session journal input is invalid.".into());
    }
    let now = Utc::now().to_rfc3339();
    db.execute(
        "INSERT INTO relay_session_journal(session_id,phase,protected_restore_blob,created_at,updated_at,last_error) VALUES(?1,'prepared',?2,?3,?3,NULL)",
        params![session_id, protected_restore_blob, now],
    ).map_err(|err| err.to_string())?;
    Ok(())
}

/// Enforce the forward lifecycle. The restore phase may be entered from any
/// incomplete phase so partial setup can be rolled back after an error.
pub fn advance_relay_session(
    db: &Connection,
    session_id: &str,
    next: RelayJournalPhase,
) -> Result<(), String> {
    let allowed_previous: &[&str] = match next {
        RelayJournalPhase::TunnelReady => &["prepared"],
        RelayJournalPhase::RoutesActive => &["tunnel_ready"],
        RelayJournalPhase::Restoring => &["prepared", "tunnel_ready", "routes_active", "restoring"],
        RelayJournalPhase::Prepared => {
            return Err("Relay session lifecycle transition is invalid.".into())
        }
    };
    let placeholders = (0..allowed_previous.len())
        .map(|index| format!("?{}", index + 4))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "UPDATE relay_session_journal SET phase=?2, updated_at=?3, last_error=NULL WHERE session_id=?1 AND phase IN ({placeholders})"
    );
    let mut values = vec![
        rusqlite::types::Value::Text(session_id.to_string()),
        rusqlite::types::Value::Text(next.as_str().to_string()),
        rusqlite::types::Value::Text(Utc::now().to_rfc3339()),
    ];
    values.extend(
        allowed_previous
            .iter()
            .map(|phase| rusqlite::types::Value::Text((*phase).to_string())),
    );
    let changed = db
        .execute(&sql, rusqlite::params_from_iter(values))
        .map_err(|err| err.to_string())?;
    if changed != 1 {
        return Err("Relay session lifecycle transition is invalid.".into());
    }
    Ok(())
}

/// Retain the recovery record if restoration failed; do not delete it until
/// the caller has confirmed that the original route state has been restored.
pub fn record_relay_restore_failure(db: &Connection, session_id: &str) -> Result<(), String> {
    let changed = db.execute(
        "UPDATE relay_session_journal SET phase='restoring', updated_at=?2, last_error=?3 WHERE session_id=?1",
        params![session_id, Utc::now().to_rfc3339(), "Route restoration did not complete; retry is required."],
    ).map_err(|err| err.to_string())?;
    if changed != 1 {
        return Err("Relay restore journal entry was not found.".into());
    }
    Ok(())
}

pub fn pending_relay_recovery(db: &Connection) -> Result<Vec<RelayJournalEntry>, String> {
    let mut statement = db.prepare(
        "SELECT session_id,phase,protected_restore_blob,last_error FROM relay_session_journal ORDER BY created_at",
    ).map_err(|err| err.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|err| err.to_string())?;
    rows.map(|row| {
        let (session_id, phase, protected_restore_blob, last_error) =
            row.map_err(|err| err.to_string())?;
        Ok(RelayJournalEntry {
            session_id,
            phase: RelayJournalPhase::parse(&phase)?,
            protected_restore_blob,
            last_error,
        })
    })
    .collect()
}

pub fn clear_relay_session(db: &Connection, session_id: &str) -> Result<(), String> {
    let changed = db
        .execute(
            "DELETE FROM relay_session_journal WHERE session_id=?1 AND phase='restoring'",
            [session_id],
        )
        .map_err(|err| err.to_string())?;
    if changed != 1 {
        return Err("Relay session can be cleared only after restoration is confirmed.".into());
    }
    Ok(())
}

fn valid_relay_session_id(value: &str) -> bool {
    (16..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

pub fn relay_manifest_generation(db: &Connection) -> Result<u64, String> {
    let generation = db
        .query_row(
            "SELECT generation FROM relay_manifest_state WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| err.to_string())?
        .unwrap_or(0);
    u64::try_from(generation).map_err(|_| "Stored relay manifest generation is invalid.".into())
}

pub fn cached_relay_manifest(db: &Connection) -> Result<Option<CachedRelayManifest>, String> {
    let cached = db.query_row(
        "SELECT generation, signer_key_id, accepted_at, envelope_json FROM relay_manifest_state WHERE id=1",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
    )
    .optional()
    .map_err(|err| err.to_string())?;
    cached
        .map(|(generation, signer_key_id, accepted_at, envelope_json)| {
            Ok(CachedRelayManifest {
                generation: u64::try_from(generation)
                    .map_err(|_| "Stored relay manifest generation is invalid.")?,
                signer_key_id,
                accepted_at,
                envelope_json,
            })
        })
        .transpose()
}

pub fn cache_verified_relay_manifest(
    db: &mut Connection,
    verified: &crate::relay_manifest::VerifiedManifest,
) -> Result<(), String> {
    let generation = verified.manifest().generation;
    let generation_i64 = i64::try_from(generation)
        .map_err(|_| "Relay manifest generation exceeds SQLite integer range.")?;
    let envelope_json = String::from_utf8(verified.signed_envelope().to_vec())
        .map_err(|_| "Verified relay manifest envelope is not valid UTF-8.")?;
    let accepted_at = Utc::now().to_rfc3339();
    let transaction = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|err| err.to_string())?;
    let current = transaction
        .query_row(
            "SELECT generation FROM relay_manifest_state WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|err| err.to_string())?
        .unwrap_or(0);
    if current < 0 || generation_i64 <= current {
        return Err("Relay manifest generation is stale or already accepted.".into());
    }
    transaction
        .execute(
            "INSERT INTO relay_manifest_state(id,generation,signer_key_id,accepted_at,envelope_json)
             VALUES(1,?1,?2,?3,?4)
             ON CONFLICT(id) DO UPDATE SET generation=excluded.generation, signer_key_id=excluded.signer_key_id, accepted_at=excluded.accepted_at, envelope_json=excluded.envelope_json",
            rusqlite::params![generation_i64, verified.signer_key_id(), accepted_at, envelope_json],
        )
        .map_err(|err| err.to_string())?;
    transaction.commit().map_err(|err| err.to_string())
}

pub fn custom_games(db: &Connection) -> Result<Vec<(String, String)>, String> {
    let mut statement = db
        .prepare("SELECT name, exe FROM custom_games ORDER BY name")
        .map_err(|err| err.to_string())?;
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|err| err.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

pub fn add_game(db: &Connection, name: &str, exe: &str) -> Result<(), String> {
    db.execute("INSERT INTO custom_games(name, exe) VALUES(?1, ?2) ON CONFLICT(exe) DO UPDATE SET name=excluded.name", params![name, exe]).map_err(|err| err.to_string())?;
    Ok(())
}

pub fn game_profile(db: &Connection, exe: &str) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT mode FROM game_profiles WHERE exe=?1",
        params![exe],
        |row| row.get(0),
    )
    .optional()
    .map_err(|err| err.to_string())
}

pub fn save_game_profile(db: &Connection, game: &str, exe: &str, mode: &str) -> Result<(), String> {
    db.execute("INSERT INTO game_profiles(exe,game,mode) VALUES(?1,?2,?3) ON CONFLICT(exe) DO UPDATE SET game=excluded.game, mode=excluded.mode", params![exe, game, mode])
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn default_mode(db: &Connection) -> Result<Option<String>, String> {
    db.query_row(
        "SELECT value FROM app_settings WHERE key='default_mode'",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(|err| err.to_string())
}

pub fn save_default_mode(db: &Connection, mode: &str) -> Result<(), String> {
    db.execute("INSERT INTO app_settings(key,value) VALUES('default_mode',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![mode])
        .map_err(|err| err.to_string())?;
    Ok(())
}

pub fn history(db: &Connection) -> Result<Vec<SessionRecord>, String> {
    let mut statement = db.prepare("SELECT id, at, game, mode, before_json, after_json, change FROM sessions ORDER BY id DESC LIMIT 100").map_err(|err| err.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                at: row.get(1)?,
                game: row.get(2)?,
                mode: row.get(3)?,
                before_json: row.get(4)?,
                after_json: row.get(5)?,
                change: row.get(6)?,
            })
        })
        .map_err(|err| err.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

pub fn save_session(
    db: &Connection,
    game: &str,
    mode: &str,
    before_json: &str,
    after_json: &str,
    change: &str,
) -> Result<(), String> {
    db.execute("INSERT INTO sessions(at,game,mode,before_json,after_json,change) VALUES(?1,?2,?3,?4,?5,?6)",
        params![Utc::now().to_rfc3339(), game, mode, before_json, after_json, change]).map_err(|err| err.to_string())?;
    Ok(())
}

pub fn frame_history(db: &Connection) -> Result<Vec<FrameSessionRecord>, String> {
    let mut statement = db.prepare("SELECT id, at, game, pid, frame_count, average_fps, one_percent_low, point_one_percent_low, average_frame_time_ms, p95_frame_time_ms, frame_time_std_dev_ms, frame_time_spikes, dropped_frames, average_cpu_busy_ms, average_gpu_time_ms, average_display_latency_ms, capture_seconds, csv_path, analysis_version FROM frame_sessions ORDER BY id DESC LIMIT 100").map_err(|err| err.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(FrameSessionRecord {
                id: row.get(0)?,
                at: row.get(1)?,
                game: row.get(2)?,
                pid: row.get(3)?,
                frame_count: row.get(4)?,
                average_fps: row.get(5)?,
                one_percent_low: row.get(6)?,
                point_one_percent_low: row.get(7)?,
                average_frame_time_ms: row.get(8)?,
                p95_frame_time_ms: row.get(9)?,
                frame_time_std_dev_ms: row.get(10)?,
                frame_time_spikes: row.get(11)?,
                dropped_frames: row.get(12)?,
                average_cpu_busy_ms: row.get(13)?,
                average_gpu_time_ms: row.get(14)?,
                average_display_latency_ms: row.get(15)?,
                capture_seconds: row.get(16)?,
                csv_path: row.get(17)?,
                analysis_version: row.get(18)?,
            })
        })
        .map_err(|err| err.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

pub fn save_frame_session(
    db: &Connection,
    capture: &crate::frames::FrameCapture,
) -> Result<(), String> {
    db.execute("INSERT INTO frame_sessions(at,game,pid,frame_count,average_fps,one_percent_low,point_one_percent_low,average_frame_time_ms,p95_frame_time_ms,frame_time_std_dev_ms,frame_time_spikes,dropped_frames,average_cpu_busy_ms,average_gpu_time_ms,average_display_latency_ms,capture_seconds,csv_path,analysis_version) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,1)", params![
        Utc::now().to_rfc3339(), capture.game, capture.pid, capture.frame_count,
        capture.average_fps, capture.one_percent_low, capture.point_one_percent_low,
        capture.average_frame_time_ms, capture.p95_frame_time_ms, capture.frame_time_std_dev_ms,
        capture.frame_time_spikes, capture.dropped_frames, capture.average_cpu_busy_ms,
        capture.average_gpu_time_ms, capture.average_display_latency_ms,
        capture.capture_seconds, capture.csv_path
    ]).map_err(|err| err.to_string())?;
    Ok(())
}

pub fn journal(db: &Connection) -> Result<Vec<JournalEntry>, String> {
    let mut statement = db
        .prepare("SELECT pid, started_at, original_priority FROM change_journal")
        .map_err(|err| err.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(JournalEntry {
                pid: row.get(0)?,
                started_at: row.get(1)?,
                original_priority: row.get(2)?,
            })
        })
        .map_err(|err| err.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|err| err.to_string())
}

pub fn record_priority(
    db: &Connection,
    pid: u32,
    started_at: u64,
    original: u32,
    game: &str,
    mode: &str,
) -> Result<(), String> {
    db.execute("INSERT OR IGNORE INTO change_journal(pid,started_at,original_priority,at,reason,game,mode) VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![pid, started_at, original, Utc::now().to_rfc3339(), "Temporary above-normal game process priority", game, mode]).map_err(|err| err.to_string())?;
    Ok(())
}

pub fn clear_priority(db: &Connection, pid: u32) -> Result<(), String> {
    db.execute("DELETE FROM change_journal WHERE pid=?1", params![pid])
        .map_err(|err| err.to_string())?;
    Ok(())
}

#[cfg(test)]
mod relay_manifest_store_tests {
    use super::*;
    use crate::relay_manifest::{
        verify_manifest, RelayManifest, RelayNode, SignedManifestEnvelope,
    };
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use ed25519_dalek::{Signer, SigningKey};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    const NOW: u64 = 1_800_000_000;
    const KEY_ID: &str = "storage-test-key";
    const DOMAIN: &[u8] = b"VYRE-RELAY-MANIFEST-V1\0";

    fn verified_manifest(generation: u64) -> crate::relay_manifest::VerifiedManifest {
        let signing_key = SigningKey::from_bytes(&[51u8; 32]);
        let manifest = RelayManifest {
            schema_version: 1,
            generation,
            issued_at: NOW - 10,
            expires_at: NOW + 3600,
            nodes: vec![RelayNode {
                id: format!("test-node-{generation}"),
                region: "eu-test".into(),
                endpoint_host: "relay.example.net".into(),
                endpoint_port: 51820,
                public_key: STANDARD.encode([generation as u8; 32]),
                protocol: "wireguard-udp".into(),
            }],
        };
        let payload = serde_json::to_vec(&manifest).unwrap();
        let mut signed_message = Vec::new();
        signed_message.extend_from_slice(DOMAIN);
        signed_message.extend_from_slice(KEY_ID.as_bytes());
        signed_message.push(0);
        signed_message.extend_from_slice(&payload);
        let envelope = SignedManifestEnvelope {
            key_id: KEY_ID.into(),
            payload: STANDARD.encode(payload),
            signature: STANDARD.encode(signing_key.sign(&signed_message).to_bytes()),
        };
        let trusted = BTreeMap::from([(KEY_ID.into(), signing_key.verifying_key().to_bytes())]);
        verify_manifest(
            &serde_json::to_vec(&envelope).unwrap(),
            &trusted,
            NOW,
            generation.saturating_sub(1),
        )
        .unwrap()
    }

    #[test]
    fn persists_cached_envelope_and_rejects_generation_replay() {
        let mut db = open(Path::new(":memory:")).unwrap();
        assert_eq!(relay_manifest_generation(&db).unwrap(), 0);

        let first = verified_manifest(1);
        cache_verified_relay_manifest(&mut db, &first).unwrap();
        assert_eq!(relay_manifest_generation(&db).unwrap(), 1);
        let cached = cached_relay_manifest(&db).unwrap().unwrap();
        assert_eq!(cached.generation, 1);
        assert_eq!(cached.signer_key_id, KEY_ID);
        assert!(!cached.envelope_json.is_empty());
        assert!(cache_verified_relay_manifest(&mut db, &first).is_err());

        let second = verified_manifest(2);
        cache_verified_relay_manifest(&mut db, &second).unwrap();
        assert_eq!(relay_manifest_generation(&db).unwrap(), 2);

        let database = Mutex::new(db);
        let trusted_key = SigningKey::from_bytes(&[51u8; 32])
            .verifying_key()
            .to_bytes();
        let trusted = BTreeMap::from([(KEY_ID.into(), trusted_key)]);
        let cached = crate::relay_client::load_cached(&database, &trusted, NOW)
            .unwrap()
            .unwrap();
        assert_eq!(cached.manifest().generation, 2);
        assert_eq!(
            crate::relay_client::load_cached(&database, &trusted, NOW + 3601),
            Err(crate::relay_client::ManifestFetchError::Manifest(
                crate::relay_manifest::ManifestError::ManifestExpired
            ))
        );
    }

    #[test]
    fn relay_restore_journal_survives_restart_and_clears_only_after_confirmed_restore() {
        let path = std::env::temp_dir().join(format!(
            "vyre-relay-restore-test-{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        {
            let db = open(&path).unwrap();
            begin_relay_session(
                &db,
                "session-0123456789abcdef",
                b"dpapi-protected-test-snapshot",
            )
            .unwrap();
            assert!(advance_relay_session(
                &db,
                "session-0123456789abcdef",
                RelayJournalPhase::RoutesActive
            )
            .is_err());
            advance_relay_session(
                &db,
                "session-0123456789abcdef",
                RelayJournalPhase::TunnelReady,
            )
            .unwrap();
            advance_relay_session(
                &db,
                "session-0123456789abcdef",
                RelayJournalPhase::RoutesActive,
            )
            .unwrap();
        }

        let db = open(&path).unwrap();
        let pending = pending_relay_recovery(&db).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].phase, RelayJournalPhase::RoutesActive);
        assert_eq!(
            pending[0].protected_restore_blob,
            b"dpapi-protected-test-snapshot"
        );
        assert!(clear_relay_session(&db, "session-0123456789abcdef").is_err());

        record_relay_restore_failure(&db, "session-0123456789abcdef").unwrap();
        let failed = pending_relay_recovery(&db).unwrap();
        assert_eq!(failed[0].phase, RelayJournalPhase::Restoring);
        assert_eq!(
            failed[0].last_error.as_deref(),
            Some("Route restoration did not complete; retry is required.")
        );
        advance_relay_session(
            &db,
            "session-0123456789abcdef",
            RelayJournalPhase::Restoring,
        )
        .unwrap();
        clear_relay_session(&db, "session-0123456789abcdef").unwrap();
        assert!(pending_relay_recovery(&db).unwrap().is_empty());
        drop(db);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn relay_restore_journal_rejects_invalid_ids_and_empty_snapshots() {
        let db = open(Path::new(":memory:")).unwrap();
        assert!(begin_relay_session(&db, "bad", b"ciphertext").is_err());
        assert!(begin_relay_session(&db, "session-0123456789abcdef", b"").is_err());
    }
}
