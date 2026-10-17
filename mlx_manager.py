import asyncio
import json
import os
import random
import re
import shutil
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
MODEL_MAIN_PATH = os.environ.get("MODEL_MAIN_PATH", os.path.expanduser("~/.mlx-models/Qwen3.6-35B-A3B-4bit"))
MODEL_COMPACT_PATH = os.environ.get("MODEL_COMPACT_PATH", os.path.expanduser("~/.mlx-models/Qwen2.5-7B-Instruct-4bit"))

# Internal Ports
PORT_MAIN_INTERNAL = int(os.environ.get("PORT_MAIN_INTERNAL", 18234))
PORT_COMPACT_INTERNAL = int(os.environ.get("PORT_COMPACT_INTERNAL", 18235))

# Public Proxy Ports
PROXY_PORT_MAIN = int(os.environ.get("PROXY_PORT_MAIN", 1234))
PROXY_PORT_COMPACT = int(os.environ.get("PROXY_PORT_COMPACT", 1235))
PORT_TO_MODEL = {
    PORT_MAIN_INTERNAL: MODEL_MAIN_PATH,
    PORT_COMPACT_INTERNAL: MODEL_COMPACT_PATH,
}

# ComfyUI Configuration
COMFY_PYTHON_BIN = os.environ.get(
    "COMFY_PYTHON_BIN",
    os.path.expanduser("~/ComfyUI-Installs/ComfyUI/ComfyUI/.venv/bin/python3"),
)
COMFY_MAIN_PY = os.environ.get(
    "COMFY_MAIN_PY",
    os.path.expanduser("~/ComfyUI-Installs/ComfyUI/ComfyUI/main.py"),
)
COMFY_PORT = int(os.environ.get("COMFY_PORT", 8188))
COMFY_MODEL_PATHS_YAML = os.environ.get(
    "COMFY_MODEL_PATHS_YAML",
    os.path.expanduser("~/Library/Application Support/Comfy Desktop/instance-model-paths/inst-1790991158418.yaml"),
)
COMFY_INPUT_DIR = os.environ.get("COMFY_INPUT_DIR", os.path.expanduser("~/ComfyUI-Shared/input"))
COMFY_OUTPUT_DIR = os.environ.get("COMFY_OUTPUT_DIR", os.path.expanduser("~/ComfyUI-Shared/output"))
COMFY_WORKFLOWS_DIR = os.path.expanduser("~/.hermes/skills/creative/comfyui/workflows")

# Log Directory & Files
LOG_DIR = os.environ.get("MLX_LOG_DIR", os.path.expanduser("~/.mlx-server"))
LOG_FILE_MAIN_OUT = os.path.join(LOG_DIR, "main.log")
LOG_FILE_MAIN_ERR = os.path.join(LOG_DIR, "main.error.log")
LOG_FILE_COMPACT_OUT = os.path.join(LOG_DIR, "compaction.log")
LOG_FILE_COMPACT_ERR = os.path.join(LOG_DIR, "compaction.error.log")
LOG_FILE_COMFY_OUT = os.path.join(LOG_DIR, "comfyui.log")
LOG_FILE_COMFY_ERR = os.path.join(LOG_DIR, "comfyui.error.log")

PROMPT_CACHE_BYTES = os.environ.get("PROMPT_CACHE_BYTES", "4GB")
PREFILL_STEP_SIZE = os.environ.get("PREFILL_STEP_SIZE", "512")

process_main = None
process_compact = None
process_comfy = None
last_active_model = "main"  # Default to Qwen3.6 main model
swap_lock = asyncio.Lock()
is_manual_locked = False


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


async def _stop_main_unlocked():
    global process_main
    if is_running(process_main):
        stop_process(process_main, "Main-Qwen3.6-35B")
        process_main = None
    wait_for_port_release(PORT_MAIN_INTERNAL)


async def _stop_compact_unlocked():
    global process_compact
    if is_running(process_compact):
        stop_process(process_compact, "Compactor-7B")
        process_compact = None
    wait_for_port_release(PORT_COMPACT_INTERNAL)


async def _start_main_unlocked():
    global process_main
    if is_running(process_main):
        return
    kill_rogue_mlx_servers()
    wait_for_port_release(PORT_MAIN_INTERNAL)
    await asyncio.sleep(0.3)

    print(f"[MLX Manager] Starting Main Model ({os.path.basename(MODEL_MAIN_PATH)}) on port {PORT_MAIN_INTERNAL}...", flush=True)
    cmd = [
        PYTHON_BIN, "-m", "mlx_lm.server",
        "--model", MODEL_MAIN_PATH,
        "--port", str(PORT_MAIN_INTERNAL),
        "--host", "127.0.0.1",
        "--prefill-step-size", PREFILL_STEP_SIZE,
        "--max-tokens", "12288",
        "--prompt-cache-size", "1",
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
        print(f"[MLX Manager] Main model server is ready for inference! (PID: {process_main.pid})", flush=True)


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
        "--prefill-step-size", PREFILL_STEP_SIZE,
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
        print(f"[MLX Manager] Compaction model server is ready with full VRAM! (PID: {process_compact.pid})", flush=True)


async def _stop_comfy_unlocked():
    global process_comfy
    if is_running(process_comfy):
        stop_process(process_comfy, "ComfyUI")
        process_comfy = None
    wait_for_port_release(COMFY_PORT)


async def wait_for_comfy_ready(port: int, timeout: float = 40.0) -> bool:
    start_time = time.time()
    async with httpx.AsyncClient() as client:
        while time.time() - start_time < timeout:
            try:
                r = await client.get(f"http://127.0.0.1:{port}/system_stats", timeout=1.0)
                if r.status_code == 200:
                    return True
            except Exception:
                pass
            await asyncio.sleep(0.5)
    return False


async def _start_comfy_unlocked():
    global process_comfy
    if is_running(process_comfy):
        return
    wait_for_port_release(COMFY_PORT)
    await asyncio.sleep(0.3)

    print(f"[MLX Manager] Starting ComfyUI backend on port {COMFY_PORT}...", flush=True)
    cmd = [
        COMFY_PYTHON_BIN, COMFY_MAIN_PY,
        "--listen", "127.0.0.1",
        "--port", str(COMFY_PORT),
    ]
    if os.path.exists(COMFY_MODEL_PATHS_YAML):
        cmd.extend(["--extra-model-paths-config", COMFY_MODEL_PATHS_YAML])
    if os.path.exists(COMFY_INPUT_DIR):
        cmd.extend(["--input-directory", COMFY_INPUT_DIR])
    if os.path.exists(COMFY_OUTPUT_DIR):
        cmd.extend(["--output-directory", COMFY_OUTPUT_DIR])

    os.makedirs(LOG_DIR, exist_ok=True)
    f_out = open(LOG_FILE_COMFY_OUT, "a")
    f_err = open(LOG_FILE_COMFY_ERR, "a")
    process_comfy = subprocess.Popen(
        cmd,
        stdout=f_out,
        stderr=f_err,
        preexec_fn=os.setsid
    )
    ready = await wait_for_comfy_ready(COMFY_PORT, timeout=40.0)
    if not ready:
        print("[MLX Manager] ERROR: ComfyUI backend failed to become ready.", flush=True)
        raise RuntimeError("ComfyUI server failed to start within timeout")
    else:
        print(f"[MLX Manager] ComfyUI server is ready! (PID: {process_comfy.pid})", flush=True)


def build_workflow_payload(action: str, params: dict) -> dict:
    """Load workflow JSON template and inject user parameters."""
    if action in ("t2i", "generate"):
        template_path = os.path.join(COMFY_WORKFLOWS_DIR, "qwen_image_2_1_t2i.json")
        with open(template_path, "r", encoding="utf-8") as f:
            wf = json.load(f)
        prompt = params.get("prompt", "A high quality artistic image")
        neg_prompt = params.get("negative_prompt", "blurry, ugly, distorted, noisy, artifacts")
        seed = params.get("seed", random.randint(1, 2**31 - 1))
        steps = params.get("steps", 14)
        resolution = params.get("resolution", 1024)

        if "5" in wf and "inputs" in wf["5"]:
            wf["5"]["inputs"]["prompt"] = prompt
            wf["5"]["inputs"]["negative_prompt"] = neg_prompt
            wf["5"]["inputs"]["resolution"] = resolution
        if "6" in wf and "inputs" in wf["6"]:
            wf["6"]["inputs"]["seed"] = seed
            wf["6"]["inputs"]["steps"] = steps
        return wf

    elif action in ("edit", "i2i"):
        template_path = os.path.join(COMFY_WORKFLOWS_DIR, "qwen_image_2_1_edit.json")
        with open(template_path, "r", encoding="utf-8") as f:
            wf = json.load(f)
        input_image = params.get("image")
        if not input_image:
            raise ValueError("'image' parameter is required for edit/i2i workflow")

        # If absolute path outside input dir, copy to input dir
        image_name = os.path.basename(input_image)
        dest_path = os.path.join(COMFY_INPUT_DIR, image_name)
        if os.path.abspath(input_image) != os.path.abspath(dest_path) and os.path.exists(input_image):
            os.makedirs(COMFY_INPUT_DIR, exist_ok=True)
            shutil.copy2(input_image, dest_path)

        prompt = params.get("prompt", "In <image1>, edit the image according to prompt")
        if "<image1>" not in prompt:
            prompt = f"In <image1>, {prompt}"
        neg_prompt = params.get("negative_prompt", "blurry, ugly, distorted, noisy, artifacts")
        seed = params.get("seed", random.randint(1, 2**31 - 1))
        steps = params.get("steps", 14)

        if "4" in wf and "inputs" in wf["4"]:
            wf["4"]["inputs"]["image"] = image_name
        if "5" in wf and "inputs" in wf["5"]:
            wf["5"]["inputs"]["prompt"] = prompt
            wf["5"]["inputs"]["negative_prompt"] = neg_prompt
        if "6" in wf and "inputs" in wf["6"]:
            wf["6"]["inputs"]["seed"] = seed
            wf["6"]["inputs"]["steps"] = steps
        return wf

    elif action == "raw" or "workflow" in params:
        return params.get("workflow", params)
    else:
        raise ValueError(f"Unknown workflow action: {action}. Supported: t2i, edit, raw")


async def execute_comfy_workflow(prompt_payload: dict, timeout: float = 300.0) -> dict:
    """Submit prompt to ComfyUI, poll /history/{prompt_id} until completed, and return output file info."""
    client_id = f"mlx-swapper-{uuid.uuid4().hex[:8]}"
    body = {
        "prompt": prompt_payload,
        "client_id": client_id
    }
    async with httpx.AsyncClient(timeout=timeout) as client:
        # 1. Queue prompt
        resp = await client.post(f"http://127.0.0.1:{COMFY_PORT}/prompt", json=body)
        if resp.status_code != 200:
            raise RuntimeError(f"ComfyUI prompt error: {resp.status_code} {resp.text}")
        data = resp.json()
        prompt_id = data.get("prompt_id")
        if not prompt_id:
            raise RuntimeError(f"ComfyUI returned no prompt_id: {data}")
        print(f"[MLX Manager] ComfyUI workflow queued: prompt_id={prompt_id}", flush=True)

        # 2. Poll history until completion
        start_time = time.time()
        while time.time() - start_time < timeout:
            await asyncio.sleep(1.0)
            hist_resp = await client.get(f"http://127.0.0.1:{COMFY_PORT}/history/{prompt_id}")
            if hist_resp.status_code == 200:
                hist_data = hist_resp.json()
                if prompt_id in hist_data:
                    task_info = hist_data[prompt_id]
                    status = task_info.get("status", {})
                    completed = status.get("completed", False)
                    messages = status.get("messages", [])
                    status_str = status.get("status_str", "")

                    if status_str == "error" or any("execution_error" in str(m) for m in messages):
                        raise RuntimeError(f"ComfyUI execution failed: {status}")

                    if completed:
                        outputs = task_info.get("outputs", {})
                        images = []
                        for node_id, node_out in outputs.items():
                            for img in node_out.get("images", []):
                                fn = img.get("filename")
                                subfolder = img.get("subfolder", "")
                                if fn:
                                    full_path = os.path.join(COMFY_OUTPUT_DIR, subfolder, fn)
                                    images.append(full_path)
                        elapsed = round(time.time() - start_time, 2)
                        print(f"[MLX Manager] ComfyUI workflow completed in {elapsed}s! Outputs: {images}", flush=True)
                        return {
                            "prompt_id": prompt_id,
                            "completed": True,
                            "elapsed_seconds": elapsed,
                            "images": images,
                            "outputs": outputs
                        }
        raise TimeoutError(f"ComfyUI workflow {prompt_id} timed out after {timeout}s")


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
                continue
            elif isinstance(parsed, list):
                for item in parsed:
                    if isinstance(item, dict) and "name" in item:
                        tool_calls.append(item)
                continue
        except Exception:
            pass

        # 1-b. XML function call syntax: <function=name> <parameter=key>value</parameter> </function>
        fn_match = re.search(r"<function=([a-zA-Z0-9_-]+)>(.*?)</function>", block, re.DOTALL)
        if fn_match:
            fn_name = fn_match.group(1).strip()
            fn_body = fn_match.group(2)
            params = {}
            param_matches = re.findall(r"<parameter=([a-zA-Z0-9_-]+)>(.*?)</parameter>", fn_body, re.DOTALL)
            for p_name, p_val in param_matches:
                p_val_str = p_val.strip()
                try:
                    params[p_name] = json.loads(p_val_str)
                except Exception:
                    params[p_name] = p_val_str
            tool_calls.append({"name": fn_name, "arguments": params})

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
        target_model = PORT_TO_MODEL.get(internal_port)
        if target_model and "model" in _data and _data["model"] != target_model:
            _data["model"] = target_model

        # Dynamically forward Hermes / client reasoning & chat_template_kwargs to mlx_lm
        chat_template_kwargs = _data.get("chat_template_kwargs") or {}

        if "enable_thinking" not in chat_template_kwargs:
            reasoning_effort = _data.get("reasoning_effort")
            if not reasoning_effort and isinstance(_data.get("reasoning"), dict):
                reasoning_effort = _data["reasoning"].get("effort")

            thinking_opt = _data.get("thinking")
            if isinstance(thinking_opt, dict) and thinking_opt.get("type") == "disabled":
                reasoning_effort = "none"

            if reasoning_effort in ("none", "off", "disabled"):
                chat_template_kwargs["enable_thinking"] = False
                print(f"[MLX Proxy] 🧠 Thinking OFF (from Hermes reasoning_effort={reasoning_effort})", flush=True)
            elif reasoning_effort in ("low", "minimal", "medium", "high"):
                chat_template_kwargs["enable_thinking"] = True
                print(f"[MLX Proxy] 🧠 Thinking ON (from Hermes reasoning_effort={reasoning_effort})", flush=True)
            else:
                chat_template_kwargs["enable_thinking"] = False
        else:
            print(f"[MLX Proxy] 🧠 Thinking from Hermes extra_body chat_template_kwargs: {chat_template_kwargs.get('enable_thinking')}", flush=True)

        # Ensure thinking is ALWAYS forced OFF for compression / summarization requests
        if not _data.get("tools"):
            first_msg_content = ""
            if _data.get("messages") and len(_data["messages"]) > 0:
                first_msg_content = str(_data["messages"][0].get("content", "")).lower()
            if any(k in first_msg_content for k in ("summar", "compact", "recap", "conversation history")):
                chat_template_kwargs["enable_thinking"] = False
                print("[MLX Proxy] 🗜️ Compression/summary task detected -> Force thinking OFF!", flush=True)

        _data["chat_template_kwargs"] = chat_template_kwargs

        body = json.dumps(_data).encode("utf-8")
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
                        full_reasoning = ""
                        already_native = False
                        req_id = f"chatcmpl-{uuid.uuid4().hex[:8]}"
                        model_name = os.path.basename(MODEL_MAIN_PATH)

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
                                            if delta.get("reasoning"):
                                                full_reasoning += delta["reasoning"]
                                            elif delta.get("reasoning_content"):
                                                full_reasoning += delta["reasoning_content"]
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
                            # Check both content and reasoning text for tool call blocks
                            extracted = extract_tool_calls(full_content)
                            if not extracted and full_reasoning:
                                extracted = extract_tool_calls(full_reasoning)
                            if extracted:
                                print(f"[MLX Proxy] ⚡️ Successfully adapted {len(extracted)} text/reasoning tool call(s) to native OpenAI tool_calls!", flush=True)
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
                        if not extracted:
                            extracted = extract_tool_calls(msg.get("reasoning", "") or msg.get("reasoning_content", ""))
                        if extracted:
                            print(f"[MLX Proxy] ⚡️ Successfully adapted {len(extracted)} non-stream text/reasoning tool call(s) to native OpenAI format!", flush=True)
                            adapted = make_non_streaming_tool_call_response(
                                payload.get("id", f"chatcmpl-{uuid.uuid4().hex[:8]}"),
                                payload.get("model", os.path.basename(MODEL_MAIN_PATH)),
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
    if is_running(process_main):
        running_models.append({"name": os.path.basename(MODEL_MAIN_PATH), "pid": process_main.pid, "port": PORT_MAIN_INTERNAL})
    if is_running(process_compact):
        running_models.append({"name": "compact-7b", "pid": process_compact.pid, "port": PORT_COMPACT_INTERNAL})
    if is_running(process_comfy):
        running_models.append({"name": "comfyui", "pid": process_comfy.pid, "port": COMFY_PORT})
    return {
        "status": "ok",
        "active_models_count": len(running_models),
        "active_models": running_models,
        "last_active": last_active_model,
        "is_manual_locked": is_manual_locked
    }


# ── App 1: Port 1234 (Main Qwen3.6 Model Proxy) ───────────────────
app_main = FastAPI(title="MLX Main Model Proxy (Qwen3.6-35B)")


@app_main.post("/v1/comfy/run")
async def comfy_run(request: Request):
    """Execute ComfyUI workflow with dynamic swap:
       Stop Main 35B -> Start ComfyUI -> Inject & Run Workflow -> Await outputs -> Stop ComfyUI -> Restore Main 35B.
    """
    try:
        data = await request.json()
    except Exception:
        data = {}

    action = data.get("action", "t2i")
    timeout = float(data.get("timeout", 300.0))

    try:
        wf_payload = build_workflow_payload(action, data)
    except Exception as e:
        return Response(content=json.dumps({"error": f"Invalid workflow configuration: {str(e)}"}), status_code=400, media_type="application/json")

    await swap_lock.acquire()
    try:
        print(f"[MLX Manager] ComfyUI run requested (action={action})! Swapping Main model out...", flush=True)
        await _stop_main_unlocked()
        await _stop_compact_unlocked()
        await _start_comfy_unlocked()

        result = await execute_comfy_workflow(wf_payload, timeout=timeout)
        return result
    except Exception as e:
        print(f"[MLX Manager] ERROR in comfy_run: {e}", flush=True)
        return Response(content=json.dumps({"error": str(e)}), status_code=500, media_type="application/json")
    finally:
        try:
            print("[MLX Manager] ComfyUI workflow finished. Stopping ComfyUI and restoring Main model...", flush=True)
            await _stop_comfy_unlocked()
            await _start_main_unlocked()
            print("[MLX Manager] Main model restored successfully!", flush=True)
        finally:
            swap_lock.release()
            print("[MLX Manager] Swap lock released after ComfyUI run.", flush=True)


@app_main.post("/v1/swap/lock")
async def swap_manual_lock(request: Request):
    global is_manual_locked
    target = request.query_params.get("target", "comfyui")
    await swap_lock.acquire()
    try:
        if target == "comfyui":
            await _stop_main_unlocked()
            await _stop_compact_unlocked()
            await _start_comfy_unlocked()
        is_manual_locked = True
        return {"status": "locked", "target": target}
    except Exception as e:
        swap_lock.release()
        return Response(content=json.dumps({"error": str(e)}), status_code=500, media_type="application/json")


@app_main.post("/v1/swap/unlock")
async def swap_manual_unlock():
    global is_manual_locked
    try:
        await _stop_comfy_unlocked()
        await _stop_compact_unlocked()
        await _start_main_unlocked()
        is_manual_locked = False
        return {"status": "unlocked", "active_model": "main"}
    finally:
        if swap_lock.locked():
            swap_lock.release()


@app_main.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_main(request: Request, path: str):
    global last_active_model
    if request.method == "GET" and path in ("health", "status"):
        return get_system_status()
    if request.method == "GET" and path in ("v1/models", "models"):
        return {
            "object": "list",
            "data": [
                {
                    "id": MODEL_MAIN_PATH,
                    "object": "model",
                    "created": int(time.time()),
                    "owned_by": "mlx_main"
                },
                {
                    "id": os.path.basename(MODEL_MAIN_PATH),
                    "object": "model",
                    "created": int(time.time()),
                    "owned_by": "mlx_main"
                }
            ]
        }

    async with swap_lock:
        last_active_model = "main"
        await _stop_compact_unlocked()
        await _start_main_unlocked()

    return await generic_proxy(request, path, PORT_MAIN_INTERNAL)


# ── App 2: Port 1235 (Compaction Model Proxy) ────────────────────
app_compact = FastAPI(title="MLX Compaction Proxy")

@app_compact.api_route("/{path:path}", methods=["GET", "POST", "PUT", "DELETE", "OPTIONS", "HEAD"])
async def proxy_compact(request: Request, path: str):
    if request.method == "GET" and path in ("health", "status"):
        return get_system_status()
    if request.method != "POST" or "chat/completions" not in path:
        if path in ("v1/models", "models"):
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
        return Response(content='{"status": "ok"}', status_code=200, media_type="application/json")

    # For actual compaction (POST chat/completions):
    # Hold swap_lock across the ENTIRE compaction and restore Main model afterwards
    await swap_lock.acquire()
    try:
        print(f"[MLX Manager] Compaction requested! Stopping Main model and starting Compaction model...", flush=True)
        await _stop_main_unlocked()
        await _start_compact_unlocked()

        url = f"http://127.0.0.1:{PORT_COMPACT_INTERNAL}/{path}"
        headers = dict(request.headers)
        headers.pop("host", None)
        headers.pop("content-length", None)

        body = await request.body()
        try:
            payload = json.loads(body)
            messages = payload.get("messages", [])
            if messages and isinstance(messages, list):
                last_msg = messages[-1]
                if isinstance(last_msg, dict) and last_msg.get("role") == "user":
                    content = last_msg.get("content", "")
                    if isinstance(content, str) and "CRITICAL CONSTRAINTS FOR COMPACTNESS" not in content:
                        last_msg["content"] = content + (
                            "\n\nCRITICAL CONSTRAINTS FOR COMPACTNESS:\n"
                            "- Keep the entire summary strictly under 1,500 tokens.\n"
                            "- Summarize actions concisely as brief 1-line bullet points without pasting code blocks, diffs, or raw terminal dumps.\n"
                            "- Group similar repetitive tool actions together.\n"
                            "- Conclude immediately with ## Active State and your next steps.\n"
                        )
                        body = json.dumps(payload).encode("utf-8")
        except Exception as ex:
            print(f"[MLX Manager] Notice: could not inject compactness constraint: {ex}", flush=True)

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
                    # Safety guarantee: ensure Hermes never aborts on length finish_reason
                    if b'"finish_reason":"length"' in chunk or b'"finish_reason": "length"' in chunk:
                        chunk = chunk.replace(b'"finish_reason":"length"', b'"finish_reason":"stop"').replace(b'"finish_reason": "length"', b'"finish_reason": "stop"')
                    yield chunk
            finally:
                try:
                    await resp.aclose()
                    await client.aclose()
                    print(f"[MLX Manager] Compaction stream complete! Restoring Main model (Qwen3.6)...", flush=True)
                    await _stop_compact_unlocked()
                    await _start_main_unlocked()
                    print(f"[MLX Manager] Main model restored successfully!", flush=True)
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
    print(f"[MLX Manager] Starting MLX Dynamic Swapper (Main: Qwen3.6-35B on {PROXY_PORT_MAIN}, Compactor: Qwen2.5-7B on {PROXY_PORT_COMPACT})", flush=True)
    kill_rogue_mlx_servers()
    await asyncio.sleep(0.5)
    async with swap_lock:
        # Start Qwen3.6-35B as default primary model on boot
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

    servers = [server_main, server_compact]

    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, lambda: asyncio.create_task(shutdown_all(*servers)))

    print(f"[MLX Manager] Proxies listening: Main on {PROXY_PORT_MAIN}, Compactor on {PROXY_PORT_COMPACT}", flush=True)
    await asyncio.gather(*[s.serve() for s in servers])


async def shutdown_all(*servers: uvicorn.Server):
    print("[MLX Manager] Shutting down manager and stopping MLX processes...", flush=True)
    for s in servers:
        s.should_exit = True
    async with swap_lock:
        await _stop_main_unlocked()
        await _stop_compact_unlocked()
        await _stop_comfy_unlocked()


if __name__ == "__main__":
    asyncio.run(main())
