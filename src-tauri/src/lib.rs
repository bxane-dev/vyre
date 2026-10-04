mod network;
mod priority;
mod routing;
mod storage;
mod system;
mod traffic;

use chrono::Utc;
use network::{measure, stability_score, Probe};
use serde::Serialize;
use serde_json::json;
use std::{path::Path, sync::Mutex, thread, time::Duration};
use system::{Game, MachineSnapshot, SystemMonitor};
use tauri::{Manager, State};

struct AppState {
    db: Mutex<rusqlite::Connection>,
    monitor: SystemMonitor,
    mode: Mutex<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    at: String,
    machine: MachineSnapshot,
    probe: Probe,
    network_score: Option<u8>,
    mode: String,
    active_changes: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BenchmarkResult {
    game: String,
    mode: String,
    before: Probe,
    after: Probe,
    before_score: Option<u8>,
    after_score: Option<u8>,
    change: String,
    warning: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Diagnosis {
    probe: Probe,
    score: Option<u8>,
    primary_problem: String,
    evidence: String,
    recommendation: String,
    cpu_percent: f32,
    ram_percent: f64,
}

fn custom_games(state: &AppState) -> Result<Vec<(String, String)>, String> {
    storage::custom_games(&*state.db.lock().map_err(|err| err.to_string())?)
}

fn restore_entries(state: &AppState, only_closed: bool) -> Result<usize, String> {
    let entries = storage::journal(&*state.db.lock().map_err(|err| err.to_string())?)?;
    let mut restored = 0;
    for entry in entries {
        let still_same_process = state.monitor.process_matches(entry.pid, entry.started_at);
        if only_closed && still_same_process {
            continue;
        }
        if still_same_process {
            priority::restore(entry.pid, entry.original_priority)?;
            restored += 1;
        }
        storage::clear_priority(&*state.db.lock().map_err(|err| err.to_string())?, entry.pid)?;
    }
    Ok(restored)
}

#[tauri::command]
fn snapshot(state: State<AppState>) -> Result<Snapshot, String> {
    let custom = custom_games(&state)?;
    let machine = state.monitor.snapshot(&custom);
    let probe = measure(3, Duration::from_millis(250));
    let active_changes = storage::journal(&*state.db.lock().map_err(|err| err.to_string())?)?.len();
    Ok(Snapshot {
        at: Utc::now().to_rfc3339(),
        network_score: stability_score(&probe),
        probe,
        machine,
        mode: state.mode.lock().map_err(|err| err.to_string())?.clone(),
        active_changes,
    })
}

#[tauri::command]
fn set_mode(state: State<AppState>, mode: String) -> Result<(), String> {
    if mode != "Safe" && mode != "Competitive" {
        return Err("This mode is not implemented yet.".into());
    }
    *state.mode.lock().map_err(|err| err.to_string())? = mode;
    Ok(())
}

#[tauri::command]
fn add_custom_game(state: State<AppState>, name: String, exe: String) -> Result<(), String> {
    let name = name.trim();
    let exe = exe.trim();
    if name.is_empty() || name.len() > 80 {
        return Err("Enter a game name up to 80 characters.".into());
    }
    let path = Path::new(exe);
    if !path.is_file()
        || !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        return Err("Enter the full path to an existing .exe file.".into());
    }
    storage::add_game(&*state.db.lock().map_err(|err| err.to_string())?, name, exe)
}

#[tauri::command]
fn run_diagnostic(state: State<AppState>) -> Result<Diagnosis, String> {
    let probe = measure(30, Duration::from_secs(1));
    let machine = state.monitor.snapshot(&custom_games(&state)?);
    let (primary_problem, evidence, recommendation) = if let Some(error) = &probe.error {
        (
            "Probe unavailable".into(),
            error.clone(),
            "Check that Windows ping is available, then retry.".into(),
        )
    } else if probe.loss_percent.unwrap_or(0.0) > 1.0 {
        ("Packet loss".into(), format!("{} of {} probes did not reply to {}.", probe.sent - probe.received, probe.sent, probe.target), "Check Wi-Fi signal, local congestion, and the wired connection before changing routes.".into())
    } else if probe.jitter_ms.unwrap_or(0.0) > 8.0 {
        ("Unstable latency".into(), format!("Mean consecutive latency change: {:.1} ms to {}.", probe.jitter_ms.unwrap_or(0.0), probe.target), "Check simultaneous uploads and Wi-Fi interference. A loaded-link test is needed to confirm bufferbloat.".into())
    } else if machine.ram_percent > 90.0 {
        (
            "Memory pressure".into(),
            format!("RAM in use: {:.0}%.", machine.ram_percent),
            "Close memory-heavy background apps before launching a game.".into(),
        )
    } else if machine.cpu_percent > 85.0 {
        (
            "High CPU load".into(),
            format!("CPU load at test end: {:.0}%.", machine.cpu_percent),
            "Review the top CPU processes and close unnecessary work.".into(),
        )
    } else {
        ("No major issue detected".into(), format!("{} successful probes; average {:.1} ms; jitter {:.1} ms.", probe.received, probe.average_ms.unwrap_or(0.0), probe.jitter_ms.unwrap_or(0.0)), "Test while the issue is happening. This probe does not measure in-game FPS or bufferbloat.".into())
    };
    Ok(Diagnosis {
        score: stability_score(&probe),
        probe,
        primary_problem,
        evidence,
        recommendation,
        cpu_percent: machine.cpu_percent,
        ram_percent: machine.ram_percent,
    })
}

#[tauri::command]
fn boost_game(state: State<AppState>, pid: u32) -> Result<BenchmarkResult, String> {
    let custom = custom_games(&state)?;
    let game: Game = state
        .monitor
        .snapshot(&custom)
        .games
        .into_iter()
        .find(|game| game.pid == pid)
        .ok_or("Game process is no longer running.")?;
    let mode = state.mode.lock().map_err(|err| err.to_string())?.clone();
    let before = measure(8, Duration::from_millis(700));
    let mut warning = None;
    let change = if mode == "Competitive" {
        let already_active = storage::journal(&*state.db.lock().map_err(|err| err.to_string())?)?
            .iter()
            .any(|entry| entry.pid == pid && entry.started_at == game.started_at);
        if already_active {
            "Game process priority was already managed by this session.".to_string()
        } else {
            match priority::current(pid) {
                Ok(original) => {
                    storage::record_priority(
                        &*state.db.lock().map_err(|err| err.to_string())?,
                        pid,
                        game.started_at,
                        original,
                        &game.name,
                        &mode,
                    )?;
                    match priority::set_above_normal(pid) {
                    Ok(()) => "Set game process priority to Above Normal for this session; original priority saved for restore.".to_string(),
                    Err(error) => { let _ = storage::clear_priority(&*state.db.lock().map_err(|err| err.to_string())?, pid); warning = Some(error); "No system change applied.".to_string() }
                }
                }
                Err(error) => {
                    warning = Some(error);
                    "No system change applied.".to_string()
                }
            }
        }
    } else {
        "Safe mode measured the connection without changing system settings.".to_string()
    };
    let after = measure(8, Duration::from_millis(700));
    let before_score = stability_score(&before);
    let after_score = stability_score(&after);
    storage::save_session(
        &*state.db.lock().map_err(|err| err.to_string())?,
        &game.name,
        &mode,
        &serde_json::to_string(&before).map_err(|err| err.to_string())?,
        &serde_json::to_string(&after).map_err(|err| err.to_string())?,
        &change,
    )?;
    Ok(BenchmarkResult {
        game: game.name,
        mode,
        before,
        after,
        before_score,
        after_score,
        change,
        warning,
    })
}

#[tauri::command]
fn history(state: State<AppState>) -> Result<Vec<storage::SessionRecord>, String> {
    storage::history(&*state.db.lock().map_err(|err| err.to_string())?)
}

#[tauri::command]
fn restore_everything(state: State<AppState>) -> Result<usize, String> {
    restore_entries(&state, false)
}

#[tauri::command]
fn export_report(app: tauri::AppHandle, state: State<AppState>) -> Result<String, String> {
    let machine = state.monitor.snapshot(&custom_games(&state)?);
    let probe = measure(5, Duration::from_millis(500));
    let active_changes = storage::journal(&*state.db.lock().map_err(|err| err.to_string())?)?.len();
    let report = json!({
        "app": "vyre", "version": env!("CARGO_PKG_VERSION"), "createdAt": Utc::now().to_rfc3339(),
        "probe": probe, "networkScore": stability_score(&probe),
        "system": { "cpuPercent": machine.cpu_percent, "ramPercent": machine.ram_percent, "ramUsedGb": machine.ram_used_gb, "ramTotalGb": machine.ram_total_gb, "downloadMbps": machine.download_mbps, "uploadMbps": machine.upload_mbps },
        "detectedGames": machine.games.iter().map(|game| &game.name).collect::<Vec<_>>(),
        "topProcesses": machine.top_processes,
        "activeTemporaryChanges": active_changes,
        "limitations": ["Probe target is not a game server", "No FPS or bufferbloat measurements in this version", "No relay nodes are deployed"]
    });
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())?
        .join("reports");
    std::fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    let path = directory.join(format!(
        "vyre-report-{}.json",
        Utc::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&report).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
fn traffic_connections() -> Result<Vec<traffic::ConnectionInfo>, String> {
    traffic::connections()
}

#[tauri::command]
fn trace_route() -> Result<String, String> {
    routing::trace_direct_route()
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let db_path = app.path().app_data_dir()?.join("vyre.sqlite3");
            let db = storage::open(&db_path).map_err(std::io::Error::other)?;
            app.manage(AppState {
                db: Mutex::new(db),
                monitor: SystemMonitor::new(),
                mode: Mutex::new("Safe".into()),
            });
            let handle = app.handle().clone();
            // Crash recovery: replay any persisted priority changes from the prior run.
            let _ = restore_entries(handle.state::<AppState>().inner(), false);
            thread::spawn(move || loop {
                thread::sleep(Duration::from_secs(4));
                let _ = restore_entries(handle.state::<AppState>().inner(), true);
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            set_mode,
            add_custom_game,
            run_diagnostic,
            boost_game,
            history,
            restore_everything,
            export_report,
            traffic_connections,
            trace_route
        ])
        .build(tauri::generate_context!())
        .expect("failed to build vyre");
    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            let _ = restore_entries(app.state::<AppState>().inner(), false);
        }
    });
}
