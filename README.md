# vyre 0.1.1

![vyre logo](src/assets/vyre-logo.png)

vyre is a Windows 10/11 desktop application for measured gaming connection and system diagnostics. It uses Tauri 2, React, TypeScript, Rust, and a local SQLite database. Version 0.1.1 has working diagnostics and a reversible process-priority action; advanced features are listed below.

See the [changelog](CHANGELOG.md) for release changes and current limits.

## Run

Install the supplied NSIS setup program, or build from source:

```powershell
pnpm install
pnpm tauri dev
```

For a distributable Windows installer:

```powershell
pnpm tauri build
```

Rust with the MSVC Windows target and WebView2 are required for source builds. The installer bundles the application, not a relay service.

## Architecture

```text
src/                         React interface
  main.tsx                   Live dashboard and working controls
  LiveModules.tsx            Performance, Traffic, Smart Route, Optimizations
  style.css                  Theme
src-tauri/src/
  lib.rs                     Command API, session workflow, restore coordinator
  network.rs                 Windows ICMP ping probe and score
  system.rs                  Process, CPU, RAM, adapter byte counters, game detection
  steam_games.rs             Installed Steam game recognition and App ID matching
  priority.rs                Windows process priority API wrapper
  relay.rs                   Route scoring and stability-gated recommendations
  relay_client.rs            Pinned-host HTTPS manifest fetch/cache boundary
  relay_manifest.rs          Pinned-key signed relay-manifest validation
  relay_session.rs            Signed, scoped session grants and replay protection
  relay_tunnel.rs             Narrow WireGuard profile construction
  relay_coordinator.rs        Testable lifecycle orchestration and rollback
  relay_interop.rs            Loopback-only WireGuard protocol test
  routing.rs                 Direct route trace through Windows tracert
  traffic.rs                 TCP connection ownership through Windows netstat
  storage.rs                 SQLite sessions, custom games, restore journal
docs/relay-architecture.md   Relay security, routing, recovery, and implementation gates
src-tauri/capabilities/      Tauri window capability
src-tauri/icons/             Application icons
```

The interface invokes only named Rust commands. It does not expose a general shell command, inject into games, edit packets, modify game files, or require permanent administrator access. SQLite lives in vyre's Windows application data directory. Diagnostic reports are written to the `reports` directory next to that database.

## Available now

- Live probe ping, jitter, and packet loss to **1.1.1.1** using Windows `ping.exe`. These values are **not game-server latency**. Parse failures are shown as unavailable.
- CPU, RAM, whole-system network transfer rates, and highest-CPU processes from `sysinfo`.
- Performance page with CPU, RAM, per-process CPU and memory readings, and an on-demand 15-second PresentMon capture for a selected game. Capture shows average FPS, percentile-derived 1% and 0.1% lows, frame-time percentiles and deviation, long-frame spike and dropped-frame counts, plus per-frame CPU busy, GPU work, and display latency when those metrics are available. The raw CSV is kept in the app data `frames` folder.
- Traffic page with adapter transfer rates and current TCP connection ownership by process. Connection counts are not per-process bandwidth usage.
- Smart Route page with measured direct-route quality and a Windows trace to the probe target. No relay route is advertised.
- Optimizations page with working Safe and Competitive controls, a detected-game selector, benchmark result, and restore action.
- Safe and Competitive mode preference saved per game. The selection loads for the active game; BOOST GAME remains the explicit apply action.
- Automatic detection for common game EXE names, running Steam games from the local Steam library manifests (with their Steam App IDs), and custom EXE paths. Minecraft Java is reported only when its command line identifies Minecraft. No Steam credentials or account ownership data are read.
- Safe mode: baseline and follow-up probes with no system change.
- Competitive mode: captures a 15-second frame baseline, saves the original priority, and temporarily tests **Above Normal**. A second PresentMon capture must show at least a 3% improvement in 1% low while average FPS stays within 2% of baseline; otherwise VYRE restores the original priority. Failed after-capture also triggers restore. A SQLite journal supports restore on game close, normal exit, manual restore, or next launch after a crash.
- Before/after probe results and session history in local SQLite.
- Lag Doctor: a 30-probe diagnosis of loss, jitter, CPU, and RAM, with evidence and limitations.
- JSON diagnostic report export.
- Local SQLite history for up to 100 recent frame captures, with average FPS and low-percentile comparisons to each game's prior capture, plus frame-time percentiles and spike counts.
- Optional centered, always-on-top performance overlay with live probe/network and system readings. FPS and 1% low show the most recent on-demand PresentMon capture and its age; this is not a continuous FPS feed.
- The window opens centered. The icon is a V.

The network score is a simple local indicator based on probe ping, jitter, and loss. It is not an FPS score or a validated prediction of game performance. Process priority cannot shorten the internet route; before/after probe changes may be ordinary variation. The current probe still targets `1.1.1.1`, not an active game server, and VYRE does not reserve ISP bandwidth. Steam game names and App IDs come from local Steam install manifests; [SteamDB says it has no public API and does not allow automated scraping](https://steamdb.info/faq/), so VYRE does not scrape its site. The local App ID can be looked up on SteamDB by its app page.

## Planned, not active

GPU/VRAM/temperature sensors, per-process bandwidth, QoS and bandwidth shaping, bufferbloat under load, MTU and DNS tests, relay nodes, game-only tunneling, privileged Windows service, updater, and VYRE code signing. Existing pages identify these limits without displaying invented readings or controls that appear to work.

Frame capture bundles PresentMon 2.6.0 from the official [GameTechDev/PresentMon release](https://github.com/GameTechDev/PresentMon/releases/tag/v2.6.0) and includes its MIT license at `src-tauri/resources/PresentMon-LICENSE.txt`. PresentMon records displayed frame timing through Windows event tracing; Windows permissions and game protection can prevent capture. VYRE does not auto-elevate.

Future privileged features should run in a separate least-privilege Windows service. The service should authenticate a local IPC client, validate an allowlisted command schema, persist a write-ahead restore journal before changes, and reject operations on protected processes. The relay design and implementation gates are in [docs/relay-architecture.md](docs/relay-architecture.md). VYRE has test-only WireGuard interoperability and local relay-security foundations, but no production control plane, Windows route adapter, deployed nodes, live route-benchmark controls, or active relay UI.

## Safety and privacy

The app records no account credentials, packet contents, or private files. Custom EXE paths are stored locally for matching; exported reports include detected game names but no EXE paths. Windows may deny priority changes to protected games, in which case vyre reports the error and applies no change. Do not run vyre as administrator to force access to an anti-cheat protected process.

## Verification

`pnpm build`, `cargo check`, and `pnpm tauri build` pass on Windows. The packaged app was launched and its live dashboard rendered with actual readings. The installer is unsigned; signing is a future release step.
