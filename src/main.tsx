import React, { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Activity, ArrowDownRight, ArrowRight, ArrowUpRight, BarChart3, CheckCircle2, ChevronDown, CircleHelp, Clock3, Cpu, Crosshair, Gauge, Gamepad2, HeartPulse, History, LayoutDashboard, MemoryStick, Network, Plus, RotateCcw, Route, Settings2, ShieldCheck, Signal, SlidersHorizontal, Sparkles, Wifi, X } from 'lucide-react';
import LiveModules from './LiveModules';
import vyreLogo from './assets/vyre-logo.png';
import './style.css';

export type Probe = { target: string; sent: number; received: number; averageMs: number | null; bestMs: number | null; worstMs: number | null; jitterMs: number | null; lossPercent: number | null; samplesMs: (number | null)[]; error: string | null };
export type Game = { name: string; exe: string; pid: number; startedAt: number; cpuPercent: number; memoryMb: number; custom: boolean; steamAppId: number | null };
type Process = { name: string; pid: number; cpuPercent: number; memoryMb: number };
type Adapter = { name: string; downloadMbps: number; uploadMbps: number };
export type Connection = { pid: number; process: string; local: string; remote: string; state: string };
export type Machine = { cpuPercent: number; ramPercent: number; ramUsedGb: number; ramTotalGb: number; downloadMbps: number; uploadMbps: number; games: Game[]; topProcesses: Process[]; adapters: Adapter[] };
type Snapshot = { at: string; machine: Machine; probe: Probe; networkScore: number | null; mode: string; activeChanges: number };
export type Benchmark = { game: string; mode: string; before: Probe; after: Probe; beforeScore: number | null; afterScore: number | null; change: string; warning: string | null; frameBefore: FrameCapture | null; frameAfter: FrameCapture | null };
type Diagnosis = { probe: Probe; score: number | null; primaryProblem: string; evidence: string; recommendation: string; cpuPercent: number; ramPercent: number };
type Session = { id: number; at: string; game: string; mode: string; beforeJson: string; afterJson: string; change: string };
type FrameCapture = { game: string; pid: number; frameCount: number; averageFps: number; onePercentLow: number; pointOnePercentLow: number; averageFrameTimeMs: number; p95FrameTimeMs: number; frameTimeStdDevMs: number; frameTimeSpikes: number; droppedFrames: number; averageCpuBusyMs: number | null; averageGpuTimeMs: number | null; averageDisplayLatencyMs: number | null; captureSeconds: number; csvPath: string };
type FrameSession = FrameCapture & { id: number; at: string; analysisVersion: number };

const sections = [
  { title: 'Dashboard', icon: LayoutDashboard }, { title: 'Games', icon: Gamepad2 }, { title: 'Ping Optimizer', icon: Activity },
  { title: 'Smart Route', icon: Route }, { title: 'Traffic', icon: Network }, { title: 'Performance', icon: Gauge },
  { title: 'Lag Doctor', icon: HeartPulse }, { title: 'Optimizations', icon: SlidersHorizontal },
  { title: 'Statistics', icon: BarChart3 }, { title: 'History', icon: History }, { title: 'Settings', icon: Settings2 }, { title: 'Restore', icon: RotateCcw },
];
const units = (value: number | null | undefined, suffix: string, digits = 1) => value == null ? '—' : `${value.toFixed(digits)}${suffix}`;
const pct = (value: number | null | undefined) => units(value, '%', 0);
function Sparkline({ samples }: { samples: (number | null)[] }) {
  const valid = samples.filter((value): value is number => value != null);
  if (valid.length < 2) return <div className="empty-chart">Waiting for valid probe samples</div>;
  const min = Math.min(...valid), max = Math.max(...valid), range = Math.max(max - min, 4);
  const points = samples.map((value, index) => value == null ? null : `${20 + index * 560 / Math.max(samples.length - 1, 1)},${125 - ((value - min) / range) * 85}`).filter(Boolean).join(' ');
  return <svg className="sparkline" viewBox="0 0 600 150" preserveAspectRatio="none" role="img" aria-label="Measured latency samples"><defs><linearGradient id="area" x1="0" y1="0" x2="0" y2="1"><stop stopColor="#bcf66b" stopOpacity=".23"/><stop offset="1" stopColor="#bcf66b" stopOpacity="0"/></linearGradient></defs><line x1="0" y1="125" x2="600" y2="125" stroke="#25312b" strokeDasharray="4 6"/><polyline points={points} fill="none" stroke="#bcf66b" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round"/><text x="4" y="20" fill="#768078" fontSize="12">{max.toFixed(1)} ms</text><text x="4" y="143" fill="#768078" fontSize="12">{min.toFixed(1)} ms</text></svg>;
}
function Metric({ icon: Icon, label, value, note, accent = false }: { icon: React.ComponentType<{ size?: number }>; label: string; value: string; note: string; accent?: boolean }) {
  return <div className={`metric ${accent ? 'metric-accent' : ''}`}><div className="metric-top"><span>{label}</span><Icon size={17}/></div><strong>{value}</strong><small>{note}</small></div>;
}
function App() {
  const [section, setSection] = useState('Dashboard');
  const [data, setData] = useState<Snapshot | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Benchmark | null>(null);
  const [diagnosis, setDiagnosis] = useState<Diagnosis | null>(null);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [frameSessions, setFrameSessions] = useState<FrameSession[]>([]);
  const [selectedPid, setSelectedPid] = useState<number | null>(null);
  const [profileMode, setProfileMode] = useState('Safe');
  const [gameName, setGameName] = useState('');
  const [gameExe, setGameExe] = useState('');
  const [showGameForm, setShowGameForm] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [samples, setSamples] = useState<(number | null)[]>([]);
  const [connections, setConnections] = useState<Connection[]>([]);
  const [trace, setTrace] = useState<string | null>(null);
  const [frameCapture, setFrameCapture] = useState<FrameCapture | null>(null);
  const isTauri = '__TAURI_INTERNALS__' in window;
  async function refresh() {
    try {
      const next = await invoke<Snapshot>('snapshot');
      setData(next); setError(null);
      setSamples(previous => [...previous, next.probe.averageMs].slice(-32));
      setSelectedPid(current => next.machine.games.some(game => game.pid === current) ? current : (next.machine.games[0]?.pid ?? null));
    } catch (cause) { setError(String(cause)); }
  }
  async function loadHistory() { try { const [network, frames] = await Promise.all([invoke<Session[]>('history'), invoke<FrameSession[]>('frame_history')]); setSessions(network); setFrameSessions(frames); } catch (cause) { setError(String(cause)); } }
  useEffect(() => { if (!isTauri) return; void refresh(); const timer = window.setInterval(() => { if (!busy) void refresh(); }, 4000); return () => window.clearInterval(timer); }, [busy, isTauri]);
  useEffect(() => { if (section === 'History' || section === 'Statistics') void loadHistory(); }, [section]);
  useEffect(() => { if (section === 'Traffic' && isTauri) void refreshConnections(); }, [section, isTauri]);
  useEffect(() => { if (selectedPid == null) { setProfileMode(data?.mode ?? 'Safe'); return; } let active = true; void invoke<string>('get_game_profile', { pid: selectedPid }).then(mode => { if (active) setProfileMode(mode); }).catch(cause => { if (active) setError(String(cause)); }); return () => { active = false; }; }, [selectedPid, isTauri]);
  const activeGame = data?.machine.games.find(game => game.pid === selectedPid) ?? null;
  const title = section === 'Dashboard' ? 'Your game, in focus.' : section;
  const subtitles: Record<string, string> = {
    Dashboard: 'Live signals from your PC and connection.',
    Performance: 'CPU, memory, and process activity measured on this PC.',
    Traffic: 'Adapter throughput and current TCP connection ownership.',
    'Smart Route': 'Inspect the direct network path to the probe target.',
    Optimizations: 'Measured profile controls and temporary changes.',
    'Lag Doctor': 'A 30-second evidence-based connection check.',
    Restore: 'Review and undo temporary system changes.',
  };
  const subtitle = subtitles[section] ?? 'Measured data and controls.';
  const stateTag = error ? 'Connection issue' : data ? 'Monitoring live' : 'Waiting for backend';

  async function runBoost() {
    if (!selectedPid) return;
    setBusy(profileMode === 'Competitive' ? 'Measuring baseline and checking the profile (up to 45 seconds)…' : 'Benchmarking in Safe mode…'); setError(null); setNotice(null);
    try { setResult(await invoke<Benchmark>('boost_game', { pid: selectedPid })); await refresh(); await loadHistory(); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(null); }
  }
  async function runDoctor() {
    setBusy('Lag Doctor is measuring 30 probes…'); setError(null);
    try { setDiagnosis(await invoke<Diagnosis>('run_diagnostic')); await refresh(); } catch (cause) { setError(String(cause)); } finally { setBusy(null); }
  }
  async function changeMode(mode: string) {
    try { if (selectedPid != null) await invoke('set_game_profile', { pid: selectedPid, mode }); else await invoke('set_mode', { mode }); setProfileMode(mode); setNotice(`${mode} profile saved${activeGame ? ` for ${activeGame.name}` : ''}.`); if (selectedPid == null) await refresh(); } catch (cause) { setError(String(cause)); }
  }
  async function saveGame(event: React.FormEvent) {
    event.preventDefault(); setError(null);
    try { await invoke('add_custom_game', { name: gameName, exe: gameExe }); setNotice('Custom game added. It will appear when the EXE runs.'); setGameName(''); setGameExe(''); setShowGameForm(false); await refresh(); }
    catch (cause) { setError(String(cause)); }
  }
  async function restore() {
    setBusy('Restoring original settings…');
    try { const count = await invoke<number>('restore_everything'); setNotice(`${count} running process priority ${count === 1 ? 'change' : 'changes'} restored.`); await refresh(); }
    catch (cause) { setError(String(cause)); } finally { setBusy(null); }
  }
  async function exportReport() {
    setBusy('Collecting diagnostic report…');
    try { const path = await invoke<string>('export_report'); setNotice(`Diagnostic report saved to ${path}`); }
    catch (cause) { setError(String(cause)); } finally { setBusy(null); }
  }
  async function refreshConnections() {
    setBusy('Inspecting TCP connections…');
    try { setConnections(await invoke<Connection[]>('traffic_connections')); setError(null); }
    catch (cause) { setError(String(cause)); } finally { setBusy(null); }
  }
  async function runTrace() {
    setBusy('Tracing the direct route…'); setTrace(null);
    try { setTrace(await invoke<string>('trace_route')); setError(null); }
    catch (cause) { setError(String(cause)); } finally { setBusy(null); }
  }
  async function captureFrames() {
    if (!selectedPid) return;
    setBusy('Capturing 15 seconds of game frame data…'); setError(null);
    try { setFrameCapture(await invoke<FrameCapture>('capture_frames', { pid: selectedPid })); if (section === 'History' || section === 'Statistics') await loadHistory(); }
    catch (cause) { setError(String(cause)); }
    finally { setBusy(null); }
  }
  async function toggleOverlay() {
    try { const open = await invoke<boolean>('toggle_overlay'); setNotice(open ? 'Performance overlay opened.' : 'Performance overlay closed.'); }
    catch (cause) { setError(String(cause)); }
  }
  const historyRows = useMemo(() => sessions.map(session => ({ ...session, before: JSON.parse(session.beforeJson) as Probe, after: JSON.parse(session.afterJson) as Probe })), [sessions]);

  return <div className="app-shell"><aside className="sidebar"><div className="brand"><img className="brand-logo" src={vyreLogo} alt="vyre"/><span className="brand-tagline">PERFORMANCE ENGINE</span></div><div className="side-label">WORKSPACE</div><nav>{sections.map(item => <button key={item.title} className={`nav-item ${section === item.title ? 'active' : ''}`} onClick={() => setSection(item.title)}><item.icon size={18}/><span>{item.title}</span></button>)}</nav><div className="side-bottom"><div className="safety-card"><ShieldCheck size={18}/><div><b>Safe by design</b><span>External monitoring. Reversible changes.</span></div></div><div className="version">VYRE 0.1.1</div></div></aside>
    <main className="main"><header className="topbar"><div className="breadcrumb">OVERVIEW <span>/</span> {section.toUpperCase()}</div><div className="top-actions"><span className={`live-dot ${data && !error ? 'on' : ''}`}/><span>{stateTag}</span><div className="top-divider"/><span className="mode-pill"><ShieldCheck size={13}/>{profileMode} mode</span></div></header><div className="content"><div className="page-heading"><div><div className="eyebrow">VYRE / {section.toUpperCase()}</div><h1>{title}</h1><p>{subtitle}</p></div>{section === 'Dashboard' && <div className="heading-right"><span className="light-dot"/> {activeGame ? `${activeGame.name} detected` : 'No supported game detected'}</div>}</div>
    {!isTauri && <div className="notice warn"><CircleHelp size={17}/> Open this project with <code>pnpm tauri dev</code> on Windows to connect the live backend. Browser preview contains no simulated data.</div>}
    {error && <div className="notice error"><X size={17}/>{error}<button onClick={() => setError(null)} aria-label="Dismiss error"><X size={16}/></button></div>}
    {notice && <div className="notice success"><CheckCircle2 size={17}/>{notice}<button onClick={() => setNotice(null)} aria-label="Dismiss notice"><X size={16}/></button></div>}
    {busy && <div className="notice busy"><span className="spinner"/>{busy}</div>}
    {section === 'Dashboard' && <><div className="hero"><div className="hero-copy"><div className="hero-kicker"><Sparkles size={15}/> SESSION CONTROL</div><h2>{activeGame ? `Ready for ${activeGame.name}` : 'Ready when your game is.'}</h2><p>{activeGame ? 'Benchmark the route to the probe target, apply your selected profile, and see what actually changed.' : 'Launch a supported game or add a custom EXE to enable the game profile.'}</p><div className="hero-actions"><button className="button primary" disabled={!activeGame || !!busy} onClick={runBoost}><Crosshair size={18}/> BOOST GAME <ArrowRight size={17}/></button><button className="button secondary" disabled={!!busy || !isTauri} onClick={() => setSection('Lag Doctor')}>Run Lag Doctor</button></div></div><div className="hero-score"><div className="score-ring" style={{ '--score': `${data?.networkScore ?? 0}%` } as React.CSSProperties}><div><span>NETWORK SCORE</span><strong>{data?.networkScore ?? '—'}</strong><small>/ 100</small></div></div><span>Based on probe latency, jitter & loss</span></div></div>
      <div className="section-heading"><div><h3>Live telemetry</h3><p>Measured locally, refreshed every few seconds</p></div><span>Target: {data?.probe.target ?? '1.1.1.1'} <CircleHelp size={14}/></span></div><div className="metric-grid"><Metric icon={Signal} label="PROBE PING" value={units(data?.probe.averageMs, ' ms')} note="To probe target, not game server" accent/><Metric icon={Activity} label="JITTER" value={units(data?.probe.jitterMs, ' ms')} note="Mean consecutive difference"/><Metric icon={Wifi} label="PACKET LOSS" value={pct(data?.probe.lossPercent)} note={`${data?.probe.received ?? 0}/${data?.probe.sent ?? 0} probes received`}/><Metric icon={Cpu} label="CPU LOAD" value={pct(data?.machine.cpuPercent)} note="System-wide utilization"/><Metric icon={MemoryStick} label="MEMORY" value={pct(data?.machine.ramPercent)} note={data ? `${data.machine.ramUsedGb.toFixed(1)} / ${data.machine.ramTotalGb.toFixed(1)} GB` : 'System RAM'}/><Metric icon={ArrowDownRight} label="DOWNLOAD" value={units(data?.machine.downloadMbps, ' Mbps')} note="All network adapters"/><Metric icon={ArrowUpRight} label="UPLOAD" value={units(data?.machine.uploadMbps, ' Mbps')} note="All network adapters"/><Metric icon={Gauge} label="FPS / FRAME TIME" value={frameCapture ? `${frameCapture.averageFps.toFixed(0)} FPS` : '—'} note={frameCapture ? `${frameCapture.game} · captured ${frameCapture.captureSeconds}s` : 'Capture from Performance page'}/></div>
      <div className="two-column"><div className="panel chart-panel"><div className="panel-head"><div><h3>Latency trend</h3><p>Recent live probe averages</p></div><span className="tag"><span className="live-dot on"/> LIVE</span></div><Sparkline samples={samples}/><div className="chart-foot"><span>Lower is better</span><span>Last {samples.length} samples</span></div></div><div className="panel"><div className="panel-head"><div><h3>System activity</h3><p>Processes using CPU right now</p></div><Cpu size={17}/></div>{data?.machine.topProcesses.length ? <div className="process-list">{data.machine.topProcesses.slice(0, 5).map(process => <div className="process-row" key={process.pid}><div className="process-icon">{process.name[0]?.toUpperCase()}</div><div className="process-name"><b>{process.name}</b><small>PID {process.pid} · {process.memoryMb.toFixed(0)} MB</small></div><strong>{process.cpuPercent.toFixed(1)}%</strong></div>)}</div> : <div className="empty-state">No process data yet.</div>}</div></div>
      {result && <div className="panel result-panel"><div className="panel-head"><div><h3>Latest benchmark · {result.game}</h3><p>{result.change}</p></div><button className="icon-button" onClick={() => setResult(null)} aria-label="Close benchmark"><X size={18}/></button></div><div className="comparison"><div><span>PROBE PING</span><strong>{units(result.before.averageMs, ' ms')} <ArrowRight size={17}/> {units(result.after.averageMs, ' ms')}</strong></div><div><span>JITTER</span><strong>{units(result.before.jitterMs, ' ms')} <ArrowRight size={17}/> {units(result.after.jitterMs, ' ms')}</strong></div><div><span>LOSS</span><strong>{pct(result.before.lossPercent)} <ArrowRight size={17}/> {pct(result.after.lossPercent)}</strong></div><div><span>NETWORK SCORE</span><strong>{result.beforeScore ?? '—'} <ArrowRight size={17}/> {result.afterScore ?? '—'}</strong></div></div><p className="fine-print">The probe tests 1.1.1.1, not the game server. A priority change does not change the internet route; any network difference may be normal variation.</p>{result.warning && <p className="fine-print warning-text">{result.warning}</p>}</div>}</>}
    {section === 'Games' && <><div className="section-toolbar"><div><h3>Detected games</h3><p>Running Steam games are recognized from installed Steam library manifests; supported games and custom EXEs are also detected.</p></div><button className="button secondary" onClick={() => setShowGameForm(!showGameForm)}><Plus size={16}/> Add custom EXE</button></div>{showGameForm && <form className="panel game-form" onSubmit={saveGame}><label>Game name<input value={gameName} onChange={event => setGameName(event.target.value)} placeholder="My game" required/></label><label>Full EXE path<input value={gameExe} onChange={event => setGameExe(event.target.value)} placeholder="C:\Games\MyGame\game.exe" required/></label><button className="button primary" type="submit">Save game</button></form>}{data?.machine.games.length ? <div className="game-grid">{data.machine.games.map(game => <button className={`panel game-card ${selectedPid === game.pid ? 'selected' : ''}`} onClick={() => { setSelectedPid(game.pid); setSection('Dashboard'); }} key={game.pid}><div className="game-avatar"><Gamepad2 size={24}/></div><div><h3>{game.name}</h3><p>{game.exe}</p><span className="tag green">{game.steamAppId != null ? `STEAM · APP ${game.steamAppId}` : `RUNNING · PID ${game.pid}`}</span></div><ChevronDown size={18}/></button>)}</div> : <div className="panel empty-large"><Gamepad2 size={28}/><h3>No game running</h3><p>Start a game from Steam to have VYRE match it automatically, or add a custom EXE for a non-Steam title.</p></div>}<div className="panel info-panel"><h3>Automatic session restore</h3><p>When the selected game closes, vyre restores any priority it changed. A saved journal also restores after a restart of vyre.</p><p className="fine-print">Steam names and App IDs come from the local Steam install manifests. VYRE does not read your Steam account or ownership.</p></div></>}
    {section === 'Ping Optimizer' && <><div className="two-column top"><div className="panel"><div className="panel-head"><div><h3>Connection to probe target</h3><p>Windows ICMP echo · {data?.probe.target ?? '1.1.1.1'}</p></div><Signal size={20}/></div><div className="big-reading">{units(data?.probe.averageMs, ' ms')}<span>average latency</span></div><div className="mini-stats"><div><span>BEST</span><b>{units(data?.probe.bestMs, ' ms')}</b></div><div><span>WORST</span><b>{units(data?.probe.worstMs, ' ms')}</b></div><div><span>JITTER</span><b>{units(data?.probe.jitterMs, ' ms')}</b></div><div><span>LOSS</span><b>{pct(data?.probe.lossPercent)}</b></div></div><button className="button secondary" disabled={!!busy || !isTauri} onClick={() => void refresh()}><RotateCcw size={16}/> Recheck now</button></div><div className="panel chart-panel"><div className="panel-head"><div><h3>Recent latency</h3><p>Each point is a measured probe average</p></div></div><Sparkline samples={samples}/></div></div><div className="panel info-panel"><h3>What this measures</h3><p>This ping measures the round trip from your PC to {data?.probe.target ?? '1.1.1.1'} only. It can help spot local or ISP connection issues, but it is not a game-server ping. VYRE recognizes running Steam games automatically, but game-server measurement and relay routing need game-specific server endpoints and an active Windows route service; they are not enabled yet. VYRE does not reserve bandwidth.</p></div></>}
    {section === 'Lag Doctor' && <><div className="panel doctor-intro"><div className="doctor-icon"><HeartPulse size={30}/></div><div><h3>Find the cause before changing settings.</h3><p>Runs 30 ICMP probes over about 30 seconds and checks current CPU and RAM usage. It reports only problems supported by that evidence.</p></div><button className="button primary" disabled={!!busy || !isTauri} onClick={runDoctor}><Activity size={17}/> Start diagnosis</button></div>{diagnosis && <div className="panel diagnosis"><div className="diagnosis-top"><div><span className="eyebrow">DIAGNOSTIC RESULT</span><h2>{diagnosis.primaryProblem}</h2></div><div className="diagnosis-score">{diagnosis.score ?? '—'}<span>/100</span></div></div><div className="diagnosis-grid"><div><span>AVG PING</span><b>{units(diagnosis.probe.averageMs, ' ms')}</b></div><div><span>JITTER</span><b>{units(diagnosis.probe.jitterMs, ' ms')}</b></div><div><span>LOSS</span><b>{pct(diagnosis.probe.lossPercent)}</b></div><div><span>CPU / RAM</span><b>{pct(diagnosis.cpuPercent)} / {pct(diagnosis.ramPercent)}</b></div></div><div className="diagnosis-detail"><span>EVIDENCE</span><p>{diagnosis.evidence}</p><span>RECOMMENDATION</span><p>{diagnosis.recommendation}</p></div></div>}<div className="panel info-panel"><h3>Diagnostic limits</h3><p>Bufferbloat requires a loaded-link test. Frame-time spikes need PresentMon. Neither is inferred from this idle probe.</p><button className="button secondary report-button" disabled={!!busy || !isTauri} onClick={exportReport}>Export diagnostic report <ArrowRight size={16}/></button></div></>}
    {(section === 'History' || section === 'Statistics') && <><div className="section-toolbar"><div><h3>Gaming sessions</h3><p>Local SQLite records of probe results and measured frame captures.</p></div><span className="tag">{sessions.length} PROBES · {frameSessions.length} FRAME CAPTURES</span></div>{historyRows.length ? <div className="panel history-table"><div className="table-head"><span>GAME / DATE</span><span>MODE</span><span>PING BEFORE</span><span>PING AFTER</span><span>LOSS AFTER</span></div>{historyRows.map(row => <div className="table-row" key={row.id}><div><b>{row.game}</b><small>{new Date(row.at).toLocaleString()}</small></div><span>{row.mode}</span><strong>{units(row.before.averageMs, ' ms')}</strong><strong>{units(row.after.averageMs, ' ms')}</strong><span>{pct(row.after.lossPercent)}</span></div>)}</div> : <div className="panel empty-large"><History size={29}/><h3>No network sessions yet</h3><p>Run BOOST GAME with a detected game to save before and after probe data.</p></div>}
      <div className="section-toolbar frame-history-heading"><div><h3>Frame capture history</h3><p>15-second PresentMon samples · comparison is against the previous saved capture of the same game.</p></div></div>
      {frameSessions.length ? <div className="panel history-table frame-history-table"><div className="table-head"><span>GAME / DATE</span><span>AVG FPS</span><span>1% LOW</span><span>P95 FRAME</span><span>SPIKES</span><span>DROPPED</span><span>Δ AVG / 1%</span></div>{frameSessions.map((row,index) => { const previous = frameSessions.slice(index + 1).find(item => item.game === row.game); const delta = (current: number, old: number | undefined) => old == null ? '—' : `${current - old >= 0 ? '+' : ''}${(current - old).toFixed(1)}`; return <div className="table-row" key={`frame-${row.id}`}><div><b>{row.game}</b><small>{new Date(row.at).toLocaleString()} · {row.captureSeconds}s</small></div><strong>{row.averageFps.toFixed(1)}</strong><strong>{row.onePercentLow.toFixed(1)}</strong><strong>{row.analysisVersion > 0 ? `${row.p95FrameTimeMs.toFixed(1)} ms` : '—'}</strong><span>{row.analysisVersion > 0 ? row.frameTimeSpikes : '—'}</span><span>{row.analysisVersion > 0 ? row.droppedFrames : '—'}</span><span>{previous ? `${delta(row.averageFps, previous.averageFps)} / ${delta(row.onePercentLow, previous.onePercentLow)} FPS` : 'First capture'}</span></div>; })}</div> : <div className="panel empty-large"><Gauge size={29}/><h3>No frame captures yet</h3><p>Capture a running game on the Performance page. VYRE stores up to 100 recent results locally.</p></div>}
    </>}
    <LiveModules
      section={section}
      machine={data?.machine}
      probe={data?.probe}
      networkScore={data?.networkScore}
      mode={profileMode}
      selectedPid={selectedPid}
      activeChanges={data?.activeChanges}
      busy={!!busy}
      isTauri={isTauri}
      connections={connections}
      trace={trace}
      result={result}
      frameCapture={frameCapture}
      onSelectGame={setSelectedPid}
      onMode={mode => void changeMode(mode)}
      onBoost={() => void runBoost()}
      onRestore={() => void restore()}
      onConnections={() => void refreshConnections()}
      onTrace={() => void runTrace()}
      onCaptureFrames={() => void captureFrames()}
    />
    {section === 'Settings' && <div className="panel module-panel overlay-setting"><div className="panel-head"><div><h3>Performance overlay</h3><p>Centered, always-on-top window with live network and system readings. FPS is labeled with its last capture time.</p></div><Gauge size={18}/></div><button className="button secondary" disabled={!isTauri} onClick={() => void toggleOverlay()}>Open / close overlay</button><p className="fine-print">Separate VYRE window; no game injection. Some games or anti-cheat systems may not display third-party overlays.</p></div>}
    {section === 'Settings' && <><div className="panel settings-panel"><div className="panel-head"><div><h3>{activeGame ? `${activeGame.name} profile` : 'Default optimization mode'}</h3><p>{activeGame ? 'Saved for this game and loaded when you select it. BOOST GAME is still manual.' : 'Choose which actions BOOST GAME may apply when no game is selected.'}</p></div></div><div className="mode-options">{['Safe', 'Competitive', 'Maximum', 'Custom'].map(mode => <button key={mode} className={`mode-option ${profileMode === mode ? 'selected' : ''}`} disabled={mode === 'Maximum' || mode === 'Custom' || !isTauri} onClick={() => changeMode(mode)}><span><b>{mode}</b><small>{mode === 'Safe' ? 'Measure only; make no system change.' : mode === 'Competitive' ? 'Temporary Above Normal process priority.' : 'Not available in this version.'}</small></span>{profileMode === mode ? <CheckCircle2 size={18}/> : mode === 'Maximum' || mode === 'Custom' ? <Clock3 size={17}/> : <ArrowRight size={17}/>}</button>)}</div></div><div className="panel info-panel"><h3>Local-first privacy</h3><p>Game profiles, probe measurements, and session history stay in vyre’s local SQLite database. No telemetry account or cloud upload is included.</p></div></>}
    {section === 'Restore' && <div className="panel restore-panel"><div className="restore-symbol"><RotateCcw size={31}/></div><h2>Restore everything</h2><p>Return any running game process priority changed by vyre to its saved original value. Restore also runs on normal app exit, game exit, and next launch after a crash.</p><div className="restore-count"><span>ACTIVE TEMPORARY CHANGES</span><strong>{data?.activeChanges ?? '—'}</strong></div><button className="button primary" disabled={!!busy || !isTauri || !data?.activeChanges} onClick={restore}><RotateCcw size={17}/> RESTORE EVERYTHING</button></div>}
    <footer className="footer"><span><ShieldCheck size={14}/> No game injection or packet modification</span><span>Last sample {data ? new Date(data.at).toLocaleTimeString() : '—'}</span></footer></div></main></div>;
}

function OverlayApp() {
  const [data, setData] = useState<Snapshot | null>(null);
  const [capture, setCapture] = useState<FrameSession | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    document.body.classList.add('overlay-mode');
    let active = true;
    const refresh = async () => { try { const next = await invoke<Snapshot>('snapshot'); if (active) { setData(next); setError(false); } } catch { if (active) setError(true); } };
    const refreshFrames = async () => { try { const rows = await invoke<FrameSession[]>('frame_history'); if (active) setCapture(rows[0] ?? null); } catch { if (active) setCapture(null); } };
    void refresh(); void refreshFrames();
    const timer = window.setInterval(() => void refresh(), 3000);
    const frameTimer = window.setInterval(() => void refreshFrames(), 15000);
    return () => { active = false; window.clearInterval(timer); window.clearInterval(frameTimer); document.body.classList.remove('overlay-mode'); };
  }, []);
  const ageSeconds = capture ? Math.max(0, Math.floor((Date.now() - Date.parse(capture.at)) / 1000)) : null;
  return <div className="overlay-shell"><header className="overlay-header"><div><b>VYRE</b><span>LIVE SESSION</span></div><button aria-label="Close overlay" onClick={() => void getCurrentWindow().close()}>×</button></header><div className="overlay-metrics"><div><span>PING</span><b>{units(data?.probe.averageMs, ' ms')}</b></div><div><span>JITTER</span><b>{units(data?.probe.jitterMs, ' ms')}</b></div><div><span>LOSS</span><b>{pct(data?.probe.lossPercent)}</b></div><div><span>CPU</span><b>{pct(data?.machine.cpuPercent)}</b></div><div><span>RAM</span><b>{pct(data?.machine.ramPercent)}</b></div><div><span>LAST FPS</span><b>{capture ? capture.averageFps.toFixed(0) : '—'}</b><small>{capture ? `${capture.onePercentLow.toFixed(0)} 1% low · ${ageSeconds}s ago` : 'Capture on Performance page'}</small></div></div><footer className="overlay-footer"><span>{error ? 'Waiting for live telemetry' : 'Live network and system data'}</span><span>FPS from last PresentMon capture</span></footer></div>;
}

const isOverlayWindow = (window as Window & { __VYRE_OVERLAY__?: boolean }).__VYRE_OVERLAY__ === true;
createRoot(document.getElementById('root')!).render(isOverlayWindow ? <OverlayApp/> : <App/>);
