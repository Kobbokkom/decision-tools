#!/usr/bin/env python3
"""Collect sample JSON evidence from a registered workflow."""
import json
import sys

payload = json.load(sys.stdin)
inputs, results = payload["input"], payload["results"]
operation = sys.argv[1]
if operation == "pr":
    output = {"changed_paths": inputs["paths"], "summary": inputs["summary"]}
elif operation == "deployment":
    output = {
        "healthy": 200 <= inputs["http_status"] < 300,
        "expected_version": inputs["expected_version"],
        "actual_version": inputs["actual_version"],
    }
elif operation == "result":
    decision = results.get("classify", results.get("verify"))
    output = {"follow_up": "human_review", "judgment": decision["answers"]["route"]}
else:
    raise SystemExit("unknown sample operation")
print(json.dumps(output))
