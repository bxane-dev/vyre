# Changelog

## Unreleased

### Added

- Persist up to 100 frame capture summaries in local SQLite and compare each game capture with its previous sample.
- 15-second PresentMon capture for a selected running game, with average FPS, percentile-derived 1% and 0.1% lows, and saved raw CSV.
- Bundled, Authenticode-signed PresentMon 2.6.0 console executable and its MIT license. Capture runs at the current user's privilege and reports Windows trace access failures.

### Limits

- Frame capture requires Windows to allow the current user to start an ETW trace. Some protected games or accounts may deny it. No automatic elevation is requested.
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
