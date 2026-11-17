import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';

const stats = {
  system_memory: {
    total_gb: 36,
    used_gb: 18,
    used_pct: 50,
    free_pct: 50,
    pressure_status: 'normal',
  },
  daemon: {
    engine: 'Rust Native (Axum)',
    pid: 123,
    ram_mb: 4,
    uptime_secs: 60,
  },
  active_model: 'main',
  main_running: true,
  compact_running: false,
  comfy_running: false,
  is_locked: false,
};

const fetchMock = vi.fn<typeof fetch>();
let actionResponse: () => Promise<Response>;

beforeEach(() => {
  actionResponse = async () => Response.json({ status: 'locked' });
  fetchMock.mockReset();
  fetchMock.mockImplementation(async (input, init) => {
    if (input === '/v1/system/stats') return Response.json(stats);
    if (init?.method === 'POST') return actionResponse();
    throw new Error(`Unexpected request: ${input}`);
  });
  vi.stubGlobal('fetch', fetchMock);
  vi.stubGlobal('confirm', vi.fn(() => true));
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('dashboard action feedback', () => {
  it('shows a successful lock action', async () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: /현재 모델 고정/ }));

    expect(await screen.findByText('✔ Lock 완료')).toBeTruthy();
    expect(fetchMock).toHaveBeenCalledWith('/v1/swap/lock', { method: 'POST' });
  });

  it('shows the JSON error instead of reporting a failed lock as successful', async () => {
    actionResponse = async () => Response.json({ error: 'backend failure' }, { status: 500 });
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: /현재 모델 고정/ }));

    expect(await screen.findByText('스왑 락 오류: backend failure')).toBeTruthy();
    expect(screen.queryByText('✔ Lock 완료')).toBeNull();
  });

  it('preserves a plain-text ComfyUI backend error', async () => {
    actionResponse = async () => new Response('Workflow template not found', { status: 400 });
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: /ComfyUI 온디맨드 런/ }));

    expect(await screen.findByText(/ComfyUI.*오류: Workflow template not found/)).toBeTruthy();
    expect(screen.queryByText('✔ ComfyUI 워크플로우 정상 완료!')).toBeNull();
  });

  it('clears an earlier success when the next action fails', async () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: /현재 모델 고정/ }));
    await screen.findByText('✔ Lock 완료');

    actionResponse = async () => new Response('ComfyUI unavailable', { status: 503 });
    fireEvent.click(screen.getByRole('button', { name: /ComfyUI 온디맨드 런/ }));

    expect(await screen.findByText(/ComfyUI.*오류: ComfyUI unavailable/)).toBeTruthy();
    expect(screen.queryByText('✔ Lock 완료')).toBeNull();
  });

  it('clears a success while the next request is pending', async () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: /현재 모델 고정/ }));
    await screen.findByText('✔ Lock 완료');

    actionResponse = () => new Promise<Response>(() => {});
    fireEvent.click(screen.getByRole('button', { name: /현재 모델 고정/ }));

    expect(screen.queryByText('✔ Lock 완료')).toBeNull();
  });

  it('preserves network failure messages without reporting success', async () => {
    actionResponse = async () => { throw new Error('Network disconnected'); };
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: /현재 모델 고정/ }));

    expect(await screen.findByText('스왑 락 오류: Network disconnected')).toBeTruthy();
    expect(screen.queryByText('✔ Lock 완료')).toBeNull();
  });
});
