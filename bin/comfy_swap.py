#!/usr/bin/env python3
"""
comfy_swap.py — CLI and Hermes Tool helper to interact with MLX Swapper's ComfyUI endpoint.

Usage:
  python3 comfy_swap.py generate --prompt "A beautiful cybernetic tiger, cinematic lighting, 8k"
  python3 comfy_swap.py edit --image "/path/to/img.png" --prompt "Make the sky nighttime with stars"
"""

import argparse
import json
import os
import sys
import urllib.request
import urllib.error

SWAPPER_URL = os.environ.get("SWAPPER_URL", "http://127.0.0.1:1234")


def call_swapper(payload: dict, timeout: float = 600.0) -> dict:
    url = f"{SWAPPER_URL}/v1/comfy/run"
    data = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"}
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            res_body = response.read().decode("utf-8")
            return json.loads(res_body)
    except urllib.error.HTTPError as e:
        err_msg = e.read().decode("utf-8", errors="ignore")
        try:
            err_json = json.loads(err_msg)
            return {"status": "error", "error": err_json.get("error", err_msg)}
        except Exception:
            return {"status": "error", "error": f"HTTP {e.code}: {err_msg}"}
    except Exception as e:
        return {"status": "error", "error": str(e)}


def main():
    parser = argparse.ArgumentParser(description="ComfyUI Dynamic Swapper Runner")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # 1. generate (t2i)
    p_gen = subparsers.add_parser("generate", help="Generate an image from prompt (Text-to-Image)")
    p_gen.add_argument("--prompt", "-p", required=True, help="Positive text prompt")
    p_gen.add_argument("--negative-prompt", "-n", default="blurry, ugly, distorted, noisy, artifacts", help="Negative prompt")
    p_gen.add_argument("--seed", type=int, default=None, help="Random seed")
    p_gen.add_argument("--steps", type=int, default=14, help="Sampling steps")
    p_gen.add_argument("--resolution", type=int, default=1024, help="Resolution")

    # 2. edit (i2i)
    p_edit = subparsers.add_parser("edit", help="Edit an existing image (Image-to-Image)")
    p_edit.add_argument("--image", "-i", required=True, help="Path to input image file")
    p_edit.add_argument("--prompt", "-p", required=True, help="Edit instruction prompt")
    p_edit.add_argument("--negative-prompt", "-n", default="blurry, ugly, distorted, noisy, artifacts", help="Negative prompt")
    p_edit.add_argument("--seed", type=int, default=None, help="Random seed")
    p_edit.add_argument("--steps", type=int, default=14, help="Sampling steps")

    args = parser.parse_args()

    payload = {"action": args.command}
    if args.command == "generate":
        payload["prompt"] = args.prompt
        payload["negative_prompt"] = args.negative_prompt
        payload["steps"] = args.steps
        payload["resolution"] = args.resolution
        if args.seed is not None:
            payload["seed"] = args.seed

    elif args.command == "edit":
        image_path = os.path.abspath(os.path.expanduser(args.image))
        if not os.path.exists(image_path):
            print(json.dumps({"status": "error", "error": f"Image file not found: {image_path}"}))
            sys.exit(1)
        payload["image"] = image_path
        payload["prompt"] = args.prompt
        payload["negative_prompt"] = args.negative_prompt
        payload["steps"] = args.steps
        if args.seed is not None:
            payload["seed"] = args.seed

    print(f"[*] Requesting ComfyUI {args.command} via MLX Swapper...", file=sys.stderr)
    result = call_swapper(payload)
    print(json.dumps(result, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
