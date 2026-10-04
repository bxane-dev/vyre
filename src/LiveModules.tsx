import { Activity, ArrowDownRight, ArrowRight, ArrowUpRight, CheckCircle2, Cpu, Gauge, MemoryStick, Network, RotateCcw, Route, ShieldCheck } from 'lucide-react';
import type { Benchmark, Connection, Machine, Probe } from './main';

type Props = {
  section: string;
  machine: Machine | undefined;
  probe: Probe | undefined;
  networkScore: number | null | undefined;
  mode: string | undefined;
  selectedPid: number | null;
  activeChanges: number | undefined;
  busy: boolean;
  isTauri: boolean;
  connections: Connection[];
  trace: string | null;
  result: Benchmark | null;
  onSelectGame: (pid: number) => void;
  onMode: (mode: string) => void;
  onBoost: () => void;
  onRestore: () => void;
  onConnections: () => void;
  onTrace: () => void;
};
const measure = (value: number | null | undefined, unit: string, decimals = 1) => value == null ? '—' : `${value.toFixed(decimals)}${unit}`;

export default function LiveModules(props: Props) {
  const { section, machine, probe, networkScore, mode, selectedPid, activeChanges, busy, isTauri, connections, trace, result, onSelectGame, onMode, onBoost, onRestore, onConnections, onTrace } = props;
  if (section === 'Performance') return <>
    <div className="module-metrics">
      <div className="panel module-metric"><Cpu size={19}/><span>CPU LOAD</span><strong>{measure(machine?.cpuPercent, '%', 0)}</strong><small>System-wide utilization</small></div>
      <div className="panel module-metric"><MemoryStick size={19}/><span>RAM PRESSURE</span><strong>{measure(machine?.ramPercent, '%', 0)}</strong><small>{machine ? `${machine.ramUsedGb.toFixed(1)} / ${machine.ramTotalGb.toFixed(1)} GB in use` : 'System memory'}</small></div>
      <div className="panel module-metric"><Gauge size={19}/><span>FRAME DATA</span><strong>—</strong><small>PresentMon is not connected</small></div>
    </div>
    <div className="panel module-panel"><div className="panel-head"><div><h3>Processes by CPU use</h3><p>Current process sample from Windows</p></div><Cpu size={18}/></div>
      {machine?.topProcesses.length ? <div className="module-list">{machine.topProcesses.map(process => <div className="module-process" key={process.pid}><div><b>{process.name}</b><small>PID {process.pid} · {process.memoryMb.toFixed(0)} MB RAM</small></div><strong>{process.cpuPercent.toFixed(1)}%</strong></div>)}</div> : <p className="module-empty">Waiting for a process sample.</p>}
      <p className="fine-print">Process CPU percentages use a single core as 100%, so a multi-core game can exceed 100%.</p>
    </div>
    <div className="panel info-panel"><h3>Frame-time capture</h3><p>FPS, 1% lows, GPU utilization, and stutter correlation need a PresentMon integration. vyre shows no estimated values for them.</p></div>
  </>;

  if (section === 'Traffic') return <>
    <div className="module-metrics two"><div className="panel module-metric"><ArrowDownRight size={21}/><span>TOTAL DOWNLOAD</span><strong>{measure(machine?.downloadMbps, ' Mbps')}</strong><small>Across Windows network adapters</small></div><div className="panel module-metric"><ArrowUpRight size={21}/><span>TOTAL UPLOAD</span><strong>{measure(machine?.uploadMbps, ' Mbps')}</strong><small>Across Windows network adapters</small></div></div>
    <div className="panel module-panel"><div className="panel-head"><div><h3>Active adapters</h3><p>Per-adapter transfer rate since the last sample</p></div><Network size={18}/></div>
      {machine?.adapters.length ? <div className="module-list">{machine.adapters.map(adapter => <div className="adapter-row" key={adapter.name}><b>{adapter.name}</b><span><ArrowDownRight size={14}/>{measure(adapter.downloadMbps, ' Mbps')}</span><span><ArrowUpRight size={14}/>{measure(adapter.uploadMbps, ' Mbps')}</span></div>)}</div> : <p className="module-empty">No adapter transferred data during this sample.</p>}
    </div>
    <div className="panel module-panel"><div className="panel-head"><div><h3>Current TCP connections</h3><p>Windows netstat ownership by process · no bandwidth estimate</p></div><button className="button secondary" disabled={busy || !isTauri} onClick={onConnections}><RotateCcw size={15}/> Refresh</button></div>
      {connections.length ? <div className="connection-table"><div className="connection-head"><span>PROCESS</span><span>REMOTE ADDRESS</span><span>STATE</span></div>{connections.slice(0, 40).map((connection, index) => <div className="connection-row" key={`${connection.pid}-${connection.remote}-${index}`}><div><b>{connection.process}</b><small>PID {connection.pid}</small></div><code>{connection.remote}</code><span>{connection.state}</span></div>)}</div> : <p className="module-empty">No active TCP connections returned.</p>}
      <p className="fine-print">Connection ownership does not reveal bytes used by each process. Pausing, limiting, and prioritizing traffic require additional Windows service work.</p>
    </div>
  </>;

  if (section === 'Smart Route') return <>
    <div className="panel direct-route"><div className="panel-head"><div><h3>Direct route quality</h3><p>Current connection to {probe?.target ?? '1.1.1.1'}</p></div><span className="tag green"><CheckCircle2 size={13}/> MEASURED</span></div>
      <div className="route-readings"><div><span>PING</span><b>{measure(probe?.averageMs, ' ms')}</b></div><div><span>JITTER</span><b>{measure(probe?.jitterMs, ' ms')}</b></div><div><span>LOSS</span><b>{measure(probe?.lossPercent, '%', 0)}</b></div><div><span>QUALITY SCORE</span><b>{networkScore ?? '—'}/100</b></div></div>
      <p className="fine-print">This is a public probe target, not your game server. The score compares direct-route stability only.</p>
    </div>
    <div className="panel module-panel"><div className="panel-head"><div><h3>Trace direct path</h3><p>Windows tracert, up to 12 hops</p></div><Route size={18}/></div><button className="button primary" disabled={busy || !isTauri} onClick={onTrace}><Route size={17}/> Run route trace</button>{trace && <pre className="trace-output">{trace}</pre>}</div>
    <div className="panel info-panel"><h3>Relay network status</h3><p>No relay nodes are deployed or configured. vyre cannot compare Frankfurt, Amsterdam, or other relays, and does not claim route-based ping reduction.</p></div>
  </>;

  if (section === 'Optimizations') return <>
    <div className="two-column top"><div className="panel module-panel"><div className="panel-head"><div><h3>Game profile</h3><p>Choose a detected process and the action to test</p></div><ShieldCheck size={18}/></div>
      <label className="module-label">RUNNING GAME<select value={selectedPid ?? ''} onChange={event => onSelectGame(Number(event.target.value))} disabled={!machine?.games.length}>{machine?.games.length ? machine.games.map(game => <option value={game.pid} key={game.pid}>{game.name} · PID {game.pid}</option>) : <option value="">No game detected</option>}</select></label>
      <div className="mode-options compact"><button className={`mode-option ${mode === 'Safe' ? 'selected' : ''}`} onClick={() => onMode('Safe')} disabled={!isTauri}><span><b>Safe</b><small>Measure only; make no system change.</small></span>{mode === 'Safe' && <CheckCircle2 size={18}/>}</button><button className={`mode-option ${mode === 'Competitive' ? 'selected' : ''}`} onClick={() => onMode('Competitive')} disabled={!isTauri}><span><b>Competitive</b><small>Temporary Above Normal process priority.</small></span>{mode === 'Competitive' && <CheckCircle2 size={18}/>}</button></div>
      <button className="button primary module-action" disabled={!selectedPid || busy || !isTauri} onClick={onBoost}><Activity size={17}/> Benchmark and apply <ArrowRight size={16}/></button>
    </div><div className="panel module-panel"><div className="panel-head"><div><h3>Restore status</h3><p>Recorded original values for temporary changes</p></div><RotateCcw size={18}/></div><div className="restore-count"><span>ACTIVE CHANGES</span><strong>{activeChanges ?? '—'}</strong></div><p className="module-description">vyre restores the saved priority when the game closes, when the app exits normally, or at the next launch after a crash.</p><button className="button secondary" disabled={!activeChanges || busy || !isTauri} onClick={onRestore}><RotateCcw size={16}/> Restore now</button></div></div>
    {result && <div className="panel module-panel"><div className="panel-head"><div><h3>Latest measured result · {result.game}</h3><p>{result.change}</p></div></div><div className="route-readings"><div><span>PING BEFORE</span><b>{measure(result.before.averageMs, ' ms')}</b></div><div><span>PING AFTER</span><b>{measure(result.after.averageMs, ' ms')}</b></div><div><span>JITTER BEFORE</span><b>{measure(result.before.jitterMs, ' ms')}</b></div><div><span>JITTER AFTER</span><b>{measure(result.after.jitterMs, ' ms')}</b></div></div><p className="fine-print">Process priority does not change the route to the probe target. Network differences may be normal variation.</p>{result.warning && <p className="fine-print warning-text">{result.warning}</p>}</div>}
    <div className="panel info-panel"><h3>Other controls</h3><p>GPU, memory, Windows power, and network shaping controls are unavailable until each has a measured benefit and a verified restore path. Maximum and Custom modes are not active.</p></div>
  </>;
  return null;
}
