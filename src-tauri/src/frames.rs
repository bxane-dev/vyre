use serde::Serialize;
use std::{collections::HashMap, fs::File, io::BufReader, path::Path};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameCapture {
    pub game: String,
    pub pid: u32,
    pub frame_count: usize,
    pub average_fps: f64,
    pub one_percent_low: f64,
    pub point_one_percent_low: f64,
    pub capture_seconds: u32,
    pub csv_path: String,
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
    let time_index = headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case("DisplayedTime"))
        .ok_or(
            "PresentMon CSV has no DisplayedTime column. Update vyre's bundled PresentMon version.",
        )?;
    let chain_index = headers
        .iter()
        .position(|header| header.eq_ignore_ascii_case("SwapChainAddress"));
    let mut chains: HashMap<String, Vec<f64>> = HashMap::new();
    for row in reader.records() {
        let row = row.map_err(|err| format!("PresentMon CSV row is invalid: {err}"))?;
        let Some(value) = row
            .get(time_index)
            .and_then(|value| value.trim().parse::<f64>().ok())
        else {
            continue;
        };
        if !value.is_finite() || value <= 0.0 {
            continue;
        }
        let chain = chain_index
            .and_then(|index| row.get(index))
            .unwrap_or("default")
            .to_owned();
        chains.entry(chain).or_default().push(value);
    }
    let mut frame_times = chains
        .into_values()
        .max_by_key(Vec::len)
        .unwrap_or_default();
    if frame_times.len() < 30 {
        return Err(format!(
            "PresentMon recorded only {} displayed frames. Keep the game active and retry.",
            frame_times.len()
        ));
    }
    frame_times.sort_by(f64::total_cmp);
    let mean = frame_times.iter().sum::<f64>() / frame_times.len() as f64;
    let percentile_ms = |percentile: f64| {
        let index = ((frame_times.len() - 1) as f64 * percentile).ceil() as usize;
        frame_times[index.min(frame_times.len() - 1)]
    };
    let csv_path = path.to_string_lossy().into_owned();
    Ok(FrameCapture {
        game,
        pid,
        frame_count: frame_times.len(),
        average_fps: 1000.0 / mean,
        one_percent_low: 1000.0 / percentile_ms(0.99),
        point_one_percent_low: 1000.0 / percentile_ms(0.999),
        capture_seconds,
        csv_path,
    })
}
