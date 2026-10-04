#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::process::Command;

pub fn trace_direct_route() -> Result<String, String> {
    let mut command = Command::new("tracert.exe");
    command.args(["-d", "-h", "12", "-w", "350", crate::network::PROBE_TARGET]);
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let output = command
        .output()
        .map_err(|err| format!("Unable to run tracert: {err}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err("Windows tracert returned no route data.".into());
    }
    Ok(text.chars().take(12_000).collect())
}
