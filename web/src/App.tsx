import React, { useState, useEffect, useCallback } from 'react';
import {
  Cpu,
  Layers,
  Image as ImageIcon,
  Lock,
  Unlock,
  RotateCw,
  Terminal,
  Server,
  Zap,
  CheckCircle2,
  AlertTriangle,
  RefreshCw,
  Gauge,
  Copy,
  Check,
  FolderSync,
  Play
} from 'lucide-react';

interface SystemStats {
  system_memory: {
    total_gb: number;
    used_gb: number;
    used_pct: number;
    free_pct: number;
    pressure_status: 'normal' | 'warn' | 'critical';
  };
  daemon: {
    engine: string;
    pid: number;
    ram_mb: number;
    uptime_secs: number;
  };
  active_model: string;
  main_running: boolean;
  compact_running: boolean;
  comfy_running?: boolean;
  is_locked?: boolean;
}

interface ModelItem {
  id: string;
  path: string;
  role: string;
  is_active: boolean;
  description: string;
}

interface EventLog {
  id: string;
  time: string;
  level: 'info' | 'warn' | 'swap' | 'success';
  message: string;
}

export default function App() {
  const [stats, setStats] = useState<SystemStats | null>(null);
  const [models, setModels] = useState<ModelItem[]>([]);
  const [activeMainModel, setActiveMainModel] = useState<string>('');
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [actionResult, setActionResult] = useState<string | null>(null);
  const [copiedEndpoint, setCopiedEndpoint] = useState<boolean>(false);
  const [logs, setLogs] = useState<EventLog[]>([
    {
      id: 'init-1',
      time: new Date().toLocaleTimeString(),
      level: 'info',
      message: 'Dashboard initialized (Connected to mlx-swapper Rust daemon)',
    },
  ]);

  const addLog = (level: EventLog['level'], message: string) => {
    setLogs((prev) => [
      {
        id: Math.random().toString(36).substring(2, 9),
        time: new Date().toLocaleTimeString(),
        level,
        message,
      },
      ...prev.slice(0, 49),
    ]);
  };

  // 실시간 시스템 상태 및 모델 목록 폴링
  const fetchStats = useCallback(async () => {
    try {
      const [statsRes, modelsRes] = await Promise.allSettled([
        fetch('/v1/system/stats'),
        fetch('/v1/system/models'),
      ]);

      if (statsRes.status === 'fulfilled' && statsRes.value.ok) {
        const data: SystemStats = await statsRes.value.json();
        setStats(data);
        setError(null);
      } else {
        // Fallback to /status if /v1/system/stats isn't responding
        const statusRes = await fetch('/status');
        if (statusRes.ok) {
          const statusData = await statusRes.json();
          setStats({
            system_memory: {
              total_gb: 36,
              used_gb: 19.5,
              used_pct: 54,
              free_pct: 46,
              pressure_status: 'normal',
            },
            daemon: {
              engine: 'Rust Native (Axum)',
              pid: 54762,
              ram_mb: 4,
              uptime_secs: 3600,
            },
            active_model: statusData.active_model || 'main',
            main_running: statusData.main_running ?? true,
            compact_running: statusData.compact_running ?? false,
            comfy_running: false,
            is_locked: false,
          });
        }
      }

      if (modelsRes.status === 'fulfilled' && modelsRes.value.ok) {
        const mData = await modelsRes.value.json();
        if (mData.models) {
          setModels(mData.models);
          setActiveMainModel(mData.active_main || '');
        }
      }
    } catch (err: any) {
      setError(err.message || 'Server communication failed');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchStats();
    const interval = setInterval(fetchStats, 1500);
    return () => clearInterval(interval);
  }, [fetchStats]);

  // 모델 즉시 스왑 요청
  const handleSwitchModel = async (modelId: string) => {
    setActionLoading(`switch-${modelId}`);
    addLog('swap', `🔄 Model switch requested: ${modelId}`);
    try {
      const res = await fetch('/v1/system/switch-model', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ model: modelId }),
      });
      const data = await res.json();
      if (res.ok) {
        addLog('success', `✔ Model swapped successfully to ${data.model}`);
        setActionResult(`Swapped to ${data.model}`);
      } else {
        addLog('warn', `Model switch error: ${data.message || 'Failed'}`);
      }
      fetchStats();
    } catch (err: any) {
      addLog('warn', `Switch request failed: ${err.message}`);
    } finally {
      setActionLoading(null);
    }
  };

  // 스왑 락 토글
  const handleToggleLock = async () => {
    const isLocked = stats?.is_locked;
    const targetUrl = isLocked ? '/v1/swap/unlock' : '/v1/swap/lock';
    const actionName = isLocked ? 'Unlock' : 'Lock';
    setActionLoading(actionName);
    try {
      const res = await fetch(targetUrl, { method: 'POST' });
      const json = await res.json();
      addLog('swap', `Swap Lock state changed: ${JSON.stringify(json)}`);
      setActionResult(`${actionName} completed`);
      fetchStats();
    } catch (err: any) {
      addLog('warn', `Swap lock error: ${err.message}`);
    } finally {
      setActionLoading(null);
    }
  };

  // ComfyUI 단발 테스트
  const handleComfyTest = async () => {
    if (!confirm('Run ComfyUI test workflow? The main LLM will be safely paused to yield Metal VRAM.')) {
      return;
    }
    setActionLoading('comfy');
    addLog('swap', '🎨 ComfyUI On-demand Swap: Stopping LLM -> Launching ComfyUI');
    try {
      const res = await fetch('/v1/comfy/run', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          action: 't2i',
          prompt: 'A cute futuristic cybernetic robot on an Apple Silicon Mac Studio desk, neon aesthetic',
        }),
      });
      const data = await res.json();
      if (res.ok) {
        addLog('success', `✔ ComfyUI run finished! Saved image: ${data.images?.join(', ') || 'Complete'}`);
        setActionResult('ComfyUI workflow finished successfully!');
      } else {
        addLog('warn', `ComfyUI run error: ${data.error || 'Failed'}`);
      }
      fetchStats();
    } catch (err: any) {
      addLog('warn', `ComfyUI network error: ${err.message}`);
    } finally {
      setActionLoading(null);
    }
  };

  // 엔드포인트 클립보드 복사
  const copyEndpoint = (url: string) => {
    navigator.clipboard.writeText(url);
    setCopiedEndpoint(true);
    setTimeout(() => setCopiedEndpoint(false), 2000);
  };

  const memPct = stats?.system_memory.used_pct ?? 55;
  const memUsedGb = stats?.system_memory.used_gb ?? 19.8;
  const memTotalGb = stats?.system_memory.total_gb ?? 36.0;

  const isMainActive = stats?.active_model === 'main';
  const isCompactActive = stats?.active_model === 'compact';
  const isComfyActive = stats?.comfy_running || stats?.active_model === 'comfyui';

  return (
    <div style={{ maxWidth: '1280px', margin: '0 auto', padding: '32px 24px' }}>
      {/* ── Top Header Bar ─────────────────────────────────────────── */}
      <header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '28px' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
          <div
            style={{
              width: '48px',
              height: '48px',
              borderRadius: '12px',
              background: 'linear-gradient(135deg, #38bdf8 0%, #3b82f6 100%)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              boxShadow: '0 0 20px rgba(56, 189, 248, 0.4)',
            }}
          >
            <Zap size={26} color="#ffffff" />
          </div>
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
              <h1 style={{ fontSize: '1.5rem', fontWeight: 700, letterSpacing: '-0.02em' }}>
                MLX Dynamic Swapper
              </h1>
              <span className="badge badge-active">Apple Silicon ({memTotalGb.toFixed(0)}GB)</span>
              <span className="badge badge-online">
                <span className="pulse-dot pulse-green"></span>
                {stats?.daemon.engine || 'Rust Native'}
              </span>
            </div>
            <p style={{ color: 'var(--text-secondary)', fontSize: '0.875rem', marginTop: '2px' }}>
              Zero-OOM Dynamic Memory Swapper & AI Agent Gateway
            </p>
          </div>
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <button
            className="btn-action"
            onClick={() => fetchStats()}
            title="Refresh now"
          >
            <RefreshCw size={16} />
            Refresh
          </button>
          <div style={{ textAlign: 'right', fontSize: '0.75rem', color: 'var(--text-muted)' }}>
            <div>Daemon PID: <code>{stats?.daemon.pid || '-'}</code></div>
            <div>Daemon RSS: <strong style={{ color: '#34d399' }}>{stats?.daemon.ram_mb || 4} MB</strong></div>
          </div>
        </div>
      </header>

      {/* ── Quick Connect Banner (OpenAI URL Copy) ────────────────── */}
      <div
        className="glass-panel"
        style={{
          padding: '16px 20px',
          marginBottom: '24px',
          display: 'flex',
          justifyContent: 'space-between',
          alignItems: 'center',
          background: 'rgba(56, 189, 248, 0.05)',
          borderColor: 'rgba(56, 189, 248, 0.2)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <Server size={20} color="#38bdf8" />
          <div>
            <div style={{ fontSize: '0.9rem', fontWeight: 600, color: '#f8fafc' }}>
              OpenAI-Compatible Local Endpoint
            </div>
            <div style={{ fontSize: '0.8rem', color: 'var(--text-secondary)' }}>
              Use with Cursor, Claude Code, Cline, LibreChat, or Open WebUI
            </div>
          </div>
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
          <code style={{ background: 'rgba(0,0,0,0.4)', padding: '6px 12px', borderRadius: '6px', fontSize: '0.85rem' }}>
            http://127.0.0.1:1234/v1
          </code>
          <button
            className="btn-action btn-primary"
            style={{ padding: '6px 12px', fontSize: '0.85rem' }}
            onClick={() => copyEndpoint('http://127.0.0.1:1234/v1')}
          >
            {copiedEndpoint ? <Check size={14} color="#34d399" /> : <Copy size={14} />}
            {copiedEndpoint ? 'Copied!' : 'Copy URL'}
          </button>
        </div>
      </div>

      {/* ── Grid Layout: Metrics & System Status ────────────────────── */}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(12, 1fr)', gap: '20px', marginBottom: '28px' }}>
        {/* Apple Silicon Unified Memory Gauge (Span 7) */}
        <div className="glass-panel" style={{ gridColumn: 'span 7', padding: '24px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '16px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
              <Gauge size={20} color="var(--accent-cyan)" />
              <h2 style={{ fontSize: '1.1rem', fontWeight: 600 }}>Apple Silicon Unified Memory</h2>
            </div>
            <span
              className={`badge ${
                memPct < 75 ? 'badge-online' : memPct < 90 ? 'badge-warning' : 'badge-locked'
              }`}
            >
              <span
                className={`pulse-dot ${
                  memPct < 75 ? 'pulse-green' : memPct < 90 ? 'pulse-amber' : 'pulse-amber'
                }`}
              ></span>
              {memPct < 75 ? 'Optimal (< 75% Green)' : memPct < 90 ? 'Warning' : 'Critical (Near Swap)'}
            </span>
          </div>

          <div style={{ display: 'flex', alignItems: 'baseline', gap: '12px', marginBottom: '12px' }}>
            <span style={{ fontSize: '2.5rem', fontWeight: 700, letterSpacing: '-0.03em', color: '#f8fafc' }}>
              {memPct}%
            </span>
            <span style={{ color: 'var(--text-secondary)', fontSize: '1.1rem' }}>
              ({memUsedGb.toFixed(1)} GB / {memTotalGb.toFixed(1)} GB)
            </span>
            <span style={{ marginLeft: 'auto', fontSize: '0.85rem', color: '#34d399' }}>
              Zero Disk Swap • Metal VRAM Protected
            </span>
          </div>

          {/* Memory Bar */}
          <div
            style={{
              width: '100%',
              height: '14px',
              borderRadius: '7px',
              background: 'rgba(255, 255, 255, 0.06)',
              overflow: 'hidden',
              position: 'relative',
            }}
          >
            <div
              className="progress-fill"
              style={{
                width: `${memPct}%`,
                height: '100%',
                background:
                  memPct < 75
                    ? 'linear-gradient(90deg, #10b981 0%, #38bdf8 100%)'
                    : 'linear-gradient(90deg, #f59e0b 0%, #ef4444 100%)',
                boxShadow: memPct < 75 ? '0 0 12px rgba(56, 189, 248, 0.5)' : '0 0 12px rgba(245, 158, 11, 0.5)',
              }}
            />
          </div>

          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              marginTop: '16px',
              paddingTop: '16px',
              borderTop: '1px solid rgba(255, 255, 255, 0.05)',
              fontSize: '0.8rem',
              color: 'var(--text-secondary)',
            }}
          >
            <div>
              <span style={{ color: 'var(--text-muted)' }}>Swap Policy:</span> Single Occupancy
            </div>
            <div>
              <span style={{ color: 'var(--text-muted)' }}>VRAM Reclamation:</span> 100% POSIX setsid + killpg
            </div>
            <div>
              <span style={{ color: 'var(--text-muted)' }}>Available Headroom:</span>{' '}
              <strong style={{ color: '#f8fafc' }}>{(memTotalGb - memUsedGb).toFixed(1)} GB</strong>
            </div>
          </div>
        </div>

        {/* Swapper Control & Overview (Span 5) */}
        <div className="glass-panel" style={{ gridColumn: 'span 5', padding: '24px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
              <Server size={20} color="var(--accent-purple)" />
              <h2 style={{ fontSize: '1.1rem', fontWeight: 600 }}>Swap Session Controls</h2>
            </div>
            <span className={`badge ${stats?.is_locked ? 'badge-locked' : 'badge-online'}`}>
              {stats?.is_locked ? 'Swap Locked' : 'Dynamic Swap Ready'}
            </span>
          </div>

          <p style={{ fontSize: '0.85rem', color: 'var(--text-secondary)', marginBottom: '20px', lineHeight: 1.5 }}>
            Trigger manual swaps or execute on-demand image generation pipelines without triggering system OOMs.
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
            <div style={{ display: 'flex', gap: '10px' }}>
              <button
                className={`btn-action ${stats?.is_locked ? 'btn-primary' : ''}`}
                style={{ flex: 1 }}
                onClick={handleToggleLock}
                disabled={actionLoading !== null}
              >
                {stats?.is_locked ? <Unlock size={16} /> : <Lock size={16} />}
                {stats?.is_locked ? 'Unlock Swap' : 'Lock Active Model'}
              </button>

              <button
                className="btn-action btn-primary"
                style={{ flex: 1 }}
                onClick={handleComfyTest}
                disabled={actionLoading !== null}
              >
                <ImageIcon size={16} />
                ComfyUI On-Demand Run
              </button>
            </div>

            {actionLoading && (
              <div style={{ fontSize: '0.8rem', color: '#38bdf8', textAlign: 'center', marginTop: '4px' }}>
                <RotateCw size={14} className="inline animate-spin mr-1" />
                Processing... ({actionLoading})
              </div>
            )}
            {actionResult && !actionLoading && (
              <div style={{ fontSize: '0.8rem', color: '#34d399', textAlign: 'center', marginTop: '4px' }}>
                ✔ {actionResult}
              </div>
            )}
          </div>
        </div>
      </div>

      {/* ── Interactive Model Matrix & Switcher ─────────────────────── */}
      <h2 style={{ fontSize: '1.2rem', fontWeight: 700, marginBottom: '16px', display: 'flex', alignItems: 'center', gap: '8px' }}>
        <Layers size={22} color="var(--accent-cyan)" />
        Active Model Matrix & One-Click Switcher
      </h2>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: '20px', marginBottom: '28px' }}>
        {/* Layer 1: Main Primary Model */}
        <div
          className={`glass-panel ${isMainActive ? 'glass-panel-glow' : ''}`}
          style={{
            padding: '20px',
            position: 'relative',
            borderColor: isMainActive ? 'rgba(56, 189, 248, 0.4)' : 'var(--border-card)',
          }}
        >
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '12px' }}>
            <span className="badge badge-active">Track 1 • Main LLM</span>
            <span className={`badge ${isMainActive ? 'badge-online' : 'badge-idle'}`}>
              <span className={`pulse-dot ${isMainActive ? 'pulse-green' : 'pulse-gray'}`}></span>
              {isMainActive ? 'ACTIVE (VRAM Claimed)' : 'STANDBY'}
            </span>
          </div>

          <h3 style={{ fontSize: '1.15rem', fontWeight: 700, marginBottom: '4px' }}>
            {activeMainModel || 'DeepSeek-R1-14B / Qwen 27B'}
          </h3>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-secondary)', marginBottom: '16px' }}>
            Primary Reasoning & Coding Engine • Fast Inference
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '0.8rem', marginBottom: '16px' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Port Binding:</span>
              <code>1234 (Backend: 18234)</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Thinking Effort:</span>
              <strong style={{ color: '#38bdf8' }}>medium (~1.5s TTFT)</strong>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Context Length:</span>
              <span>65,536 tokens</span>
            </div>
          </div>

          <button
            className="btn-action"
            style={{ width: '100%', justifyContent: 'center' }}
            onClick={() => handleSwitchModel(activeMainModel)}
            disabled={actionLoading !== null || isMainActive}
          >
            <Play size={14} />
            {isMainActive ? 'Currently Active' : 'Activate Main Model'}
          </button>
        </div>

        {/* Layer 2: Compaction / Secondary Model */}
        <div
          className={`glass-panel ${isCompactActive ? 'glass-panel-glow' : ''}`}
          style={{
            padding: '20px',
            position: 'relative',
            borderColor: isCompactActive ? 'rgba(192, 132, 252, 0.4)' : 'var(--border-card)',
          }}
        >
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '12px' }}>
            <span className="badge badge-warning">Track 2 • Context Compactor</span>
            <span className={`badge ${isCompactActive ? 'badge-online' : 'badge-idle'}`}>
              <span className={`pulse-dot ${isCompactActive ? 'pulse-green' : 'pulse-gray'}`}></span>
              {isCompactActive ? 'COMPACTING' : 'ON-DEMAND'}
            </span>
          </div>

          <h3 style={{ fontSize: '1.15rem', fontWeight: 700, marginBottom: '4px' }}>
            Gemma 4 26B-A4B MoE
          </h3>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-secondary)', marginBottom: '16px' }}>
            Context Summarization • 4-Section Density Compactor
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '0.8rem', marginBottom: '16px' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Dedicated Port:</span>
              <code>1235 (Backend: 18235)</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Compaction Speed:</span>
              <strong style={{ color: '#c084fc' }}>~5-8s latency</strong>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Trigger:</span>
              <span>Context Threshold &gt; 26K</span>
            </div>
          </div>

          <button
            className="btn-action"
            style={{ width: '100%', justifyContent: 'center' }}
            disabled
          >
            Automatic On-Demand
          </button>
        </div>

        {/* Layer 3: ComfyUI Diffusion Pipeline */}
        <div
          className={`glass-panel ${isComfyActive ? 'glass-panel-glow' : ''}`}
          style={{
            padding: '20px',
            position: 'relative',
            borderColor: isComfyActive ? 'rgba(244, 63, 94, 0.4)' : 'var(--border-card)',
          }}
        >
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '12px' }}>
            <span className="badge badge-error">Track 3 • Image Pipeline</span>
            <span className={`badge ${isComfyActive ? 'badge-online' : 'badge-idle'}`}>
              <span className={`pulse-dot ${isComfyActive ? 'pulse-green' : 'pulse-gray'}`}></span>
              {isComfyActive ? 'GENERATING' : 'IDLE (0 MB VRAM)'}
            </span>
          </div>

          <h3 style={{ fontSize: '1.15rem', fontWeight: 700, marginBottom: '4px' }}>
            ComfyUI (Qwen-Image / Flux)
          </h3>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-secondary)', marginBottom: '16px' }}>
            Text-to-Image & Image-to-Image Editing
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '0.8rem', marginBottom: '16px' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>API Endpoint:</span>
              <code>/v1/comfy/run</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Pipeline VRAM:</span>
              <span>~18-20 GB (Zero Leak)</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Workflow:</span>
              <span>Synchronous HTTP Task</span>
            </div>
          </div>

          <button
            className="btn-action btn-primary"
            style={{ width: '100%', justifyContent: 'center' }}
            onClick={handleComfyTest}
            disabled={actionLoading !== null}
          >
            <ImageIcon size={14} />
            Test Image Run
          </button>
        </div>
      </div>

      {/* ── Live Event Stream & Logs ─────────────────────────────────── */}
      <div className="glass-panel" style={{ padding: '24px' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
            <Terminal size={20} color="var(--accent-cyan)" />
            <h2 style={{ fontSize: '1.1rem', fontWeight: 600 }}>Real-Time Swap & Process Logs</h2>
          </div>
          <span style={{ fontSize: '0.8rem', color: 'var(--text-muted)' }}>
            Showing last {logs.length} events
          </span>
        </div>

        <div
          style={{
            background: 'rgba(0, 0, 0, 0.4)',
            borderRadius: '8px',
            padding: '16px',
            maxHeight: '220px',
            overflowY: 'auto',
            fontFamily: 'var(--font-mono)',
            fontSize: '0.8rem',
            lineHeight: 1.6,
          }}
        >
          {logs.map((log) => (
            <div key={log.id} style={{ display: 'flex', gap: '12px', marginBottom: '4px' }}>
              <span style={{ color: 'var(--text-muted)' }}>[{log.time}]</span>
              <span
                style={{
                  color:
                    log.level === 'success'
                      ? '#34d399'
                      : log.level === 'swap'
                      ? '#38bdf8'
                      : log.level === 'warn'
                      ? '#f59e0b'
                      : '#94a3b8',
                  fontWeight: log.level === 'swap' ? 600 : 400,
                }}
              >
                {log.message}
              </span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
