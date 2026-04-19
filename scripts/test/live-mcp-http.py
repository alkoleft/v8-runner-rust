#!/usr/bin/env python3

import json
import os
import socket
import subprocess
import sys
import time
from typing import Optional
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

if sys.version_info < (3, 8):
    raise SystemExit("live-mcp-http.py requires Python 3.8 or newer")


ROOT_DIR = Path(__file__).resolve().parents[2]

CONFIG_VALUE = os.environ.get("V8TR_REAL_CONFIG")
CONFIG_PATH = Path(CONFIG_VALUE) if CONFIG_VALUE else None
BIN_PATH = Path(os.environ.get("V8TR_BIN", str(ROOT_DIR / "target/debug/v8-runner")))
MCP_URL = os.environ.get("V8TR_MCP_URL", "http://127.0.0.1:3000/mcp")
SMOKE_MODULE = os.environ.get("V8TR_SMOKE_MODULE", "ЮТДымовыеТесты")
EDT_PROJECT = os.environ.get("V8TR_EDT_PROJECT", "configuration")
HTTP_TIMEOUT = float(os.environ.get("V8TR_HTTP_TIMEOUT_SECONDS", "600"))
STARTUP_TIMEOUT = float(os.environ.get("V8TR_SERVER_STARTUP_TIMEOUT_SECONDS", "60"))
SERVER_LOG = ROOT_DIR / "target/manual-tests/live-mcp-http/server.stderr.log"


def ensure_binary() -> None:
    if BIN_PATH.exists() and os.access(BIN_PATH, os.X_OK):
        return
    print("Building v8-runner binary...", file=sys.stderr)
    subprocess.run(
        ["cargo", "build", "--locked", "--bin", "v8-runner"],
        cwd=ROOT_DIR,
        check=True,
        stdout=subprocess.DEVNULL,
    )


def wait_for_server(url: str, timeout_seconds: float) -> None:
    parsed = urllib.parse.urlparse(url)
    host = parsed.hostname or "127.0.0.1"
    port = parsed.port or 80
    deadline = time.time() + timeout_seconds
    while time.time() < deadline:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.settimeout(1.0)
            if sock.connect_ex((host, port)) == 0:
                return
        time.sleep(0.2)
    raise RuntimeError(f"Timed out waiting for MCP HTTP server at {url}")


def extract_sse_json(body: str) -> dict:
    for event in body.split("\n\n"):
        lines = []
        for line in event.splitlines():
            if line.startswith("data:"):
                payload = line[len("data:") :].strip()
                if payload:
                    lines.append(payload)
        if lines:
            return json.loads("\n".join(lines))
    raise RuntimeError(f"No JSON SSE payload found in response body:\n{body}")


def http_post(url: str, payload: dict, session_id: Optional[str] = None) -> tuple[int, dict, str]:
    headers = {
        "Accept": "application/json, text/event-stream",
        "Content-Type": "application/json",
    }
    if session_id:
        headers["Mcp-Session-Id"] = session_id
    request = urllib.request.Request(
        url=url,
        data=json.dumps(payload).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT) as response:
            body = response.read().decode("utf-8")
            return response.getcode(), dict(response.info()), body
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", errors="replace")
        return error.code, dict(error.headers), body


def assert_true(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def get_header_ci(headers: dict, name: str) -> Optional[str]:
    target = name.casefold()
    for key, value in headers.items():
        if key.casefold() == target:
            return value
    return None


def initialize_session(url: str) -> tuple[str, dict]:
    status, headers, body = http_post(
        url,
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "live-mcp-http", "version": "1.0.0"},
            },
        },
    )
    assert_true(status == 200, f"initialize returned HTTP {status}: {body}")
    session_id = get_header_ci(headers, "Mcp-Session-Id")
    assert_true(bool(session_id), f"initialize did not return Mcp-Session-Id: {headers}")
    payload = extract_sse_json(body)
    return session_id, payload


def send_initialized(url: str, session_id: str) -> None:
    status, _headers, body = http_post(
        url,
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        session_id=session_id,
    )
    assert_true(status == 202, f"notifications/initialized returned HTTP {status}: {body}")


def tools_list(url: str, session_id: str) -> dict:
    status, _headers, body = http_post(
        url,
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        session_id=session_id,
    )
    assert_true(status == 200, f"tools/list returned HTTP {status}: {body}")
    return extract_sse_json(body)


def call_tool(url: str, session_id: str, request_id: int, name: str, arguments: dict) -> dict:
    status, _headers, body = http_post(
        url,
        {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        },
        session_id=session_id,
    )
    assert_true(status == 200, f"tools/call {name} returned HTTP {status}: {body}")
    return extract_sse_json(body)


def expect_tool_success(payload: dict, tool_name: str) -> dict:
    content = payload.get("result", {}).get("structuredContent")
    assert_true(
        isinstance(content, dict),
        f"{tool_name}: missing structuredContent in payload {json.dumps(payload, ensure_ascii=False)}",
    )
    assert_true(
        content.get("status") == "success",
        f"{tool_name}: MCP tool status is not success: {json.dumps(content, ensure_ascii=False)}",
    )
    result = content.get("result")
    assert_true(
        isinstance(result, dict),
        f"{tool_name}: missing result object in structuredContent {json.dumps(content, ensure_ascii=False)}",
    )
    return result


def main() -> int:
    if CONFIG_PATH is None:
        print(
            "V8TR_REAL_CONFIG is not set.\n"
            "Example:\n"
            "  export V8TR_REAL_CONFIG=/home/alko/develop/open-source/ai/mcp/onec-client-mcp-devkit/.agents/tools/onec-client-mcp-devkit.edt.yaml",
            file=sys.stderr,
        )
        return 2

    if not CONFIG_PATH.is_file():
        print(
            f"Live config not found: {CONFIG_PATH}\n"
            "Override it with V8TR_REAL_CONFIG=/abs/path/to/v8project.yaml",
            file=sys.stderr,
        )
        return 2

    ensure_binary()

    SERVER_LOG.parent.mkdir(parents=True, exist_ok=True)
    with SERVER_LOG.open("wb") as stderr_log:
        server = subprocess.Popen(
            [str(BIN_PATH), "--config", str(CONFIG_PATH), "mcp", "serve", "http"],
            cwd=ROOT_DIR,
            stdout=subprocess.DEVNULL,
            stderr=stderr_log,
        )
        try:
            wait_for_server(MCP_URL, STARTUP_TIMEOUT)

            session_id, initialize_payload = initialize_session(MCP_URL)
            capabilities = initialize_payload.get("result", {}).get("capabilities", {})
            assert_true(
                "tools" in capabilities,
                f"initialize response does not advertise tools capability: {initialize_payload}",
            )

            send_initialized(MCP_URL, session_id)

            list_payload = tools_list(MCP_URL, session_id)
            actual_tools = sorted(
                tool["name"] for tool in list_payload.get("result", {}).get("tools", [])
            )
            required_tools = {
                "build_project",
                "check_syntax_edt",
                "run_module_tests",
            }
            assert_true(
                required_tools.issubset(set(actual_tools)),
                "tools/list does not contain the required subset.\n"
                f"Required: {sorted(required_tools)}\nActual: {actual_tools}",
            )

            build_payload = call_tool(
                MCP_URL,
                session_id,
                10,
                "build_project",
                {"fullRebuild": False},
            )
            build_result = expect_tool_success(build_payload, "build_project")
            assert_true(
                build_result.get("success") is True,
                f"build_project result.success is not true: {json.dumps(build_result, ensure_ascii=False)}",
            )

            syntax_payload = call_tool(
                MCP_URL,
                session_id,
                11,
                "check_syntax_edt",
                {"projectName": EDT_PROJECT},
            )
            syntax_result = expect_tool_success(syntax_payload, "check_syntax_edt")
            assert_true(
                syntax_result.get("check_result") in {"clean", "issues_found"},
                "check_syntax_edt returned unexpected check_result: "
                f"{json.dumps(syntax_result, ensure_ascii=False)}",
            )

            tests_payload = call_tool(
                MCP_URL,
                session_id,
                12,
                "run_module_tests",
                {"moduleName": SMOKE_MODULE},
            )
            tests_result = expect_tool_success(tests_payload, "run_module_tests")
            assert_true(
                tests_result.get("success") is True,
                f"run_module_tests result.success is not true: {json.dumps(tests_result, ensure_ascii=False)}",
            )

            print("Live MCP HTTP smoke completed successfully.")
            print(f"Server stderr log: {SERVER_LOG}")
            return 0
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(str(error), file=sys.stderr)
        print(f"Server stderr log: {SERVER_LOG}", file=sys.stderr)
        raise
