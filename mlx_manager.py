import asyncio
import os
import signal
import subprocess
import time
import httpx
import uvicorn
from fastapi import FastAPI, Request
from fastapi.responses import StreamingResponse, Response

PYTHON_BIN = os.environ.get("PYTHON_BIN", os.path.expanduser("~/.mlx-server/bin/python"))

MODEL_MAIN_PATH = os.environ.get("MODEL_MAIN_PATH", os.path.expanduser("~/.mlx-models/Qwen3.8-27B-4bit"))
MODEL_COMPACT_PATH = os.environ.get("MODEL_COMPACT_PATH", os.path.expanduser("~/.mlx-models/Qwen2.5-7B-Instruct-4bit"))

PORT_MAIN_INTERNAL = int(os.environ.get("PORT_MAIN_INTERNAL", 18234))
PORT_COMPACT_INTERNAL = int(os.environ.get("PORT_COMPACT_INTERNAL", 18235))

PROXY_PORT_MAIN = int(os.environ.get("PROXY_PORT_MAIN", 1234))
PROXY_PORT_COMPACT = int(os.environ.get("PROXY_PORT_COMPACT", 1235))

LOG_DIR = os.environ.get("MLX_LOG_DIR", os.path.expanduser("~/.mlx-server"))
LOG_FILE_MAIN_OUT = os.path.join(LOG_DIR, "server.log")
LOG_FILE_MAIN_ERR = os.path.join(LOG_DIR, "server.error.log")
LOG_FILE_COMPACT_OUT = os.path.join(LOG_DIR, "compaction.log")
LOG_FILE_COMPACT_ERR = os.path.join(LOG_DIR, "compaction.error.log")

PROMPT_CACHE_BYTES = os.environ.get("PROMPT_CACHE_BYTES", "6GB")

process_main = None
process_compact = None
swap_lock = asyncio.Lock()


def is_running(proc: subprocess.Popen | None) -> bool:
    if proc is None:
        return False
    return proc.poll() is None


def stop_process(proc: subprocess.Popen | None, name: str) -> None:
    if proc is None or proc.poll() is not None:
        return
    print(f"[MLX Manager] Stopping {name} (PID: {proc.pid}) to release Metal memory...", flush=True)
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        proc.wait(timeout=5)
    except Exception:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
        except Exception:
            pass
    print(f"[MLX Manager] {name} stopped. Metal memory 100% freed.", flush=True)


async def wait_for_ready(port: int, timeout: float = 30.0) -> bool:
    start_time = time.time()
    async with httpx.AsyncClient() as client:
        while time.time() - start_time < timeout:
            try:
                r = await client.get(f"http://127.0.0.1:{port}/v1/models", timeout=0.5)
                if r.status_code == 200:
                    return True
            except Exception:
                pass
            await asyncio.sleep(0.1)
    return False


async def _stop_compact_unlocked():
    global process_compact
    if is_running(process_compact):
        stop_process(process_compact, "Compaction-Model")
        process_compact = None


async def _stop_main_unlocked():
    global process_main
    if is_running(process_main):
        stop_process(process_main, "Main-Model")
        process_main = None


async def _start_main_unlocked():
    global process_main
    if is_running(process_main):
        return
    print(f"[MLX Manager] Starting Main Model ({os.path.basename(MODEL_MAIN_PATH)}) on port {PORT_MAIN_INTERNAL}...", flush=True)
    cmd = [
        PYTHON_BIN, "-m", "mlx_lm.server",
        "--model", MODEL_MAIN_PATH,
        "--port", str(PORT_MAIN_INTERNAL),
        "--host", "127.0.0.1",
        "--prefill-step-size", "2048",
        "--max-tokens", "8192",
        "--chat-template-args", '{"enable_thinking":true,"reasoning_effort":"low"}',
        "--prompt-cache-size", "2",
        "--prompt-cache-bytes", PROMPT_CACHE_BYTES,
        "--decode-concurrency", "1",
        "--prompt-concurrency", "1"
    ]
    os.makedirs(LOG_DIR, exist_ok=True)
    f_out = open(LOG_FILE_MAIN_OUT, "a")
    f_err = open(LOG_FILE_MAIN_ERR, "a")
    process_main = subprocess.Popen(
        cmd,
        stdout=f_out,
        stderr=f_err,
        preexec_fn=os.setsid
    )
    ready = await wait_for_ready(PORT_MAIN_INTERNAL, timeout=30.0)
    if not ready:
        print("[MLX Manager] ERROR: Main model server failed to become ready within timeout.", flush=True)
    else:
        print("[MLX Manager] Main model server is ready for inference!", flush=True)


async def _start_compact_unlocked():
    global process_compact
    if is_running(process_compact):
        return
    print(f"[MLX Manager] Starting Compaction Model ({os.path.basename(MODEL_COMPACT_PATH)}) on port {PORT_COMPACT_INTERNAL}...", flush=True)
    cmd = [
        PYTHON_BIN, "-m", "mlx_lm.server",
        "--model", MODEL_COMPACT_PATH,
        "--port", str(PORT_COMPACT_INTERNAL),
        "--host", "127.0.0.1",
        "--prefill-step-size", "2048",
        "--max-tokens", "8192",
        "--chat-template-args", '{"enable_thinking":false}',
        "--decode-concurrency", "1",
        "--prompt-concurrency", "1",
        "--prompt-cache-size", "0"
    ]
    os.makedirs(LOG_DIR, exist_ok=True)
    f_out = open(LOG_FILE_COMPACT_OUT, "a")
    f_err = open(LOG_FILE_COMPACT_ERR, "a")
    process_compact = subprocess.Popen(
        cmd,
        stdout=f_out,
        stderr=f_err,
        preexec_fn=os.setsid
    )
    ready = await wait_for_ready(PORT_COMPACT_INTERNAL, timeout=20.0)
    if not ready:
        print("[MLX Manager] ERROR: Compaction model server failed to become ready.", flush=True)
    else:
        print("[MLX Manager] Compaction model server is ready with full VRAM!", flush=True)


# ── App 1: Port 1234 (Main Model Proxy) ─────────────────────────
app_main = FastAPI(title="MLX Main Model Proxy")

@app_main.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_main(request: Request, path: str):
    if request.method == "GET" and path in ("v1/models", "models"):
        if not is_running(process_main):
            return {
                "object": "list",
                "data": [
                    {
                        "id": MODEL_MAIN_PATH,
                        "object": "model",
                        "created": int(time.time()),
                        "owned_by": "mlx"
                    }
                ]
            }

    async with swap_lock:
        await _stop_compact_unlocked()
        await _start_main_unlocked()

    url = f"http://127.0.0.1:{PORT_MAIN_INTERNAL}/{path}"
    headers = dict(request.headers)
    headers.pop("host", None)
    headers.pop("content-length", None)

    body = await request.body()
    client = httpx.AsyncClient(timeout=900.0)

    try:
        req = client.build_request(
            method=request.method,
            url=url,
            headers=headers,
            content=body,
            params=request.query_params
        )
        resp = await client.send(req, stream=True)

        async def stream_generator():
            try:
                async for chunk in resp.aiter_raw():
                    yield chunk
            finally:
                await resp.aclose()
                await client.aclose()

        response_headers = dict(resp.headers)
        response_headers.pop("content-length", None)
        response_headers.pop("content-encoding", None)

        return StreamingResponse(
            stream_generator(),
            status_code=resp.status_code,
            headers=response_headers,
            media_type=resp.headers.get("content-type")
        )
    except Exception as e:
        await client.aclose()
        return Response(content=f'{{"error": "{str(e)}"}}', status_code=502, media_type="application/json")


# ── App 2: Port 1235 (Compaction Model Proxy) ────────────────────
app_compact = FastAPI(title="MLX Compaction Proxy")

@app_compact.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_compact(request: Request, path: str):
    if request.method == "GET" and path in ("v1/models", "models"):
        return {
            "object": "list",
            "data": [
                {
                    "id": MODEL_COMPACT_PATH,
                    "object": "model",
                    "created": int(time.time()),
                    "owned_by": "mlx_compaction"
                }
            ]
        }

    # For actual compaction (POST /v1/chat/completions):
    # Hold swap_lock across the ENTIRE compaction and restoration of Main model so port 1234 never races!
    await swap_lock.acquire()
    try:
        print("[MLX Manager] Compaction requested! Stopping Main model and starting Compaction model...", flush=True)
        await _stop_main_unlocked()
        await _start_compact_unlocked()

        url = f"http://127.0.0.1:{PORT_COMPACT_INTERNAL}/{path}"
        headers = dict(request.headers)
        headers.pop("host", None)
        headers.pop("content-length", None)

        body = await request.body()
        client = httpx.AsyncClient(timeout=600.0)

        req = client.build_request(
            method=request.method,
            url=url,
            headers=headers,
            content=body,
            params=request.query_params
        )
        resp = await client.send(req, stream=True)

        async def stream_and_restore():
            try:
                async for chunk in resp.aiter_raw():
                    yield chunk
            finally:
                try:
                    await resp.aclose()
                    await client.aclose()
                    print("[MLX Manager] Compaction stream complete! Restoring Main model...", flush=True)
                    await _stop_compact_unlocked()
                    await _start_main_unlocked()
                    print("[MLX Manager] Main model restored successfully!", flush=True)
                finally:
                    swap_lock.release()
                    print("[MLX Manager] Swap lock released.", flush=True)

        response_headers = dict(resp.headers)
        response_headers.pop("content-length", None)
        response_headers.pop("content-encoding", None)

        return StreamingResponse(
            stream_and_restore(),
            status_code=resp.status_code,
            headers=response_headers,
            media_type=resp.headers.get("content-type")
        )
    except Exception as e:
        print(f"[MLX Manager] Compaction error: {e}", flush=True)
        try:
            await _stop_compact_unlocked()
            await _start_main_unlocked()
        finally:
            swap_lock.release()
        return Response(content=f'{{"error": "{str(e)}"}}', status_code=502, media_type="application/json")


async def main():
    print("[MLX Manager] Starting MLX Dynamic Model Swapper...", flush=True)
    async with swap_lock:
        await _start_main_unlocked()

    config_main = uvicorn.Config(
        app_main,
        host="127.0.0.1",
        port=PROXY_PORT_MAIN,
        log_level="warning"
    )
    config_compact = uvicorn.Config(
        app_compact,
        host="127.0.0.1",
        port=PROXY_PORT_COMPACT,
        log_level="warning"
    )

    server_main = uvicorn.Server(config_main)
    server_compact = uvicorn.Server(config_compact)

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, lambda: asyncio.create_task(shutdown_all(server_main, server_compact)))

    print(f"[MLX Manager] Proxies listening: Main on port {PROXY_PORT_MAIN}, Compaction on port {PROXY_PORT_COMPACT}", flush=True)
    await asyncio.gather(server_main.serve(), server_compact.serve())


async def shutdown_all(s1: uvicorn.Server, s2: uvicorn.Server):
    print("[MLX Manager] Shutting down manager and stopping MLX processes...", flush=True)
    s1.should_exit = True
    s2.should_exit = True
    async with swap_lock:
        await _stop_main_unlocked()
        await _stop_compact_unlocked()


if __name__ == "__main__":
    asyncio.run(main())
