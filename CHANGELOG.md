# Changelog

## 0.1.1 — 2026-10-05

### Architecture

- Added the Phase 7 relay architecture and local implementation foundation: authenticated WireGuard protocol tests, signed node manifests and session grants, measured route scoring, scoped tunnel configuration, recovery boundaries, privacy limits, and implementation acceptance gates. There is no production relay service, Windows route adapter, or deployed node.
- Added an isolated route measurement/scoring model that combines median RTT, p95 deviation, and packet loss; rejects comparisons with different targets or probe methods; and recommends a candidate only after three healthy agreeing windows. Failback requires sustained regression or unhealthy loss. The policy only returns a recommendation and is not connected to live measurements, route changes, or the UI.
- Added a best-route chooser that ranks only candidates which pass the stability and packet-loss gates, using their mean score to the same target. Geographic proximity is not used as a substitute for measured game-server latency. No route is changed by this code.
- Added bounded local persistence for raw-paired route comparison results. Storage re-scores the inputs and keeps the newest 1,000 records; it is not connected to a live probe producer or UI.
- Added signed relay-manifest verification using pinned Ed25519 keys, a domain-separated signature, strict schema/node validation, expiry and clock-skew checks, generation rollback protection hooks, and payload size limits. No trust keys, manifest service, or node list is distributed in this release.
- Added an HTTPS-only manifest fetch path with exact-host configuration, redirects disabled, bounded response streaming, and SQLite generation/cache persistence. Cached envelopes are re-verified before reuse. VYRE ships no endpoint or production trust keys, so the path is intentionally not invoked or shown in the UI.
- Added an in-memory WireGuard profile builder that accepts only a verified relay node and consumed short-lived session grant, supports IPv4/IPv6, rejects broad/default routes, and zeroizes private key/profile buffers on drop. It does not install a Windows service or change routes; no relay endpoint or session issuer is available yet.
- Added a durable relay lifecycle journal with ordered setup phases, a protected opaque restore snapshot, and failure retention until restore is confirmed. Startup recovery can enumerate interrupted sessions; no Windows route restore adapter is connected yet.
- Added a relay coordinator that orders tunnel startup, scoped route activation, and rollback behind an injectable backend. A heartbeat watchdog, startup recovery, and retryable restore failures are covered by fake-backend tests; no production Windows backend is connected.
- Added a test-only Cloudflare BoringTun client/relay exchange over ephemeral loopback UDP sockets. It completes a WireGuard handshake, encrypts a valid IPv4 UDP packet, verifies the relay can decrypt it, and returns an encrypted echo; no virtual adapter or host route is created.
- Added signed, short-lived relay session grants bound to a verified node, client WireGuard public key, and exact allowed destination prefixes. Grants are persisted as one-time consumed before a tunnel lease can be created; expiry, scope mismatch, key mismatch, replay after restart, and broad routes are tested. No production trust key, issuer, server-side revocation service, or relay endpoint is configured.

### Added

- Running Steam games are now matched automatically by installed library path and App ID from Steam's local manifests, including titles absent from VYRE's previous hard-coded process list. Steam names/App IDs are read locally; VYRE does not access Steam account credentials or ownership data.
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
