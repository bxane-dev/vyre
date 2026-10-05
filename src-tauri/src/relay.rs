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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteSelectionPolicy {
    pub minimum_improvement_ms: f64,
    pub maximum_candidate_loss_percent: f64,
    pub agreeing_windows: usize,
}

impl Default for RouteSelectionPolicy {
    fn default() -> Self {
        Self {
            minimum_improvement_ms: 5.0,
            maximum_candidate_loss_percent: 1.0,
            agreeing_windows: 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RouteRecommendation {
    NeedMoreComparableMeasurements,
    KeepDirect,
    Candidate(RouteMetrics),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BestRouteCandidate {
    pub route_id: String,
    pub target: String,
    pub mean_score_ms: f64,
    pub mean_improvement_ms: f64,
    pub agreeing_windows: usize,
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

/// Recommend a candidate only when the latest configured number of comparable
/// windows all agree, it clears the improvement margin, and loss stays under
/// the health limit. This returns evidence, never changes a route.
pub fn recommend_candidate(
    recent_windows: &[RouteComparison],
    policy: RouteSelectionPolicy,
) -> RouteRecommendation {
    if policy.agreeing_windows == 0
        || !policy.minimum_improvement_ms.is_finite()
        || policy.minimum_improvement_ms < 0.0
        || !policy.maximum_candidate_loss_percent.is_finite()
        || !(0.0..=100.0).contains(&policy.maximum_candidate_loss_percent)
    {
        return RouteRecommendation::KeepDirect;
    }
    if recent_windows.len() < policy.agreeing_windows {
        return RouteRecommendation::NeedMoreComparableMeasurements;
    }
    let windows = &recent_windows[recent_windows.len() - policy.agreeing_windows..];
    let first = &windows[0];
    let candidate_id = &first.candidate.route_id;
    let target = &first.direct.target;
    let method = &first.direct.method;
    let consistent = windows.iter().all(|window| {
        window.direct.route_id == first.direct.route_id
            && window.candidate.route_id == *candidate_id
            && window.direct.target.eq_ignore_ascii_case(target)
            && window.candidate.target.eq_ignore_ascii_case(target)
            && window.direct.method == *method
            && window.candidate.method == *method
            && window.improvement_ms.is_finite()
            && window.direct.sent > 0
            && window.candidate.sent > 0
            && window.direct.received <= window.direct.sent
            && window.candidate.received <= window.candidate.sent
            && valid_metrics(&window.direct)
            && valid_metrics(&window.candidate)
            && (window.improvement_ms - (window.direct.score_ms - window.candidate.score_ms)).abs()
                <= 0.01
    });
    if !consistent {
        return RouteRecommendation::KeepDirect;
    }
    if windows.iter().any(|window| {
        window.improvement_ms < policy.minimum_improvement_ms
            || window.candidate.loss_percent > policy.maximum_candidate_loss_percent
    }) {
        return RouteRecommendation::KeepDirect;
    }
    RouteRecommendation::Candidate(first.candidate.clone())
}

/// Pick the lowest-scoring healthy route only after each candidate has passed
/// the same-target, repeated-window policy against the direct path. This uses
/// measured game-server latency, not geographic distance, and never changes a
/// route itself.
pub fn recommend_best_candidate(
    candidate_windows: &[Vec<RouteComparison>],
    policy: RouteSelectionPolicy,
) -> Option<BestRouteCandidate> {
    let mut eligible = candidate_windows
        .iter()
        .filter_map(|windows| {
            let RouteRecommendation::Candidate(candidate) = recommend_candidate(windows, policy)
            else {
                return None;
            };
            let recent = &windows[windows.len() - policy.agreeing_windows..];
            let mean_score_ms = recent
                .iter()
                .map(|window| window.candidate.score_ms)
                .sum::<f64>()
                / recent.len() as f64;
            let mean_improvement_ms = recent
                .iter()
                .map(|window| window.improvement_ms)
                .sum::<f64>()
                / recent.len() as f64;
            Some(BestRouteCandidate {
                route_id: candidate.route_id,
                target: candidate.target,
                mean_score_ms,
                mean_improvement_ms,
                agreeing_windows: recent.len(),
            })
        })
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| {
        left.mean_score_ms
            .total_cmp(&right.mean_score_ms)
            .then_with(|| left.route_id.cmp(&right.route_id))
    });
    eligible.into_iter().next()
}

/// Require sustained candidate regression or unhealthy loss before suggesting
/// failback. A single bad measurement never triggers an automatic transition.
pub fn should_fail_back(
    recent_windows: &[RouteComparison],
    regression_margin_ms: f64,
    maximum_loss_percent: f64,
    agreeing_windows: usize,
) -> bool {
    if agreeing_windows == 0
        || recent_windows.len() < agreeing_windows
        || !regression_margin_ms.is_finite()
        || regression_margin_ms < 0.0
        || !maximum_loss_percent.is_finite()
        || !(0.0..=100.0).contains(&maximum_loss_percent)
    {
        return false;
    }
    let windows = &recent_windows[recent_windows.len() - agreeing_windows..];
    let first = &windows[0];
    windows.iter().all(|window| {
        window.direct.route_id == first.direct.route_id
            && window
                .direct
                .target
                .eq_ignore_ascii_case(&first.direct.target)
            && window
                .candidate
                .target
                .eq_ignore_ascii_case(&first.direct.target)
            && window.direct.method == first.direct.method
            && window.candidate.method == first.direct.method
            && window.candidate.route_id == first.candidate.route_id
            && window.improvement_ms.is_finite()
            && valid_metrics(&window.direct)
            && valid_metrics(&window.candidate)
            && (window.improvement_ms - (window.direct.score_ms - window.candidate.score_ms)).abs()
                <= 0.01
            && (window.improvement_ms <= -regression_margin_ms
                || window.candidate.loss_percent > maximum_loss_percent)
    })
}

fn valid_metrics(metrics: &RouteMetrics) -> bool {
    metrics.sent > 0
        && metrics.received <= metrics.sent
        && metrics.loss_percent.is_finite()
        && (0.0..=100.0).contains(&metrics.loss_percent)
        && metrics.median_rtt_ms.is_finite()
        && metrics.median_rtt_ms >= 0.0
        && metrics.p95_deviation_ms.is_finite()
        && metrics.p95_deviation_ms >= 0.0
        && metrics.score_ms.is_finite()
        && metrics.score_ms >= 0.0
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

    fn window(direct: &[Option<f64>], candidate: &[Option<f64>]) -> RouteComparison {
        compare(
            measurement("direct", "game.example", direct),
            measurement("relay-eu", "game.example", candidate),
        )
        .unwrap()
    }

    #[test]
    fn auto_recommendation_requires_three_agreeing_healthy_windows() {
        let policy = RouteSelectionPolicy::default();
        let windows = vec![
            window(&[Some(50.0), Some(52.0)], &[Some(35.0), Some(37.0)]),
            window(&[Some(48.0), Some(50.0)], &[Some(34.0), Some(35.0)]),
            window(&[Some(49.0), Some(51.0)], &[Some(36.0), Some(37.0)]),
        ];
        assert_eq!(
            recommend_candidate(&windows[..2], policy),
            RouteRecommendation::NeedMoreComparableMeasurements
        );
        assert!(matches!(
            recommend_candidate(&windows, policy),
            RouteRecommendation::Candidate(metrics) if metrics.route_id == "relay-eu"
        ));
    }

    #[test]
    fn auto_recommendation_keeps_direct_when_any_window_disagrees_or_loses_packets() {
        let policy = RouteSelectionPolicy::default();
        let windows = vec![
            window(&[Some(50.0), Some(52.0)], &[Some(35.0), Some(37.0)]),
            window(&[Some(48.0), Some(50.0)], &[Some(34.0), Some(35.0)]),
            window(&[Some(49.0), Some(51.0)], &[Some(50.0), Some(52.0)]),
        ];
        assert_eq!(
            recommend_candidate(&windows, policy),
            RouteRecommendation::KeepDirect
        );

        let lossy = vec![
            window(&[Some(50.0), Some(52.0)], &[Some(35.0), Some(37.0), None]),
            window(&[Some(48.0), Some(50.0)], &[Some(34.0), Some(35.0), None]),
            window(&[Some(49.0), Some(51.0)], &[Some(36.0), Some(37.0), None]),
        ];
        assert_eq!(
            recommend_candidate(&lossy, policy),
            RouteRecommendation::KeepDirect
        );
    }

    #[test]
    fn failback_requires_sustained_regression_or_unhealthy_loss() {
        let regressions = vec![
            window(&[Some(30.0), Some(31.0)], &[Some(40.0), Some(41.0)]),
            window(&[Some(30.0), Some(31.0)], &[Some(42.0), Some(43.0)]),
            window(&[Some(30.0), Some(31.0)], &[Some(44.0), Some(45.0)]),
        ];
        assert!(should_fail_back(&regressions, 5.0, 1.0, 3));
        assert!(!should_fail_back(&regressions[..2], 5.0, 1.0, 3));
    }

    #[test]
    fn best_route_is_the_lowest_measured_healthy_score_for_the_game_target() {
        let windows = |candidate_id: &str, values: &[(f64, f64)]| {
            values
                .iter()
                .map(|(direct, candidate)| {
                    compare(
                        measurement(
                            "direct",
                            "198.51.100.25",
                            &[Some(*direct), Some(*direct + 1.0)],
                        ),
                        measurement(
                            candidate_id,
                            "198.51.100.25",
                            &[Some(*candidate), Some(*candidate + 1.0)],
                        ),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>()
        };
        let slower = windows("relay-nearby", &[(80.0, 55.0), (82.0, 56.0), (81.0, 54.0)]);
        let faster = windows(
            "relay-lowest-rtt",
            &[(80.0, 35.0), (82.0, 36.0), (81.0, 34.0)],
        );
        let best =
            recommend_best_candidate(&[slower, faster], RouteSelectionPolicy::default()).unwrap();
        assert_eq!(best.route_id, "relay-lowest-rtt");
        assert_eq!(best.target, "198.51.100.25");
        assert_eq!(best.agreeing_windows, 3);
    }
}
