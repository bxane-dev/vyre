# Changelog

## Unreleased

### Architecture

- Added the Phase 7 relay architecture: authenticated WireGuard data plane, signed node manifests, measured route scoring, Windows service/WFP boundaries, failure recovery, privacy limits, and implementation acceptance gates. This is a design baseline; VYRE still has no relay implementation or deployed nodes.
- Added an isolated route measurement/scoring model that combines median RTT, p95 deviation, and packet loss and rejects comparisons with different targets or probe methods. It is not connected to live relay measurements or the UI.

### Added

- Optional centered, always-on-top overlay for live probe/network and system readings, with the latest PresentMon FPS and 1% low clearly identified as a timestamped capture.
- Competitive BOOST now captures 15-second PresentMon samples before and after a temporary priority change. It keeps the change only when the 1% low improves by at least 3% and average FPS stays within 2%; it restores the original value when the threshold is missed or after-capture fails.
- Frame analysis now reports average and p95 frame time, frame-time deviation, long-frame spikes, dropped frames, average CPU busy time, GPU work time, and display latency when PresentMon provides those columns.
- SQLite-backed Safe and Competitive mode preferences per game. Selecting a detected game loads its saved profile; applying the profile remains a manual action.
- Persist up to 100 frame capture summaries in local SQLite and compare each game capture with its previous sample.
- 15-second PresentMon capture for a selected running game, with average FPS, percentile-derived 1% and 0.1% lows, and saved raw CSV.
- Bundled, Authenticode-signed PresentMon 2.6.0 console executable and its MIT license. Capture runs at the current user's privilege and reports Windows trace access failures.

### Limits

- Frame capture requires Windows to allow the current user to start an ETW trace. Some protected games or accounts may deny it. No automatic elevation is requested.
- FPS in the overlay comes from the latest on-demand PresentMon capture; continuous frame-rate monitoring is not included.
- GPU utilization, temperatures, and persistent frame history are not included in this capture milestone.

## 0.1.0 — 2026-10-04

Initial Windows desktop release.

### Added

- Tauri, React, TypeScript, and Rust application with a centered window, V app icon, and vyre wordmark.
- Live ICMP probe measurements to `1.1.1.1`: average ping, jitter, packet loss, and a simple network stability score.
- CPU, RAM, adapter transfer-rate, and process activity views.
- Traffic view with current TCP connections and their owning processes.
- Direct-route trace using Windows `tracert`.
- Detection of common game processes and support for custom EXE paths.
- Safe mode for measurement without system changes.
- Competitive mode that temporarily sets a detected game process to Above Normal priority.
- SQLite restore journal, manual restore, and restore on game close, normal exit, or the next launch after a crash.
- Before-and-after probe results, local session history, a 30-probe Lag Doctor check, and JSON diagnostic report export.
- Windows installer and portable executable.

### Current limits

- Probe latency is measured to `1.1.1.1`, not to a game server. A process-priority change does not change the internet route, and changes in probe results do not prove a ping improvement.
- Relay routing has no deployed nodes. PresentMon FPS and frame-time capture, GPU and VRAM monitoring, per-process bandwidth rates, traffic shaping, and QoS are not implemented.
- The installer is unsigned.
