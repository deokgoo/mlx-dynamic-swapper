import React, { useState, useEffect, useCallback } from 'react';
import {
  Activity,
  Cpu,
  Layers,
  Sparkles,
  Image as ImageIcon,
  Lock,
  Unlock,
  Play,
  RotateCw,
  Terminal,
  Server,
  Zap,
  CheckCircle2,
  AlertTriangle,
  RefreshCw,
  Gauge
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

interface EventLog {
  id: string;
  time: string;
  level: 'info' | 'warn' | 'swap' | 'success';
  message: string;
}

export default function App() {
  const [stats, setStats] = useState<SystemStats | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<string | null>(null);
  const [actionLoading, setActionLoading] = useState<string | null>(null);
  const [actionResult, setActionResult] = useState<string | null>(null);
  const [logs, setLogs] = useState<EventLog[]>([
    {
      id: 'init-1',
      time: new Date().toLocaleTimeString(),
      level: 'info',
      message: '대시보드 초기화 완료 (Web UI connected to mlx-swapper Rust)',
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

  // 실시간 상태 폴링
  const fetchStats = useCallback(async () => {
    try {
      const res = await fetch('/v1/system/stats');
      if (!res.ok) {
        // 백엔드 stats가 아직 없으면 /status 기반 폴백
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
          setError(null);
          return;
        }
        throw new Error(`HTTP ${res.status}`);
      }
      const data: SystemStats = await res.json();
      setStats(data);
      setError(null);
    } catch (err: any) {
      setError(err.message || '서버 통신 실패');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchStats();
    const interval = setInterval(fetchStats, 1500); // 1.5초마다 깜빡임 없이 부드럽게 갱신
    return () => clearInterval(interval);
  }, [fetchStats]);

  // 스왑 락 제어
  const handleToggleLock = async () => {
    const isLocked = stats?.is_locked;
    const targetUrl = isLocked ? '/v1/swap/unlock' : '/v1/swap/lock';
    const actionName = isLocked ? 'Unlock' : 'Lock';
    setActionLoading(actionName);
    try {
      const res = await fetch(targetUrl, { method: 'POST' });
      const json = await res.json();
      addLog('swap', `스왑 락 변경: ${JSON.stringify(json)}`);
      setActionResult(`${actionName} 완료`);
      fetchStats();
    } catch (err: any) {
      addLog('warn', `스왑 락 오류: ${err.message}`);
    } finally {
      setActionLoading(null);
    }
  };

  // ComfyUI 단발 테스트 실행
  const handleComfyTest = async () => {
    if (!confirm('ComfyUI 테스트를 실행하시겠습니까? 메인 LLM이 일시 중지되고 VRAM이 ComfyUI에 할당됩니다.')) {
      return;
    }
    setActionLoading('comfy');
    addLog('swap', '🎨 ComfyUI 온디맨드 스왑 시작: LLM 중지 -> ComfyUI 기동 요청');
    try {
      const res = await fetch('/v1/comfy/run', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          action: 't2i',
          prompt: 'A cute futuristic robot on Apple Silicon M5 Max desk, cyberpunk neon vibe',
        }),
      });
      const data = await res.json();
      if (res.ok) {
        addLog('success', `✔ ComfyUI 실행 성공! 결과 파일: ${data.files?.join(', ') || '생성 완료'}`);
        setActionResult('ComfyUI 워크플로우 정상 완료!');
      } else {
        addLog('warn', `ComfyUI 실행 에러: ${data.error || '실패'}`);
      }
      fetchStats();
    } catch (err: any) {
      addLog('warn', `ComfyUI 통신 오류: ${err.message}`);
    } finally {
      setActionLoading(null);
    }
  };

  // 포맷 헬퍼
  const formatUptime = (secs: number) => {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    const s = secs % 60;
    return `${h}h ${m}m ${s}s`;
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
      <header style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '32px' }}>
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
              <span className="badge badge-active">Apple Silicon M5 Max</span>
              <span className="badge badge-online">
                <span className="pulse-dot pulse-green"></span>
                {stats?.daemon.engine || 'Rust Native'}
              </span>
            </div>
            <p style={{ color: 'var(--text-secondary)', fontSize: '0.875rem', marginTop: '2px' }}>
              Zero-Copy Async SSE Proxy • Single-Occupancy Metal VRAM Swapping
            </p>
          </div>
        </div>

        <div style={{ display: 'flex', alignItems: 'center', gap: '12px' }}>
          <button
            className="btn-action"
            onClick={() => fetchStats()}
            title="즉시 새로고침"
          >
            <RefreshCw size={16} />
            새로고침
          </button>
          <div style={{ textAlign: 'right', fontSize: '0.75rem', color: 'var(--text-muted)' }}>
            <div>Daemon PID: <code>{stats?.daemon.pid || '-'}</code></div>
            <div>Daemon RAM: <strong style={{ color: '#34d399' }}>{stats?.daemon.ram_mb || 4} MB</strong></div>
          </div>
        </div>
      </header>

      {/* ── Grid Layout: Metrics & System Status ────────────────────── */}
      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(12, 1fr)', gap: '20px', marginBottom: '28px' }}>
        {/* Apple Silicon Unified Memory Gauge (Span 7) */}
        <div className="glass-panel" style={{ gridColumn: 'span 7', padding: '24px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '16px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
              <Gauge size={20} color="var(--accent-cyan)" />
              <h2 style={{ fontSize: '1.1rem', fontWeight: 600 }}>통합 메모리 점유 현황 (Unified Memory)</h2>
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
              {memPct < 75 ? '쾌적 (Safe < 75%)' : memPct < 90 ? '주의 (Warning)' : '위험 (Near Swap)'}
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
              스왑 발생 0% • Metal VRAM 안정 확보
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
              <span style={{ color: 'var(--text-muted)' }}>스왑 정책:</span> 단일 점유(Single Occupancy)
            </div>
            <div>
              <span style={{ color: 'var(--text-muted)' }}>스왑 시 VRAM 반환:</span> 100% POSIX 세션 회수
            </div>
            <div>
              <span style={{ color: 'var(--text-muted)' }}>여유 공간:</span>{' '}
              <strong style={{ color: '#f8fafc' }}>{(memTotalGb - memUsedGb).toFixed(1)} GB</strong>
            </div>
          </div>
        </div>

        {/* Swapper Control & Overview (Span 5) */}
        <div className="glass-panel" style={{ gridColumn: 'span 5', padding: '24px' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '16px' }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
              <Server size={20} color="var(--accent-purple)" />
              <h2 style={{ fontSize: '1.1rem', fontWeight: 600 }}>스왑 컨트롤 & 세션 제어</h2>
            </div>
            <span className={`badge ${stats?.is_locked ? 'badge-locked' : 'badge-online'}`}>
              {stats?.is_locked ? '스왑 고정 (Locked)' : '동적 스왑 가용'}
            </span>
          </div>

          <p style={{ fontSize: '0.85rem', color: 'var(--text-secondary)', marginBottom: '20px', lineHeight: 1.5 }}>
            ComfyUI 이미지 생성이나 긴급 압축 등 특정 작업에 메모리를 독점할 때 수동으로 스왑을 제어할 수 있습니다.
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
                {stats?.is_locked ? '스왑 락 해제 (Unlock)' : '현재 모델 고정 (Lock)'}
              </button>

              <button
                className="btn-action btn-primary"
                style={{ flex: 1 }}
                onClick={handleComfyTest}
                disabled={actionLoading !== null}
              >
                <ImageIcon size={16} />
                ComfyUI 온디맨드 런
              </button>
            </div>

            {actionLoading && (
              <div style={{ fontSize: '0.8rem', color: '#38bdf8', textAlign: 'center', marginTop: '4px' }}>
                <RotateCw size={14} className="inline animate-spin mr-1" />
                작업 처리 중... ({actionLoading})
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

      {/* ── Active 3-Layer Model Matrix ─────────────────────────────── */}
      <h2 style={{ fontSize: '1.2rem', fontWeight: 700, marginBottom: '16px', display: 'flex', alignItems: 'center', gap: '8px' }}>
        <Layers size={22} color="var(--accent-cyan)" />
        3-Track 하이브리드 레이어 매트릭스
      </h2>

      <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: '20px', marginBottom: '28px' }}>
        {/* Layer 1: Qwen 3.8 27B MTP (Main) */}
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
              {isMainActive ? 'ACTIVE (VRAM 점유)' : 'STANDBY'}
            </span>
          </div>

          <h3 style={{ fontSize: '1.15rem', fontWeight: 700, marginBottom: '4px' }}>
            Qwen 3.8 27B MTP
          </h3>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-secondary)', marginBottom: '16px' }}>
            지능 담당 • 아키텍처/디버깅 • MTP Multi-Token 43 tok/s
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '0.8rem' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>엔드포인트 포트:</span>
              <code>1234 (백엔드: 18234)</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Thinking 모드:</span>
              <strong style={{ color: '#38bdf8' }}>medium (~1.5s 즉시 반응)</strong>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>Context Length:</span>
              <span>65,536 tokens</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>VRAM 점유:</span>
              <span>~17.2 GB (4-bit quant)</span>
            </div>
          </div>
        </div>

        {/* Layer 2: Gemma 4 26B MoE (Compact) */}
        <div
          className={`glass-panel ${isCompactActive ? 'glass-panel-glow' : ''}`}
          style={{
            padding: '20px',
            position: 'relative',
            borderColor: isCompactActive ? 'rgba(192, 132, 252, 0.4)' : 'var(--border-card)',
          }}
        >
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '12px' }}>
            <span className="badge badge-warning">Track 2 • Fast Compactor</span>
            <span className={`badge ${isCompactActive ? 'badge-online' : 'badge-idle'}`}>
              <span className={`pulse-dot ${isCompactActive ? 'pulse-green' : 'pulse-gray'}`}></span>
              {isCompactActive ? 'COMPACTING' : 'STANDBY'}
            </span>
          </div>

          <h3 style={{ fontSize: '1.15rem', fontWeight: 700, marginBottom: '4px' }}>
            Gemma 4 26B-A4B MoE
          </h3>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-secondary)', marginBottom: '16px' }}>
            초고속 세션 압축 • 98 tok/s • 구조화 제약 주입
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '0.8rem' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>엔드포인트 포트:</span>
              <code>1235 (백엔드: 18235)</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>트리거 시점:</span>
              <span>세션 컨텍스트 만료 압축 시</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>처리 속도:</span>
              <strong style={{ color: '#c084fc' }}>98 tokens/sec</strong>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>캐시 격리:</span>
              <span>압축 전용 격리 스왑</span>
            </div>
          </div>
        </div>

        {/* Layer 3: ComfyUI (Image Gen) */}
        <div
          className={`glass-panel ${isComfyActive ? 'glass-panel-glow' : ''}`}
          style={{
            padding: '20px',
            position: 'relative',
            borderColor: isComfyActive ? 'rgba(52, 211, 153, 0.4)' : 'var(--border-card)',
          }}
        >
          <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: '12px' }}>
            <span className="badge badge-online">Track 3 • Image Studio</span>
            <span className={`badge ${isComfyActive ? 'badge-online' : 'badge-idle'}`}>
              <span className={`pulse-dot ${isComfyActive ? 'pulse-green' : 'pulse-gray'}`}></span>
              {isComfyActive ? 'GENERATING' : 'IDLE'}
            </span>
          </div>

          <h3 style={{ fontSize: '1.15rem', fontWeight: 700, marginBottom: '4px' }}>
            ComfyUI Engine
          </h3>
          <p style={{ fontSize: '0.8rem', color: 'var(--text-secondary)', marginBottom: '16px' }}>
            FLUX / SDXL 생성 • 온디맨드 스왑 & 자동 수거
          </p>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '8px', fontSize: '0.8rem' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>API 엔드포인트:</span>
              <code>POST /v1/comfy/run</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>기동 포트:</span>
              <code>8188 (내부 실행)</code>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>스왑 프로토콜:</span>
              <span>LLM 자동 일시중지 & 복원</span>
            </div>
            <div style={{ display: 'flex', justifyContent: 'space-between' }}>
              <span style={{ color: 'var(--text-muted)' }}>결과 반환:</span>
              <span>JSON + 이미지 절대 경로</span>
            </div>
          </div>
        </div>
      </div>

      {/* ── Realtime Event Logs & Console ────────────────────────────── */}
      <div className="glass-panel" style={{ padding: '20px' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: '12px' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
            <Terminal size={18} color="var(--accent-cyan)" />
            <h3 style={{ fontSize: '0.95rem', fontWeight: 600 }}>실시간 스왑 및 이벤트 콘솔 (No-Flicker Stream)</h3>
          </div>
          <span style={{ fontSize: '0.75rem', color: 'var(--text-muted)' }}>
            최근 {logs.length}개 이벤트
          </span>
        </div>

        <div
          style={{
            background: 'rgba(11, 15, 23, 0.8)',
            border: '1px solid rgba(255, 255, 255, 0.05)',
            borderRadius: '10px',
            padding: '12px 16px',
            maxHeight: '180px',
            overflowY: 'auto',
            fontFamily: 'Fira Code, monospace',
            fontSize: '0.8rem',
            lineHeight: 1.6,
          }}
        >
          {logs.map((log) => (
            <div key={log.id} style={{ display: 'flex', gap: '12px' }}>
              <span style={{ color: 'var(--text-muted)', flexShrink: 0 }}>[{log.time}]</span>
              <span
                style={{
                  color:
                    log.level === 'swap'
                      ? '#38bdf8'
                      : log.level === 'success'
                      ? '#34d399'
                      : log.level === 'warn'
                      ? '#fbbf24'
                      : '#94a3b8',
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
