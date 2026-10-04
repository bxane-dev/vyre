use chrono::Utc;
use rusqlite::{params, Connection};
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
        CREATE TABLE IF NOT EXISTS sessions (id INTEGER PRIMARY KEY, at TEXT NOT NULL, game TEXT NOT NULL, mode TEXT NOT NULL, before_json TEXT NOT NULL, after_json TEXT NOT NULL, change TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS change_journal (pid INTEGER PRIMARY KEY, started_at INTEGER NOT NULL, original_priority INTEGER NOT NULL, at TEXT NOT NULL, reason TEXT NOT NULL, game TEXT NOT NULL, mode TEXT NOT NULL);")
        .map_err(|err| err.to_string())?;
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
