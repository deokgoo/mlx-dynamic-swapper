import asyncio
import json
import os
import re
import signal
import socket
import subprocess
import time
import uuid
import httpx
import uvicorn
from fastapi import FastAPI, Request
from fastapi.responses import StreamingResponse, Response

PYTHON_BIN = os.environ.get("PYTHON_BIN", os.path.expanduser("~/.mlx-server/bin/python"))

# Model Paths
MODEL_CODER_PATH = os.environ.get("MODEL_CODER_PATH", os.path.expanduser("~/.mlx-models/Qwen2.5-Coder-14B-Instruct-4bit"))
MODEL_COMPACT_PATH = os.environ.get("MODEL_COMPACT_PATH", os.path.expanduser("~/.mlx-models/Qwen2.5-7B-Instruct-4bit"))
MODEL_THINKER_PATH = os.environ.get("MODEL_THINKER_PATH", os.path.expanduser("~/.mlx-models/Qwen3.8-27B-4bit"))

# Internal Ports
PORT_CODER_INTERNAL = int(os.environ.get("PORT_CODER_INTERNAL", 18234))
PORT_COMPACT_INTERNAL = int(os.environ.get("PORT_COMPACT_INTERNAL", 18235))
PORT_THINKER_INTERNAL = int(os.environ.get("PORT_THINKER_INTERNAL", 18236))

# Public Proxy Ports
PROXY_PORT_CODER = int(os.environ.get("PROXY_PORT_CODER", 1234))
PROXY_PORT_COMPACT = int(os.environ.get("PROXY_PORT_COMPACT", 1235))
PROXY_PORT_THINKER = int(os.environ.get("PROXY_PORT_THINKER", 1236))
PROXY_PORT_LAYA = int(os.environ.get("PROXY_PORT_LAYA", 1237))

# Laya (typed-decision model, ~658MB) — resident service, no swap needed
LAYA_MODEL_PATH = os.environ.get("LAYA_MODEL_PATH", os.path.expanduser("~/.mlx-models/laya-multilingual-mlx"))

# Log Directory & Files
LOG_DIR = os.environ.get("MLX_LOG_DIR", os.path.expanduser("~/.mlx-server"))
LOG_FILE_CODER_OUT = os.path.join(LOG_DIR, "coder.log")
LOG_FILE_CODER_ERR = os.path.join(LOG_DIR, "coder.error.log")
LOG_FILE_COMPACT_OUT = os.path.join(LOG_DIR, "compaction.log")
LOG_FILE_COMPACT_ERR = os.path.join(LOG_DIR, "compaction.error.log")
LOG_FILE_THINKER_OUT = os.path.join(LOG_DIR, "thinker.log")
LOG_FILE_THINKER_ERR = os.path.join(LOG_DIR, "thinker.error.log")

PROMPT_CACHE_BYTES = os.environ.get("PROMPT_CACHE_BYTES", "4GB")

process_coder = None
process_compact = None
process_thinker = None
last_active_model = "thinker"  # Tracks whether thinker or coder was active before compaction
swap_lock = asyncio.Lock()


def is_running(proc: subprocess.Popen | None) -> bool:
    if proc is None:
        return False
    return proc.poll() is None


def wait_for_port_release(port: int, timeout: float = 5.0) -> bool:
    """Ensure internal port is completely closed before launching a new model."""
    start = time.time()
    while time.time() - start < timeout:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
            s.settimeout(0.2)
            if s.connect_ex(('127.0.0.1', port)) != 0:
                return True
        time.sleep(0.1)
    return False


def kill_rogue_mlx_servers(allowed_pid: int | None = None):
    """Ensure absolutely no duplicate or rogue mlx_lm.server process is lingering."""
    my_pid = os.getpid()
    try:
        out = subprocess.check_output(["pgrep", "-f", "mlx_lm.server"]).decode().strip()
        if out:
            for pid_str in out.splitlines():
                try:
                    pid = int(pid_str.strip())
                    if pid != my_pid and (allowed_pid is None or pid != allowed_pid):
                        print(f"[MLX Manager] ⚠️ Terminating rogue/duplicate MLX process {pid}...", flush=True)
                        os.kill(pid, signal.SIGKILL)
                except Exception:
                    pass
    except Exception:
        pass


def stop_process(proc: subprocess.Popen | None, name: str) -> None:
    if proc is None or proc.poll() is not None:
        return
    print(f"[MLX Manager] Stopping {name} (PID: {proc.pid}) to release Metal memory...", flush=True)
    try:
        os.killpg(os.getpgid(proc.pid), signal.SIGTERM)
        proc.wait(timeout=3)
    except Exception:
        try:
            os.killpg(os.getpgid(proc.pid), signal.SIGKILL)
            proc.wait(timeout=2)
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


async def _stop_coder_unlocked():
    global process_coder
    if is_running(process_coder):
        stop_process(process_coder, "Coder-32B")
        process_coder = None
    wait_for_port_release(PORT_CODER_INTERNAL)


async def _stop_compact_unlocked():
    global process_compact
    if is_running(process_compact):
        stop_process(process_compact, "Compactor-7B")
        process_compact = None
    wait_for_port_release(PORT_COMPACT_INTERNAL)


async def _stop_thinker_unlocked():
    global process_thinker
    if is_running(process_thinker):
        stop_process(process_thinker, "Thinker-27B")
        process_thinker = None
    wait_for_port_release(PORT_THINKER_INTERNAL)


async def _start_coder_unlocked():
    global process_coder
    if is_running(process_coder):
        return
    # Guard: Kill any duplicate/rogue MLX servers and wait for port & memory clearance
    kill_rogue_mlx_servers()
    wait_for_port_release(PORT_CODER_INTERNAL)
    await asyncio.sleep(0.3)  # Apple Silicon Metal driver memory reclamation pause

    print(f"[MLX Manager] Starting Coder Model ({os.path.basename(MODEL_CODER_PATH)}) on port {PORT_CODER_INTERNAL}...", flush=True)
    cmd = [
        PYTHON_BIN, "-m", "mlx_lm.server",
        "--model", MODEL_CODER_PATH,
        "--port", str(PORT_CODER_INTERNAL),
        "--host", "127.0.0.1",
        "--prefill-step-size", "2048",
        "--max-tokens", "8192",
        "--chat-template-args", '{"enable_thinking":false}',
        "--prompt-cache-size", "1",
        "--prompt-cache-bytes", PROMPT_CACHE_BYTES,
        "--decode-concurrency", "1",
        "--prompt-concurrency", "1"
    ]
    os.makedirs(LOG_DIR, exist_ok=True)
    f_out = open(LOG_FILE_CODER_OUT, "a")
    f_err = open(LOG_FILE_CODER_ERR, "a")
    process_coder = subprocess.Popen(
        cmd,
        stdout=f_out,
        stderr=f_err,
        preexec_fn=os.setsid
    )
    ready = await wait_for_ready(PORT_CODER_INTERNAL, timeout=30.0)
    if not ready:
        print("[MLX Manager] ERROR: Coder model server failed to become ready within timeout.", flush=True)
    else:
        print(f"[MLX Manager] Coder model server is ready for inference! (PID: {process_coder.pid})", flush=True)


async def _start_compact_unlocked():
    global process_compact
    if is_running(process_compact):
        return
    kill_rogue_mlx_servers()
    wait_for_port_release(PORT_COMPACT_INTERNAL)
    await asyncio.sleep(0.3)

    print(f"[MLX Manager] Starting Compaction Model ({os.path.basename(MODEL_COMPACT_PATH)}) on port {PORT_COMPACT_INTERNAL}...", flush=True)
    cmd = [
        PYTHON_BIN, "-m", "mlx_lm.server",
        "--model", MODEL_COMPACT_PATH,
        "--port", str(PORT_COMPACT_INTERNAL),
        "--host", "127.0.0.1",
        "--prefill-step-size", "2048",
        "--max-tokens", "16384",
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
        print(f"[MLX Manager] Compaction model server is ready with full VRAM! (PID: {process_compact.pid})", flush=True)


async def _start_thinker_unlocked():
    global process_thinker
    if is_running(process_thinker):
        return
    kill_rogue_mlx_servers()
    wait_for_port_release(PORT_THINKER_INTERNAL)
    await asyncio.sleep(0.3)

    print(f"[MLX Manager] Starting Thinker Model ({os.path.basename(MODEL_THINKER_PATH)}) on port {PORT_THINKER_INTERNAL}...", flush=True)
    cmd = [
        PYTHON_BIN, "-m", "mlx_lm.server",
        "--model", MODEL_THINKER_PATH,
        "--port", str(PORT_THINKER_INTERNAL),
        "--host", "127.0.0.1",
        "--prefill-step-size", "2048",
        "--max-tokens", "8192",
        "--chat-template-args", '{"enable_thinking":true,"reasoning_effort":"minimal"}',
        "--prompt-cache-size", "1",
        "--prompt-cache-bytes", PROMPT_CACHE_BYTES,
        "--decode-concurrency", "1",
        "--prompt-concurrency", "1"
    ]
    os.makedirs(LOG_DIR, exist_ok=True)
    f_out = open(LOG_FILE_THINKER_OUT, "a")
    f_err = open(LOG_FILE_THINKER_ERR, "a")
    process_thinker = subprocess.Popen(
        cmd,
        stdout=f_out,
        stderr=f_err,
        preexec_fn=os.setsid
    )
    ready = await wait_for_ready(PORT_THINKER_INTERNAL, timeout=30.0)
    if not ready:
        print("[MLX Manager] ERROR: Thinker model server failed to become ready within timeout.", flush=True)
    else:
        print(f"[MLX Manager] Thinker model server is ready for inference! (PID: {process_thinker.pid})", flush=True)


def extract_tool_calls(text: str):
    if not text:
        return []
    tool_calls = []
    # 1. Match XML tags like <tool_call>...</tool_call> or <tools>...</tools>
    xml_matches = re.findall(r"<(?:tool_call|tools)>(.*?)</(?:tool_call|tools)>", text, re.DOTALL)
    for block in xml_matches:
        block = block.strip()
        try:
            parsed = json.loads(block)
            if isinstance(parsed, dict) and "name" in parsed:
                tool_calls.append(parsed)
            elif isinstance(parsed, list):
                for item in parsed:
                    if isinstance(item, dict) and "name" in item:
                        tool_calls.append(item)
        except Exception:
            pass

    # 2. Match markdown code blocks: ```(?:json)? ... ```
    if not tool_calls:
        md_matches = re.findall(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
        for block in md_matches:
            try:
                parsed = json.loads(block.strip())
                if isinstance(parsed, dict) and "name" in parsed and "arguments" in parsed:
                    tool_calls.append(parsed)
            except Exception:
                pass

    # 3. Match raw JSON lines or blocks with {"name": ..., "arguments": ...}
    if not tool_calls:
        for line in text.strip().splitlines():
            line = line.strip()
            if line.startswith("<|im_start|>"):
                line = line[len("<|im_start|>"):].strip()
            if line.startswith("{") and line.endswith("}") and '"name"' in line:
                try:
                    parsed = json.loads(line)
                    if isinstance(parsed, dict) and "name" in parsed and "arguments" in parsed:
                        tool_calls.append(parsed)
                except Exception:
                    pass
        if not tool_calls:
            clean_text = text.strip()
            if clean_text.startswith("<|im_start|>"):
                clean_text = clean_text[len("<|im_start|>"):].strip()
            if clean_text.startswith("```"):
                lines = clean_text.splitlines()
                if len(lines) >= 3 and lines[-1].strip().startswith("```"):
                    clean_text = "\n".join(lines[1:-1]).strip()
            try:
                parsed = json.loads(clean_text)
                if isinstance(parsed, dict) and "name" in parsed and "arguments" in parsed:
                    tool_calls.append(parsed)
            except Exception:
                pass

    return tool_calls


def make_streaming_tool_call_chunks(request_id: str, model: str, tool_calls: list):
    formatted = []
    for i, tc in enumerate(tool_calls):
        args_str = json.dumps(tc.get("arguments", {})) if isinstance(tc.get("arguments"), dict) else str(tc.get("arguments", "{}"))
        formatted.append({
            "index": i,
            "id": f"call_{uuid.uuid4().hex[:8]}",
            "type": "function",
            "function": {
                "name": tc.get("name", ""),
                "arguments": args_str
            }
        })
    c1 = {
        "id": request_id,
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": None, "tool_calls": formatted},
            "finish_reason": None
        }]
    }
    c2 = {
        "id": request_id,
        "object": "chat.completion.chunk",
        "created": int(time.time()),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "tool_calls"
        }]
    }
    return [
        f"data: {json.dumps(c1)}\n\n".encode("utf-8"),
        f"data: {json.dumps(c2)}\n\n".encode("utf-8"),
        b"data: [DONE]\n\n"
    ]


def make_non_streaming_tool_call_response(request_id: str, model: str, tool_calls: list):
    formatted = []
    for i, tc in enumerate(tool_calls):
        args_str = json.dumps(tc.get("arguments", {})) if isinstance(tc.get("arguments"), dict) else str(tc.get("arguments", "{}"))
        formatted.append({
            "id": f"call_{uuid.uuid4().hex[:8]}",
            "type": "function",
            "function": {
                "name": tc.get("name", ""),
                "arguments": args_str
            }
        })
    return {
        "id": request_id,
        "object": "chat.completion",
        "created": int(time.time()),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": None,
                "tool_calls": formatted
            },
            "finish_reason": "tool_calls"
        }]
    }


async def generic_proxy(request: Request, path: str, internal_port: int, timeout: float = 900.0):
    url = f"http://127.0.0.1:{internal_port}/{path}"
    headers = dict(request.headers)
    headers.pop("host", None)
    headers.pop("content-length", None)

    body = await request.body()
    has_tools = False
    is_streaming = True
    try:
        _data = json.loads(body)
        has_tools = bool(_data.get("tools"))
        is_streaming = _data.get("stream", True)
        if has_tools:
            print(f"[MLX Proxy] Port {internal_port} -> {path} (has {len(_data['tools'])} tools, stream={is_streaming})", flush=True)
    except Exception:
        pass

    client = httpx.AsyncClient(timeout=timeout)

    try:
        req = client.build_request(
            method=request.method,
            url=url,
            headers=headers,
            content=body,
            params=request.query_params
        )
        resp = await client.send(req, stream=True)

        response_headers = dict(resp.headers)
        response_headers.pop("content-length", None)
        response_headers.pop("content-encoding", None)

        if path.endswith("chat/completions") and has_tools:
            if is_streaming:
                async def smart_stream_generator():
                    try:
                        raw_chunks = []
                        full_content = ""
                        already_native = False
                        req_id = f"chatcmpl-{uuid.uuid4().hex[:8]}"
                        model_name = "qwen2.5-coder"

                        early_stream = False
                        async for chunk in resp.aiter_raw():
                            if early_stream:
                                yield chunk
                                continue
                            raw_chunks.append(chunk)
                            text = chunk.decode("utf-8", errors="ignore")
                            for line in text.splitlines():
                                if line.startswith("data: ") and line != "data: [DONE]":
                                    try:
                                        payload = json.loads(line[6:])
                                        req_id = payload.get("id", req_id)
                                        model_name = payload.get("model", model_name)
                                        choices = payload.get("choices", [])
                                        if choices:
                                            delta = choices[0].get("delta", {})
                                            if delta.get("tool_calls"):
                                                already_native = True
                                            if delta.get("content"):
                                                full_content += delta["content"]
                                    except Exception:
                                        pass

                            # Early streaming detection: if message starts with normal conversational text, stream live!
                            stripped = full_content.strip()
                            if len(stripped) >= 8 and not already_native:
                                if not stripped.startswith(("{", "<", "```", "[", "`")):
                                    early_stream = True
                                    for c in raw_chunks:
                                        yield c
                                    raw_chunks.clear()

                        if not already_native and not early_stream:
                            extracted = extract_tool_calls(full_content)
                            if extracted:
                                print(f"[MLX Proxy] ⚡️ Successfully adapted {len(extracted)} text tool call(s) to native OpenAI tool_calls!", flush=True)
                                for c in make_streaming_tool_call_chunks(req_id, model_name, extracted):
                                    yield c
                                return

                        for chunk in raw_chunks:
                            yield chunk
                    finally:
                        await resp.aclose()
                        await client.aclose()

                return StreamingResponse(
                    smart_stream_generator(),
                    status_code=resp.status_code,
                    headers=response_headers,
                    media_type=resp.headers.get("content-type")
                )
            else:
                content = await resp.aread()
                await client.aclose()
                try:
                    payload = json.loads(content)
                    choice = payload.get("choices", [{}])[0]
                    msg = choice.get("message", {})
                    if not msg.get("tool_calls"):
                        extracted = extract_tool_calls(msg.get("content", ""))
                        if extracted:
                            print(f"[MLX Proxy] ⚡️ Successfully adapted {len(extracted)} non-stream text tool call(s) to native OpenAI format!", flush=True)
                            adapted = make_non_streaming_tool_call_response(
                                payload.get("id", f"chatcmpl-{uuid.uuid4().hex[:8]}"),
                                payload.get("model", "qwen2.5-coder"),
                                extracted
                            )
                            return Response(content=json.dumps(adapted), status_code=resp.status_code, headers=response_headers, media_type="application/json")
                except Exception:
                    pass
                return Response(content=content, status_code=resp.status_code, headers=response_headers, media_type=resp.headers.get("content-type"))

        async def stream_generator():
            try:
                async for chunk in resp.aiter_raw():
                    yield chunk
            finally:
                await resp.aclose()
                await client.aclose()

        return StreamingResponse(
            stream_generator(),
            status_code=resp.status_code,
            headers=response_headers,
            media_type=resp.headers.get("content-type")
        )
    except Exception as e:
        await client.aclose()
        return Response(content=f'{{"error": "{str(e)}"}}', status_code=502, media_type="application/json")


def get_system_status():
    running_models = []
    if is_running(process_coder):
        running_models.append({"name": os.path.basename(MODEL_CODER_PATH), "pid": process_coder.pid, "port": PORT_CODER_INTERNAL})
    if is_running(process_compact):
        running_models.append({"name": "compact-7b", "pid": process_compact.pid, "port": PORT_COMPACT_INTERNAL})
    if is_running(process_thinker):
        running_models.append({"name": "thinker-27b", "pid": process_thinker.pid, "port": PORT_THINKER_INTERNAL})
    return {
        "status": "ok",
        "active_models_count": len(running_models),
        "active_models": running_models,
        "last_active": last_active_model
    }


# ── App 1: Port 1234 (Coder Model Proxy) ─────────────────────────
app_coder = FastAPI(title="MLX Coder Model Proxy")

@app_coder.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_coder(request: Request, path: str):
    global last_active_model
    if request.method == "GET" and path in ("health", "status"):
        return get_system_status()
    if request.method == "GET" and path in ("v1/models", "models"):
        return {
            "object": "list",
            "data": [
                {
                    "id": MODEL_THINKER_PATH,
                    "object": "model",
                    "created": int(time.time()),
                    "owned_by": "mlx_thinker"
                }
            ]
        }

    async with swap_lock:
        last_active_model = "thinker"
        await _stop_compact_unlocked()
        await _stop_coder_unlocked()
        await _start_thinker_unlocked()

    return await generic_proxy(request, path, PORT_THINKER_INTERNAL)


# ── App 2: Port 1235 (Compaction Model Proxy) ────────────────────
app_compact = FastAPI(title="MLX Compaction Proxy")

@app_compact.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_compact(request: Request, path: str):
    if request.method == "GET" and path in ("health", "status"):
        return get_system_status()
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

    # For actual compaction:
    # Hold swap_lock across the ENTIRE compaction and restore whichever model was active before
    await swap_lock.acquire()
    try:
        print(f"[MLX Manager] Compaction requested! Stopping current models and starting Compaction model...", flush=True)
        await _stop_coder_unlocked()
        await _stop_thinker_unlocked()
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
                    print(f"[MLX Manager] Compaction stream complete! Restoring previous model ({last_active_model})...", flush=True)
                    await _stop_compact_unlocked()
                    if last_active_model == "thinker":
                        await _start_thinker_unlocked()
                    else:
                        await _start_coder_unlocked()
                    print(f"[MLX Manager] Model ({last_active_model}) restored successfully!", flush=True)
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
            if last_active_model == "thinker":
                await _start_thinker_unlocked()
            else:
                await _start_coder_unlocked()
        finally:
            swap_lock.release()
        return Response(content=f'{{"error": "{str(e)}"}}', status_code=502, media_type="application/json")


# ── App 3: Port 1236 (Thinker Model Proxy) ───────────────────────
app_thinker = FastAPI(title="MLX Thinker Model Proxy")

@app_thinker.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_thinker(request: Request, path: str):
    global last_active_model
    if request.method == "GET" and path in ("health", "status"):
        return get_system_status()
    if request.method == "GET" and path in ("v1/models", "models"):
        if not is_running(process_thinker):
            return {
                "object": "list",
                "data": [
                    {
                        "id": MODEL_THINKER_PATH,
                        "object": "model",
                        "created": int(time.time()),
                        "owned_by": "mlx_thinker"
                    }
                ]
            }

    async with swap_lock:
        last_active_model = "thinker"
        await _stop_compact_unlocked()
        await _stop_coder_unlocked()
        await _start_thinker_unlocked()

    return await generic_proxy(request, path, PORT_THINKER_INTERNAL)


# ── App 4: Port 1237 (Laya Typed-Decision Service, resident) ─────
# Laya is a small (~658MB) typed-decision model: single forward pass,
# no text generation. It runs RESIDENT (no swap) alongside the LLMs.
app_laya = FastAPI(title="Laya Typed-Decision Service")

laya_agent = None
laya_agent_error = None
laya_lock = asyncio.Lock()


def _load_laya():
    """Load the Laya agent. Returns (agent, error)."""
    global laya_agent, laya_agent_error
    if laya_agent is not None:
        return laya_agent, None
    if not os.path.isdir(LAYA_MODEL_PATH):
        laya_agent_error = f"model directory not found: {LAYA_MODEL_PATH}"
        return None, laya_agent_error
    try:
        import laya_mlx
        t0 = time.time()
        laya_agent = laya_mlx.load(LAYA_MODEL_PATH)
        print(f"[MLX Manager] Laya agent loaded from {LAYA_MODEL_PATH} in {time.time()-t0:.1f}s", flush=True)
        return laya_agent, None
    except Exception as e:
        laya_agent_error = str(e)
        print(f"[MLX Manager] Laya load failed: {e}", flush=True)
        return None, laya_agent_error


@app_laya.get("/health")
@app_laya.get("/status")
async def laya_health():
    agent, err = _load_laya()
    return {
        "status": "ok" if agent else "error",
        "service": "laya",
        "model": LAYA_MODEL_PATH,
        "loaded": agent is not None,
        "error": err,
    }


@app_laya.get("/v1/models")
async def laya_models():
    agent, _ = _load_laya()
    return {
        "object": "list",
        "data": [{
            "id": "laya-multilingual",
            "object": "model",
            "created": int(time.time()),
            "owned_by": "laya_mlx",
            "loaded": agent is not None,
        }],
    }


@app_laya.get("/v1/presets")
async def laya_presets():
    """Built-in typed question presets (triage/email/guard/moderation/router)."""
    try:
        from laya_mlx import (
            email_questions, guard_questions, moderation_questions,
            router_questions, triage_questions,
        )
        return {
            "triage": triage_questions(),
            "email": email_questions(),
            "guard": guard_questions(),
            "moderation": moderation_questions(),
            "router": router_questions(),
        }
    except Exception as e:
        return Response(content=json.dumps({"error": str(e)}), status_code=500, media_type="application/json")


@app_laya.post("/v1/predict")
async def laya_predict(request: Request):
    """Typed decisions: {state: str|dict, questions: {id: {type, instructions, criteria?}}}"""
    agent, err = _load_laya()
    if agent is None:
        return Response(
            content=json.dumps({"error": f"laya not loaded: {err}"}),
            status_code=503, media_type="application/json",
        )
    try:
        payload = await request.json()
    except Exception:
        return Response(content=json.dumps({"error": "invalid JSON body"}), status_code=400, media_type="application/json")

    state = payload.get("state")
    questions = payload.get("questions")
    if state is None or not questions:
        return Response(
            content=json.dumps({"error": "required fields: state, questions"}),
            status_code=400, media_type="application/json",
        )

    async with laya_lock:
        t0 = time.time()
        try:
            result = agent.predict(state, questions)
        except Exception as e:
            return Response(
                content=json.dumps({"error": f"prediction failed: {e}"}),
                status_code=500, media_type="application/json",
            )
    result["latency_ms"] = round((time.time() - t0) * 1000, 2)
    return result


async def main():
    print("[MLX Manager] Starting 3-Model MLX Dynamic Swapper + Laya service...", flush=True)
    # Aggressively kill ANY leftover mlx servers from previous runs to guarantee clean slate
    kill_rogue_mlx_servers()
    await asyncio.sleep(0.5)
    async with swap_lock:
        # Start Thinker (Qwen3.8-27B) as default primary model on boot
        await _start_thinker_unlocked()

    # Load Laya resident (small, no swap) — non-blocking
    asyncio.create_task(asyncio.to_thread(_load_laya))

    config_coder = uvicorn.Config(
        app_coder,
        host="127.0.0.1",
        port=PROXY_PORT_CODER,
        log_level="warning"
    )
    config_compact = uvicorn.Config(
        app_compact,
        host="127.0.0.1",
        port=PROXY_PORT_COMPACT,
        log_level="warning"
    )
    config_thinker = uvicorn.Config(
        app_thinker,
        host="127.0.0.1",
        port=PROXY_PORT_THINKER,
        log_level="warning"
    )
    config_laya = uvicorn.Config(
        app_laya,
        host="127.0.0.1",
        port=PROXY_PORT_LAYA,
        log_level="warning"
    )

    server_coder = uvicorn.Server(config_coder)
    server_compact = uvicorn.Server(config_compact)
    server_thinker = uvicorn.Server(config_thinker)
    server_laya = uvicorn.Server(config_laya)

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, lambda: asyncio.create_task(shutdown_all(server_coder, server_compact, server_thinker, server_laya)))

    print(f"[MLX Manager] Proxies listening: Coder on {PROXY_PORT_CODER}, Compactor on {PROXY_PORT_COMPACT}, Thinker on {PROXY_PORT_THINKER}, Laya on {PROXY_PORT_LAYA}", flush=True)
    await asyncio.gather(server_coder.serve(), server_compact.serve(), server_thinker.serve(), server_laya.serve())


async def shutdown_all(s1: uvicorn.Server, s2: uvicorn.Server, s3: uvicorn.Server, s4: uvicorn.Server):
    print("[MLX Manager] Shutting down manager and stopping MLX processes...", flush=True)
    s1.should_exit = True
    s2.should_exit = True
    s3.should_exit = True
    s4.should_exit = True
    async with swap_lock:
        await _stop_coder_unlocked()
        await _stop_compact_unlocked()
        await _stop_thinker_unlocked()


if __name__ == "__main__":
    asyncio.run(main())
