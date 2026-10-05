mod frames;
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
    frame_before: Option<frames::FrameCapture>,
    frame_after: Option<frames::FrameCapture>,
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
    storage::save_default_mode(&*state.db.lock().map_err(|err| err.to_string())?, &mode)?;
    *state.mode.lock().map_err(|err| err.to_string())? = mode;
    Ok(())
}

fn detected_game(state: &AppState, pid: u32) -> Result<Game, String> {
    state
        .monitor
        .snapshot(&custom_games(state)?)
        .games
        .into_iter()
        .find(|game| game.pid == pid)
        .ok_or_else(|| "Game process is no longer running.".to_string())
}

#[tauri::command]
fn get_game_profile(state: State<AppState>, pid: u32) -> Result<String, String> {
    let game = detected_game(&state, pid)?;
    Ok(
        storage::game_profile(&*state.db.lock().map_err(|err| err.to_string())?, &game.exe)?
            .unwrap_or(state.mode.lock().map_err(|err| err.to_string())?.clone()),
    )
}

#[tauri::command]
fn set_game_profile(state: State<AppState>, pid: u32, mode: String) -> Result<(), String> {
    if mode != "Safe" && mode != "Competitive" {
        return Err("This mode is not implemented yet.".into());
    }
    let game = detected_game(&state, pid)?;
    storage::save_game_profile(
        &*state.db.lock().map_err(|err| err.to_string())?,
        &game.name,
        &game.exe,
        &mode,
    )
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
fn boost_game(
    app: tauri::AppHandle,
    state: State<AppState>,
    pid: u32,
) -> Result<BenchmarkResult, String> {
    let custom = custom_games(&state)?;
    let game: Game = state
        .monitor
        .snapshot(&custom)
        .games
        .into_iter()
        .find(|game| game.pid == pid)
        .ok_or("Game process is no longer running.")?;
    let mode = storage::game_profile(&*state.db.lock().map_err(|err| err.to_string())?, &game.exe)?
        .unwrap_or(state.mode.lock().map_err(|err| err.to_string())?.clone());
    let before = measure(8, Duration::from_millis(700));
    let mut warning = None;
    let mut frame_before = None;
    let mut frame_after = None;
    let change = if mode == "Competitive" {
        let already_active = storage::journal(&*state.db.lock().map_err(|err| err.to_string())?)?
            .iter()
            .any(|entry| entry.pid == pid && entry.started_at == game.started_at);
        if already_active {
            "Game process priority was already managed by this session; no second change was applied.".to_string()
        } else {
            match capture_game_frames(&app, &state, &game, 15) {
                Err(error) => {
                    warning = Some(format!("Competitive mode requires a frame baseline; priority was not changed. {error}"));
                    "No system change applied because a frame baseline was unavailable.".to_string()
                }
                Ok(baseline) => {
                    frame_before = Some(baseline.clone());
                    match priority::current(pid) {
                        Err(error) => {
                            warning = Some(error);
                            "No system change applied.".to_string()
                        }
                        Ok(original) if priority::is_above_normal(original) => {
                            warning = Some("The game already has Above Normal priority, so VYRE left it unchanged.".into());
                            "No change applied because the game already has Above Normal priority."
                                .to_string()
                        }
                        Ok(original) => {
                            storage::record_priority(
                                &*state.db.lock().map_err(|err| err.to_string())?,
                                pid,
                                game.started_at,
                                original,
                                &game.name,
                                &mode,
                            )?;
                            if let Err(error) = priority::set_above_normal(pid) {
                                let _ = storage::clear_priority(
                                    &*state.db.lock().map_err(|err| err.to_string())?,
                                    pid,
                                );
                                warning = Some(error);
                                "No system change applied.".to_string()
                            } else {
                                match capture_game_frames(&app, &state, &game, 15) {
                                    Err(error) => {
                                        if let Err(restore_error) =
                                            restore_priority_entry(&state, &game, original)
                                        {
                                            warning = Some(format!("After-capture failed ({error}); automatic restore also failed ({restore_error}). Restore remains journaled."));
                                            "Priority was changed temporarily; restore is still pending.".to_string()
                                        } else {
                                            warning = Some(format!("After-capture failed, so VYRE restored the original priority. {error}"));
                                            "Original process priority restored because the result could not be measured.".to_string()
                                        }
                                    }
                                    Ok(after) => {
                                        let improves = after.one_percent_low
                                            >= baseline.one_percent_low * 1.03
                                            && after.average_fps >= baseline.average_fps * 0.98;
                                        if improves {
                                            frame_after = Some(after);
                                            "Kept Above Normal priority: 1% low improved by at least 3% with average FPS within 2% of baseline.".to_string()
                                        } else if let Err(error) =
                                            restore_priority_entry(&state, &game, original)
                                        {
                                            frame_after = Some(after);
                                            warning = Some(format!("Frame data did not meet the keep threshold, but automatic restore failed ({error}). Restore remains journaled."));
                                            "Priority was changed temporarily; restore is still pending.".to_string()
                                        } else {
                                            frame_after = Some(after);
                                            "Restored original priority because frame data did not meet the improvement threshold.".to_string()
                                        }
                                    }
                                }
                            }
                        }
                    }
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
        frame_before,
        frame_after,
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
    let frame_captures = storage::frame_history(&*state.db.lock().map_err(|err| err.to_string())?)?;
    let active_changes = storage::journal(&*state.db.lock().map_err(|err| err.to_string())?)?.len();
    let report = json!({
        "app": "vyre", "version": env!("CARGO_PKG_VERSION"), "createdAt": Utc::now().to_rfc3339(),
        "probe": probe, "networkScore": stability_score(&probe),
        "system": { "cpuPercent": machine.cpu_percent, "ramPercent": machine.ram_percent, "ramUsedGb": machine.ram_used_gb, "ramTotalGb": machine.ram_total_gb, "downloadMbps": machine.download_mbps, "uploadMbps": machine.upload_mbps },
        "detectedGames": machine.games.iter().map(|game| &game.name).collect::<Vec<_>>(),
        "topProcesses": machine.top_processes,
        "frameCaptures": frame_captures,
        "activeTemporaryChanges": active_changes,
        "limitations": ["Probe target is not a game server", "FPS is available only through an on-demand 15-second capture; no continuous overlay is included", "No bufferbloat test or relay nodes are available"]
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

fn restore_priority_entry(state: &AppState, game: &Game, original: u32) -> Result<(), String> {
    if state.monitor.process_matches(game.pid, game.started_at) {
        priority::restore(game.pid, original)?;
    }
    storage::clear_priority(&*state.db.lock().map_err(|err| err.to_string())?, game.pid)
}

fn capture_game_frames(
    app: &tauri::AppHandle,
    state: &AppState,
    game: &Game,
    seconds: u32,
) -> Result<frames::FrameCapture, String> {
    if !state.monitor.process_matches(game.pid, game.started_at) {
        return Err("The selected game exited before capture started.".into());
    }
    let resource_dir = app.path().resource_dir().map_err(|err| err.to_string())?;
    let packaged = resource_dir.join("PresentMon-2.6.0-x64.exe");
    let resource_subdir = resource_dir.join("resources/PresentMon-2.6.0-x64.exe");
    let portable = std::env::current_exe().ok().and_then(|path| {
        path.parent()
            .map(|parent| parent.join("resources/PresentMon-2.6.0-x64.exe"))
    });
    let executable = [
        Some(packaged),
        Some(resource_subdir),
        portable,
        Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/PresentMon-2.6.0-x64.exe")),
    ]
    .into_iter()
    .flatten()
    .find(|path| path.is_file())
    .ok_or("The bundled PresentMon capture tool is missing.")?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|err| err.to_string())?
        .join("frames");
    std::fs::create_dir_all(&directory)
        .map_err(|err| format!("Could not create capture folder: {err}"))?;
    let csv_path = directory.join(format!(
        "vyre-frames-{}-{}.csv",
        game.pid,
        Utc::now().format("%Y%m%d-%H%M%S-%3f")
    ));
    let pid_arg = game.pid.to_string();
    let seconds_arg = seconds.to_string();
    let mut command = std::process::Command::new(executable);
    command
        .args(["--process_id", &pid_arg, "--output_file"])
        .arg(&csv_path)
        .args([
            "--timed",
            &seconds_arg,
            "--terminate_after_timed",
            "--no_console_stats",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("Could not start PresentMon: {err}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(seconds as u64 + 10);
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| format!("PresentMon capture failed: {err}"))?
        {
            if !status.success() {
                return Err("PresentMon could not start a Windows frame trace. Windows may restrict tracing for this account or game.".into());
            }
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "PresentMon did not finish the {seconds}-second capture in time."
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
    if !state.monitor.process_matches(game.pid, game.started_at) {
        let _ = std::fs::remove_file(&csv_path);
        return Err("The game exited during capture. No frame result was saved.".into());
    }
    let capture = frames::parse_capture(&csv_path, game.name.clone(), game.pid, seconds)?;
    storage::save_frame_session(&*state.db.lock().map_err(|err| err.to_string())?, &capture)?;
    Ok(capture)
}

#[tauri::command]
fn capture_frames(
    app: tauri::AppHandle,
    state: State<AppState>,
    pid: u32,
) -> Result<frames::FrameCapture, String> {
    let game = detected_game(&state, pid)
        .map_err(|_| "Select a running detected game before capturing frame data.")?;
    capture_game_frames(&app, &state, &game, 15)
}

#[tauri::command]
fn frame_history(state: State<AppState>) -> Result<Vec<storage::FrameSessionRecord>, String> {
    storage::frame_history(&*state.db.lock().map_err(|err| err.to_string())?)
}

pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let db_path = app.path().app_data_dir()?.join("vyre.sqlite3");
            let db = storage::open(&db_path).map_err(std::io::Error::other)?;
            let default_mode = storage::default_mode(&db)
                .map_err(std::io::Error::other)?
                .unwrap_or_else(|| "Safe".into());
            app.manage(AppState {
                db: Mutex::new(db),
                monitor: SystemMonitor::new(),
                mode: Mutex::new(default_mode),
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
            get_game_profile,
            set_game_profile,
            add_custom_game,
            run_diagnostic,
            boost_game,
            history,
            restore_everything,
            export_report,
            traffic_connections,
            trace_route,
            capture_frames,
            frame_history
        ])
        .build(tauri::generate_context!())
        .expect("failed to build vyre");
    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            let _ = restore_entries(app.state::<AppState>().inner(), false);
        }
    });
}
