use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

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
        CREATE TABLE IF NOT EXISTS change_journal (pid INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, original_priority INTEGER NOT NULL, at TEXT NOT NULL, reason TEXT NOT NULL, game TEXT NOT NULL, mode TEXT NOT NULL);")
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
