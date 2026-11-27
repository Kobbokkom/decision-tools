#!/usr/bin/env python3
"""Exercise the published MCP tools through stdio and authenticated HTTP."""
import contextlib
import json
import os
import pathlib
import select
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
PROTOCOL = "2025-11-25"
TOKEN = "test-token-with-more-than-32-characters"
INIT = {
    "protocolVersion": PROTOCOL,
    "capabilities": {},
    "clientInfo": {"name": "decision-tools-smoke", "version": "0.1"},
}
PR = {"paths": ["README.md"], "summary": "Update an example description"}
DEPLOYMENT = {"expected_version": "0.1.0", "actual_version": "0.1.0", "http_status": 200}
TOOLS = {"decide", "workflow_list", "workflow_start", "workflow_get", "workflow_cancel"}
LOCAL_TOOLS = {"decide", "grep", "browse"}
TERMINAL = {"succeeded", "failed", "cancelled", "interrupted"}


def message(method, params, call=True):
    packet = {"jsonrpc": "2.0", "method": method, "params": params}
    if call:
        packet["id"] = 1
    return json.dumps(packet).encode()


def call(send, name, params=None, error=False):
    result = send("tools/call", {"name": name, "arguments": params or {}})
    assert bool(result.get("isError", False)) == error, result
    return result["structuredContent"]


def initialize(send, expected=TOOLS):
    assert send("initialize", INIT)["serverInfo"]["name"] == "decision-tools"
    send("notifications/initialized", {}, False)
    tools = send("tools/list", {})
    assert {tool["name"] for tool in tools["tools"]} == expected, tools


def examples(send):
    listing = call(send, "workflow_list")
    assert listing["backend"] == "demo", listing
    known = {workflow["name"] for workflow in listing["workflows"]}
    assert {"deployment-check", "pr-triage"} <= known
    smoke = {
        "evidence": {"status": "operational"},
        "questions": {"healthy": {"type": "noul", "instructions": "Is it healthy?"}},
        "allow_provider": False,
    }
    judgment = call(send, "decide", smoke)
    assert judgment["usage"]["cost"] == 0
    assert judgment["answers"]["healthy"]["type"] == "noul"
    assert "unknown workflow" in call(
        send, "workflow_start", {"workflow": "unregistered"}, error=True
    )["error"]
    assert "must be string_list" in call(
        send, "workflow_start", {"workflow": "pr-triage", "input": {"paths": "x", "summary": "x"}}, error=True
    )["error"]
    for workflow, inputs, expected in [
        ("pr-triage", PR, {"collect", "classify", "handoff"}),
        ("deployment-check", DEPLOYMENT, {"collect", "verify"}),
    ]:
        started = call(send, "workflow_start", {"workflow": workflow, "input": inputs})
        result = wait_until(
            lambda: call(send, "workflow_get", {"id": started["id"]}),
            lambda run: run["status"] in TERMINAL,
        )
        assert result["status"] == "succeeded", result
        assert set(result["results"]) == expected, result


def wait_until(read, condition, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if condition(value):
            return value
        time.sleep(0.1)
    raise AssertionError(f"condition did not pass: {value!r}")


def stop(process):
    if process.poll() is None:
        process.terminate()
    try:
        process.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate(timeout=5)


def stdio(binary, config, database, predicate, local_mode=False):
    command = [binary, "--demo", "local"] if local_mode else [binary, "--config", config, "--db", database, "--demo", "stdio"]
    with subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    ) as process:
        try:
            def send(method, params, reply=True):
                process.stdin.write(message(method, params, reply) + b"\n")
                process.stdin.flush()
                if reply:
                    ready, _, _ = select.select([process.stdout], [], [], 12)
                    assert ready, method
                    line = process.stdout.readline()
                    assert line, method
                    response = json.loads(line)
                    assert "error" not in response, response
                    return response["result"]

            initialize(send, LOCAL_TOOLS if local_mode else TOOLS)
            predicate(send)
        finally:
            stop(process)


def local_tools(send, tree):
    found = call(send, "grep", {"question": "Where is the event sent?", "root": str(tree), "allow_provider": True})
    assert found["results"][0]["path"] == "src/events.rs", found
    assert "send_event" in found["results"][0]["excerpt"]
    assert found["coverage"]["files_found"] == 1 and not found["coverage"]["incomplete"], found
    assert "allow_provider" in call(
        send, "grep", {"question": "x", "root": str(tree), "allow_provider": False}, error=True
    )["error"]
    assert "http" in call(
        send, "browse", {"url": "file:///etc/hosts", "task": "x", "allow_provider": True}, error=True
    )["error"]


def http_send(url, token, origin=None):
    def send(method, params, reply=True):
        request = urllib.request.Request(url, data=message(method, params, reply), headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Accept": "application/json, text/event-stream",
            "MCP-Protocol-Version": PROTOCOL,
        })
        if origin:
            request.add_header("Origin", origin)
        with urllib.request.urlopen(request, timeout=8) as response:
            if not reply:
                assert response.status == 202
                return None
            result = json.load(response)
            assert "error" not in result, result
            return result["result"]

    return send


def denied(send, expected):
    try:
        send("tools/list", {})
    except urllib.error.HTTPError as error:
        assert error.code == expected, error
    else:
        raise AssertionError(f"expected HTTP {expected}")


def http_examples(url, token):
    send = http_send(url, token)
    denied(http_send(url, "invalid"), 401)
    denied(http_send(url, token, "https://untrusted.example"), 403)
    initialize(send)
    examples(send)


def lifecycle(send, directory):
    def start(workflow):
        return call(send, "workflow_start", {"workflow": workflow})["id"]

    def status(run_id):
        return call(send, "workflow_get", {"id": run_id})

    cancelled = start("descendant")
    pid_file = directory / "child.pid"
    pid = wait_until(lambda: pid_file.read_text() if pid_file.exists() else "", str.isdigit)
    call(send, "workflow_cancel", {"id": cancelled})
    wait_until(lambda: status(cancelled), lambda run: run["status"] == "cancelled")
    wait_until(
        lambda: subprocess.run(["ps", "-o", "stat=", "-p", pid], capture_output=True, text=True).stdout.strip(),
        lambda state: not state or state.startswith("Z"),
    )

    timed_out = wait_until(
        lambda run_id=start("timeout"): status(run_id), lambda run: run["status"] == "failed"
    )
    assert "timed out" in timed_out["error"]
    limited = wait_until(
        lambda run_id=start("output"): status(run_id), lambda run: run["status"] == "failed"
    )
    assert "output exceeds" in limited["error"]

    def scheduled():
        return call(send, "workflow_list")["schedules"][0]["latest_run_id"]

    run_id = wait_until(scheduled, bool)
    run = wait_until(lambda: status(run_id), lambda record: record["status"] == "succeeded")
    assert run["schedule"] == "test-clock"
    assert run["scheduled_at"] % 2 == 0


def local(binary):
    with tempfile.TemporaryDirectory() as directory:
        temporary = pathlib.Path(directory)
        config = ROOT / "examples" / "workflows.toml"
        stdio(binary, config, temporary / "stdio.db", examples)
        spawn = temporary / "spawn.py"
        spawn.write_text("""#!/usr/bin/env python3
from pathlib import Path
from subprocess import Popen
child = Popen(["/bin/sleep", "30"])
Path("child.pid").write_text(str(child.pid))
child.wait()
print("{}")
""")
        spawn.chmod(0o700)
        registered = temporary / "lifecycle.toml"
        registered.write_text("""
[scripts.timeout]
program = "/bin/sleep"
args = ["30"]
timeout_secs = 1

[scripts.descendant]
program = "./spawn.py"
timeout_secs = 10

[workflows.descendant]
description = "Cancel an active process group."
entry = "run"
steps.run = { kind = "script", script = "descendant", next = "$done" }

[scripts.output]
program = "/usr/bin/yes"
timeout_secs = 3

[scripts.quick]
program = "/bin/echo"
args = ["{}"]
timeout_secs = 2

[workflows.timeout]
description = "Timeout and cancellation test."
entry = "run"
steps.run = { kind = "script", script = "timeout", next = "$done" }

[workflows.output]
description = "Output cap test."
entry = "run"
steps.run = { kind = "script", script = "output", next = "$done" }

[workflows.clock]
description = "Schedule test."
entry = "run"
steps.run = { kind = "script", script = "quick", next = "$done" }

[schedules.test-clock]
workflow = "clock"
every_secs = 2
""")
        invalid = temporary / "invalid.toml"
        invalid.write_text(registered.read_text().replace('script = "timeout", next = "$done"', 'script = "timeout", next = "run"'))
        checked = subprocess.run([binary, "--config", invalid, "check-config"], capture_output=True, text=True)
        assert checked.returncode != 0 and "cycle" in checked.stderr, checked.stderr
        stdio(binary, registered, temporary / "lifecycle.db", lambda send: lifecycle(send, temporary))
        tree = temporary / "tree"
        (tree / "src").mkdir(parents=True)
        (tree / "src/events.rs").write_text("fn send_event() {\n    post();\n}\n")
        stdio(binary, None, None, lambda send: local_tools(send, tree), local_mode=True)

        with socket.socket() as reserved:
            reserved.bind(("127.0.0.1", 0))
            port = reserved.getsockname()[1]
        endpoint = f"http://127.0.0.1:{port}"
        env = {**os.environ, "DECISION_TOOLS_TOKEN": TOKEN}
        with subprocess.Popen(
            [binary, "--config", config, "--db", temporary / "http.db", "--demo", "serve",
             "--listen", f"127.0.0.1:{port}"],
            env=env,
        ) as process:
            try:
                def ready():
                    with contextlib.suppress(OSError), urllib.request.urlopen(endpoint + "/readyz", timeout=1) as response:
                        return response.read()
                    return None

                assert wait_until(ready, bool) == b"ready (demo)"
                http_examples(endpoint + "/mcp", TOKEN)
            finally:
                stop(process)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--url":
        http_examples(sys.argv[2], os.environ["DECISION_TOOLS_TOKEN"])
    else:
        local(os.path.abspath(sys.argv[1]))
    print("MCP smoke checks passed")
