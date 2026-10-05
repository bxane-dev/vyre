use crate::steam_games::{discover_installed_games, game_for_executable, SteamGameInstall};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, sync::Mutex, time::Instant};
use sysinfo::{Networks, System};

const KNOWN_GAMES: &[(&str, &[&str])] = &[
    ("Valorant", &["VALORANT-Win64-Shipping.exe"]),
    ("Counter-Strike 2", &["cs2.exe"]),
    ("Fortnite", &["FortniteClient-Win64-Shipping.exe"]),
    ("Apex Legends", &["r5apex.exe"]),
    ("Minecraft", &["javaw.exe", "Minecraft.Windows.exe"]),
    ("Roblox", &["RobloxPlayerBeta.exe"]),
    ("Call of Duty", &["cod.exe", "ModernWarfare.exe"]),
    ("Rocket League", &["RocketLeague.exe"]),
    ("Overwatch", &["Overwatch.exe"]),
    ("League of Legends", &["League of Legends.exe"]),
    (
        "Rainbow Six Siege",
        &["RainbowSix.exe", "RainbowSix_Vulkan.exe"],
    ),
    ("GTA V", &["GTA5.exe"]),
    ("FiveM", &["FiveM.exe", "FiveM_GTAProcess.exe"]),
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub name: String,
    pub exe: String,
    pub pid: u32,
    pub started_at: u64,
    pub cpu_percent: f32,
    pub memory_mb: f64,
    pub custom: bool,
    pub steam_app_id: Option<u32>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub name: String,
    pub pid: u32,
    pub cpu_percent: f32,
    pub memory_mb: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub name: String,
    pub download_mbps: f64,
    pub upload_mbps: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineSnapshot {
    pub cpu_percent: f32,
    pub ram_percent: f64,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub download_mbps: f64,
    pub upload_mbps: f64,
    pub games: Vec<Game>,
    pub top_processes: Vec<ProcessInfo>,
    pub adapters: Vec<AdapterInfo>,
}

pub struct SystemMonitor {
    system: Mutex<System>,
    networks: Mutex<Networks>,
    network_at: Mutex<Instant>,
    steam_library: Mutex<Option<(Instant, Vec<SteamGameInstall>)>>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        let mut system = System::new_all();
        system.refresh_all();
        Self {
            system: Mutex::new(system),
            networks: Mutex::new(Networks::new_with_refreshed_list()),
            network_at: Mutex::new(Instant::now()),
            steam_library: Mutex::new(None),
        }
    }

    pub fn snapshot(&self, custom_games: &[(String, String)]) -> MachineSnapshot {
        let mut system = self.system.lock().unwrap();
        system.refresh_cpu();
        system.refresh_memory();
        system.refresh_processes();
        let mut networks = self.networks.lock().unwrap();
        networks.refresh();
        let mut network_at = self.network_at.lock().unwrap();
        let seconds = network_at.elapsed().as_secs_f64().max(0.1);
        *network_at = Instant::now();
        let download_bytes: u64 = networks.iter().map(|(_, data)| data.received()).sum();
        let upload_bytes: u64 = networks.iter().map(|(_, data)| data.transmitted()).sum();
        let mut adapters: Vec<AdapterInfo> = networks
            .iter()
            .map(|(name, data)| AdapterInfo {
                name: name.clone(),
                download_mbps: data.received() as f64 * 8.0 / seconds / 1_000_000.0,
                upload_mbps: data.transmitted() as f64 * 8.0 / seconds / 1_000_000.0,
            })
            .filter(|adapter| adapter.download_mbps > 0.001 || adapter.upload_mbps > 0.001)
            .collect();
        adapters.sort_by(|a, b| {
            (b.download_mbps + b.upload_mbps).total_cmp(&(a.download_mbps + a.upload_mbps))
        });
        let custom_by_file: HashMap<String, (String, String)> = custom_games
            .iter()
            .filter_map(|(name, path)| {
                Path::new(path).file_name().map(|file| {
                    (
                        file.to_string_lossy().to_ascii_lowercase(),
                        (name.clone(), path.clone()),
                    )
                })
            })
            .collect();
        let steam_games = {
            let mut cached = self.steam_library.lock().unwrap();
            if cached
                .as_ref()
                .is_none_or(|(at, _)| at.elapsed() >= std::time::Duration::from_secs(60))
            {
                let steam_executable = system
                    .processes()
                    .values()
                    .find(|process| process.name().eq_ignore_ascii_case("steam.exe"))
                    .and_then(|process| process.exe())
                    .map(Path::to_path_buf);
                *cached = Some((Instant::now(), discover_installed_games(steam_executable)));
            }
            cached
                .as_ref()
                .map(|(_, games)| games.clone())
                .unwrap_or_default()
        };
        let mut games = Vec::new();
        let mut processes = Vec::new();
        for (pid, process) in system.processes() {
            let exe_name = process.name().to_string();
            let known = KNOWN_GAMES.iter().find(|(_, files)| {
                files
                    .iter()
                    .any(|file| file.eq_ignore_ascii_case(&exe_name))
                    && (exe_name.to_ascii_lowercase() != "javaw.exe"
                        || process
                            .cmd()
                            .iter()
                            .any(|arg| arg.to_ascii_lowercase().contains("minecraft")))
            });
            let custom = custom_by_file
                .get(&exe_name.to_ascii_lowercase())
                .filter(|(_, path)| {
                    process
                        .exe()
                        .is_some_and(|running| running.to_string_lossy().eq_ignore_ascii_case(path))
                });
            let executable_path = process.exe();
            let steam_game =
                executable_path.and_then(|path| game_for_executable(path, &steam_games));
            let detected = if let Some((name, _)) = known {
                Some((
                    name.to_string(),
                    exe_name.clone(),
                    false,
                    steam_game.map(|game| game.app_id),
                ))
            } else if let Some(game) = steam_game {
                executable_path.map(|path| {
                    (
                        game.name.clone(),
                        path.to_string_lossy().into_owned(),
                        false,
                        Some(game.app_id),
                    )
                })
            } else {
                custom.map(|(name, path)| (name.clone(), path.clone(), true, None))
            };
            if let Some((name, exe, custom_flag, steam_app_id)) = detected {
                games.push(Game {
                    name,
                    exe,
                    pid: pid.as_u32(),
                    started_at: process.start_time(),
                    cpu_percent: process.cpu_usage(),
                    memory_mb: process.memory() as f64 / 1_048_576.0,
                    custom: custom_flag,
                    steam_app_id,
                });
            }
            if process.cpu_usage() > 0.1 || process.memory() > 250_000_000 {
                processes.push(ProcessInfo {
                    name: exe_name,
                    pid: pid.as_u32(),
                    cpu_percent: process.cpu_usage(),
                    memory_mb: process.memory() as f64 / 1_048_576.0,
                });
            }
        }
        games.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        processes.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        processes.truncate(7);
        let total = system.total_memory() as f64;
        MachineSnapshot {
            cpu_percent: system.global_cpu_info().cpu_usage(),
            ram_percent: if total > 0.0 {
                100.0 * system.used_memory() as f64 / total
            } else {
                0.0
            },
            ram_used_gb: system.used_memory() as f64 / 1_073_741_824.0,
            ram_total_gb: total / 1_073_741_824.0,
            download_mbps: download_bytes as f64 * 8.0 / seconds / 1_000_000.0,
            upload_mbps: upload_bytes as f64 * 8.0 / seconds / 1_000_000.0,
            games,
            top_processes: processes,
            adapters,
        }
    }

    pub fn process_matches(&self, pid: u32, started_at: u64) -> bool {
        let mut system = self.system.lock().unwrap();
        system.refresh_processes();
        system
            .process(sysinfo::Pid::from_u32(pid))
            .is_some_and(|process| process.start_time() == started_at)
    }
}
