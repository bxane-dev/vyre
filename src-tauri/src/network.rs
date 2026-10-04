use regex::Regex;
use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

pub const PROBE_TARGET: &str = "1.1.1.1";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub target: String,
    pub sent: usize,
    pub received: usize,
    pub average_ms: Option<f64>,
    pub best_ms: Option<f64>,
    pub worst_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loss_percent: Option<f64>,
    pub samples_ms: Vec<Option<f64>>,
    pub error: Option<String>,
}

pub fn measure(count: usize, interval: Duration) -> Probe {
    let mut samples = Vec::with_capacity(count);
    let mut error = None;
    let expression = Regex::new(r"(?i)(?:time|zeit|temps|tempo)\s*([=<])\s*(\d+)\s*ms").unwrap();
    for index in 0..count {
        let started = Instant::now();
        let mut command = Command::new("ping.exe");
        command.args(["-n", "1", "-w", "1200", PROBE_TARGET]);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        match command.output() {
            Ok(output) => {
                // Windows ping is localized; decode its console output with the active OEM code page
                // where possible. The numeric fallback is only accepted when the command succeeded.
                let output_text = String::from_utf8_lossy(&output.stdout);
                let value = expression.captures(&output_text).and_then(|capture| {
                    let raw = capture.get(2)?.as_str().parse::<f64>().ok()?;
                    Some(if capture.get(1)?.as_str() == "<" {
                        raw / 2.0
                    } else {
                        raw
                    })
                });
                if output.status.success() {
                    if value.is_none() {
                        error = Some(
                            "Ping replied, but Windows output could not be parsed on this locale."
                                .into(),
                        );
                    }
                    samples.push(value);
                } else {
                    samples.push(None);
                }
            }
            Err(err) => {
                error = Some(format!("Unable to run Windows ping: {err}"));
                samples.push(None);
            }
        }
        if index + 1 < count {
            thread::sleep(interval.saturating_sub(started.elapsed()));
        }
    }
    let values: Vec<f64> = samples.iter().filter_map(|sample| *sample).collect();
    let received = values.len();
    let jitter = if values.len() > 1 {
        Some(
            values
                .windows(2)
                .map(|pair| (pair[1] - pair[0]).abs())
                .sum::<f64>()
                / (values.len() - 1) as f64,
        )
    } else {
        None
    };
    let valid = error.is_none();
    Probe {
        target: PROBE_TARGET.into(),
        sent: count,
        received,
        average_ms: valid
            .then(|| (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64))
            .flatten(),
        best_ms: valid
            .then(|| values.iter().copied().reduce(f64::min))
            .flatten(),
        worst_ms: valid
            .then(|| values.iter().copied().reduce(f64::max))
            .flatten(),
        jitter_ms: if valid { jitter } else { None },
        loss_percent: if valid {
            Some(100.0 * (count - received) as f64 / count as f64)
        } else {
            None
        },
        samples_ms: samples,
        error,
    }
}

pub fn stability_score(probe: &Probe) -> Option<u8> {
    let ping = probe.average_ms?;
    let jitter = probe.jitter_ms.unwrap_or(0.0);
    let loss = probe.loss_percent?;
    Some(
        (100.0 - (ping / 3.0).min(35.0) - (jitter * 2.0).min(25.0) - (loss * 3.0).min(40.0))
            .clamp(0.0, 100.0)
            .round() as u8,
    )
}
