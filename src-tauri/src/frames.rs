use serde::Serialize;
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCapture {
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
}

#[derive(Default)]
struct FrameSample {
    displayed_ms: Option<f64>,
    cpu_busy_ms: Option<f64>,
    gpu_time_ms: Option<f64>,
    display_latency_ms: Option<f64>,
}

fn metric(record: &csv::StringRecord, index: Option<usize>) -> Option<f64> {
    let value = record
        .get(index?)
        .and_then(|value| value.trim().parse::<f64>().ok())?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn mean(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let values = values.flatten().collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

pub fn parse_capture(
    path: &Path,
    game: String,
    pid: u32,
    capture_seconds: u32,
) -> Result<FrameCapture, String> {
    let file =
        File::open(path).map_err(|err| format!("Could not read PresentMon capture: {err}"))?;
    let mut reader = csv::Reader::from_reader(BufReader::new(file));
    let headers = reader
        .headers()
        .map_err(|err| format!("PresentMon CSV header is invalid: {err}"))?
        .clone();
    let index = |name: &str| {
        headers
            .iter()
            .position(|header| header.eq_ignore_ascii_case(name))
    };
    let displayed_index = index("DisplayedTime").ok_or(
        "PresentMon CSV has no DisplayedTime column. Update VYRE's bundled PresentMon version.",
    )?;
    let chain_index = index("SwapChainAddress");
    let cpu_index = index("MsCPUBusy");
    let gpu_index = index("MsGPUTime");
    let latency_index = index("DisplayLatency");
    let mut chains: HashMap<String, Vec<FrameSample>> = HashMap::new();
    for row in reader.records() {
        let row = row.map_err(|err| format!("PresentMon CSV row is invalid: {err}"))?;
        let sample = FrameSample {
            displayed_ms: metric(&row, Some(displayed_index)).filter(|value| *value > 0.0),
            cpu_busy_ms: metric(&row, cpu_index),
            gpu_time_ms: metric(&row, gpu_index),
            display_latency_ms: metric(&row, latency_index),
        };
        let chain = chain_index
            .and_then(|column| row.get(column))
            .unwrap_or("default")
            .to_owned();
        chains.entry(chain).or_default().push(sample);
    }
    let samples = chains
        .into_values()
        .max_by_key(|samples| {
            samples
                .iter()
                .filter(|sample| sample.displayed_ms.is_some())
                .count()
        })
        .unwrap_or_default();
    let mut frame_times = samples
        .iter()
        .filter_map(|sample| sample.displayed_ms)
        .collect::<Vec<_>>();
    if frame_times.len() < 30 {
        return Err(format!(
            "PresentMon recorded only {} displayed frames. Keep the game active and retry.",
            frame_times.len()
        ));
    }
    frame_times.sort_by(f64::total_cmp);
    let mean_frame_time = frame_times.iter().sum::<f64>() / frame_times.len() as f64;
    let median = frame_times[frame_times.len() / 2];
    let percentile_ms = |percentile: f64| {
        let index = ((frame_times.len() - 1) as f64 * percentile).ceil() as usize;
        frame_times[index.min(frame_times.len() - 1)]
    };
    let variance = frame_times
        .iter()
        .map(|time| (time - mean_frame_time).powi(2))
        .sum::<f64>()
        / frame_times.len() as f64;
    let spike_threshold_ms = 33.3_f64.max(median * 2.0);
    let frame_time_spikes = frame_times
        .iter()
        .filter(|time| **time >= spike_threshold_ms)
        .count();
    let average_cpu_busy_ms = mean(samples.iter().map(|sample| sample.cpu_busy_ms));
    let average_gpu_time_ms = mean(samples.iter().map(|sample| sample.gpu_time_ms));
    let average_display_latency_ms = mean(samples.iter().map(|sample| sample.display_latency_ms));
    let csv_path = path.to_string_lossy().into_owned();
    Ok(FrameCapture {
        game,
        pid,
        frame_count: frame_times.len(),
        average_fps: 1000.0 / mean_frame_time,
        one_percent_low: 1000.0 / percentile_ms(0.99),
        point_one_percent_low: 1000.0 / percentile_ms(0.999),
        average_frame_time_ms: mean_frame_time,
        p95_frame_time_ms: percentile_ms(0.95),
        frame_time_std_dev_ms: variance.sqrt(),
        frame_time_spikes,
        dropped_frames: samples
            .iter()
            .filter(|sample| sample.displayed_ms.is_none())
            .count(),
        average_cpu_busy_ms,
        average_gpu_time_ms,
        average_display_latency_ms,
        capture_seconds,
        csv_path,
    })
}
