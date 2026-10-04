use serde::Serialize;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{collections::HashMap, process::Command};
use sysinfo::System;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionInfo {
    pub pid: u32,
    pub process: String,
    pub local: String,
    pub remote: String,
    pub state: String,
}

pub fn connections() -> Result<Vec<ConnectionInfo>, String> {
    let mut command = Command::new("netstat.exe");
    command.args(["-ano", "-p", "TCP"]);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .map_err(|err| format!("Unable to run netstat: {err}"))?;
    if !output.status.success() {
        return Err("Windows netstat failed to list TCP connections.".into());
    }
    let system = System::new_all();
    let names: HashMap<u32, String> = system
        .processes()
        .iter()
        .map(|(pid, process)| (pid.as_u32(), process.name().to_string()))
        .collect();
    let mut connections = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() != 5 || !parts[0].eq_ignore_ascii_case("TCP") {
            continue;
        }
        let Ok(pid) = parts[4].parse::<u32>() else {
            continue;
        };
        if parts[2] == "0.0.0.0:0" || parts[2] == "[::]:0" {
            continue;
        }
        connections.push(ConnectionInfo {
            pid,
            process: names
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| "Unknown process".into()),
            local: parts[1].into(),
            remote: parts[2].into(),
            state: parts[3].into(),
        });
    }
    connections.sort_by(|a, b| a.process.cmp(&b.process).then(a.pid.cmp(&b.pid)));
    connections.truncate(80);
    Ok(connections)
}
