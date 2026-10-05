//! Relay route scoring primitives. This module intentionally has no network,
//! UI, or Windows policy side effects; callers must supply measured samples.

use serde::{Deserialize, Serialize};

const JITTER_WEIGHT: f64 = 2.0;
const LOSS_WEIGHT_MS: f64 = 1000.0;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeMethod {
    Icmp,
    UdpEcho,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteMeasurement {
    pub route_id: String,
    pub target: String,
    pub method: ProbeMethod,
    /// Total probes sent. `samples_ms` contains one entry per probe; `None`
    /// represents a timeout or lost reply.
    pub sent: usize,
    pub samples_ms: Vec<Option<f64>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteMetrics {
    pub route_id: String,
    pub target: String,
    pub method: ProbeMethod,
    pub sent: usize,
    pub received: usize,
    pub loss_percent: f64,
    pub median_rtt_ms: f64,
    pub p95_deviation_ms: f64,
    pub score_ms: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RouteComparison {
    pub direct: RouteMetrics,
    pub candidate: RouteMetrics,
    /// Positive means the candidate scored better than direct.
    pub improvement_ms: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScoreError {
    EmptyRouteId,
    EmptyTarget,
    NoProbes,
    SampleCountMismatch,
    NoValidSamples,
    InvalidSample,
    IncomparableTarget,
    IncomparableMethod,
}

impl std::fmt::Display for ScoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::EmptyRouteId => "Route identifier is empty.",
            Self::EmptyTarget => "Probe target is empty.",
            Self::NoProbes => "No probes were sent.",
            Self::SampleCountMismatch => "Probe count does not match the sample list.",
            Self::NoValidSamples => "No valid probe replies were measured.",
            Self::InvalidSample => "Probe samples must be finite, non-negative times.",
            Self::IncomparableTarget => "Routes must be measured against the same target.",
            Self::IncomparableMethod => "Routes must use the same probe method.",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for ScoreError {}

pub fn summarize(measurement: RouteMeasurement) -> Result<RouteMetrics, ScoreError> {
    if measurement.route_id.trim().is_empty() {
        return Err(ScoreError::EmptyRouteId);
    }
    if measurement.target.trim().is_empty() {
        return Err(ScoreError::EmptyTarget);
    }
    if measurement.sent == 0 {
        return Err(ScoreError::NoProbes);
    }
    if measurement.samples_ms.len() != measurement.sent {
        return Err(ScoreError::SampleCountMismatch);
    }

    let mut samples = Vec::with_capacity(measurement.sent);
    for sample in measurement.samples_ms.into_iter().flatten() {
        if !sample.is_finite() || sample < 0.0 {
            return Err(ScoreError::InvalidSample);
        }
        samples.push(sample);
    }
    if samples.is_empty() {
        return Err(ScoreError::NoValidSamples);
    }
    samples.sort_by(f64::total_cmp);

    let median = median(&samples);
    let mut deviations: Vec<f64> = samples.iter().map(|value| (value - median).abs()).collect();
    deviations.sort_by(f64::total_cmp);
    let p95_deviation = percentile(&deviations, 0.95);
    let received = samples.len();
    let loss_fraction = (measurement.sent - received) as f64 / measurement.sent as f64;
    let loss_percent = loss_fraction * 100.0;
    let score_ms = median + JITTER_WEIGHT * p95_deviation + LOSS_WEIGHT_MS * loss_fraction;

    Ok(RouteMetrics {
        route_id: measurement.route_id.trim().to_owned(),
        target: measurement.target.trim().to_owned(),
        method: measurement.method,
        sent: measurement.sent,
        received,
        loss_percent,
        median_rtt_ms: median,
        p95_deviation_ms: p95_deviation,
        score_ms,
    })
}

pub fn compare(
    direct: RouteMeasurement,
    candidate: RouteMeasurement,
) -> Result<RouteComparison, ScoreError> {
    let direct = summarize(direct)?;
    let candidate = summarize(candidate)?;
    if !direct.target.eq_ignore_ascii_case(&candidate.target) {
        return Err(ScoreError::IncomparableTarget);
    }
    if direct.method != candidate.method {
        return Err(ScoreError::IncomparableMethod);
    }
    let improvement_ms = direct.score_ms - candidate.score_ms;
    Ok(RouteComparison {
        direct,
        candidate,
        improvement_ms,
    })
}

fn percentile(sorted: &[f64], quantile: f64) -> f64 {
    // Nearest-rank percentile, with a one-based rank as defined for small sets.
    let rank = (quantile * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank - 1]
}

fn median(sorted: &[f64]) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        sorted[middle - 1] / 2.0 + sorted[middle] / 2.0
    } else {
        sorted[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(route_id: &str, target: &str, samples_ms: &[Option<f64>]) -> RouteMeasurement {
        RouteMeasurement {
            route_id: route_id.into(),
            target: target.into(),
            method: ProbeMethod::Icmp,
            sent: samples_ms.len(),
            samples_ms: samples_ms.to_vec(),
        }
    }

    #[test]
    fn score_includes_rtt_deviation_and_loss() {
        let metrics = summarize(measurement(
            "direct",
            "game.example",
            &[Some(20.0), Some(22.0), Some(24.0), None],
        ))
        .unwrap();

        assert_eq!(metrics.received, 3);
        assert_eq!(metrics.loss_percent, 25.0);
        assert_eq!(metrics.median_rtt_ms, 22.0);
        assert_eq!(metrics.p95_deviation_ms, 2.0);
        assert_eq!(metrics.score_ms, 276.0);
    }

    #[test]
    fn comparison_requires_same_target_and_method() {
        let direct = measurement("direct", "game.example", &[Some(20.0), Some(22.0)]);
        let other_target = measurement("relay", "other.example", &[Some(10.0), Some(12.0)]);
        assert_eq!(
            compare(direct.clone(), other_target),
            Err(ScoreError::IncomparableTarget)
        );

        let mut other_method = measurement("relay", "game.example", &[Some(10.0), Some(12.0)]);
        other_method.method = ProbeMethod::UdpEcho;
        assert_eq!(
            compare(direct, other_method),
            Err(ScoreError::IncomparableMethod)
        );
    }

    #[test]
    fn lower_scored_route_has_positive_improvement() {
        let comparison = compare(
            measurement(
                "direct",
                "game.example",
                &[Some(50.0), Some(52.0), Some(48.0)],
            ),
            measurement(
                "relay-eu",
                "game.example",
                &[Some(35.0), Some(37.0), Some(36.0)],
            ),
        )
        .unwrap();

        assert!(comparison.improvement_ms > 0.0);
        assert!(comparison.candidate.score_ms < comparison.direct.score_ms);
    }

    #[test]
    fn median_averages_middle_pair_for_even_sample_count() {
        let metrics = summarize(measurement(
            "direct",
            "game.example",
            &[Some(10.0), Some(12.0), Some(14.0), Some(16.0)],
        ))
        .unwrap();

        assert_eq!(metrics.median_rtt_ms, 13.0);
        assert_eq!(metrics.p95_deviation_ms, 3.0);
    }

    #[test]
    fn rejects_missing_or_invalid_samples() {
        assert_eq!(
            summarize(measurement("direct", "game.example", &[None, None])),
            Err(ScoreError::NoValidSamples)
        );
        assert_eq!(
            summarize(measurement(
                "direct",
                "game.example",
                &[Some(f64::INFINITY)]
            )),
            Err(ScoreError::InvalidSample)
        );
    }
}
