<img src="assets/icon.png" width="96" height="96" alt="Decision Tools branching-path icon">

# Decision Tools

A self-hosted Rust MCP server for typed judgments and workflows built from registered scripts. Use it to triage a PR, check a release, or run a recurring procedure and retrieve its result later.

## Try it locally

Requires Rust 1.93+ on macOS or Linux and Python 3 for the example scripts. Clone the repo and build it with `cargo build --locked --release`.

Register a stdio MCP server named `decision-tools` in your client. Use the absolute paths for your checkout and a state directory that already exists.

```json
{
  "command": "/path/to/decision-tools/target/release/decision-tools",
  "args": [
    "--config", "/path/to/decision-tools/examples/workflows.toml",
    "--db", "/path/to/state/runs.sqlite3",
    "--demo", "stdio"
  ]
}
```

Call `workflow_list`, then start either example.

| Workflow | Example input |
| --- | --- |
| `pr-triage` | `{"paths":["README.md"],"summary":"Clarify the sample"}` |
| `deployment-check` | `{"expected_version":"0.1.0","actual_version":"0.1.0","http_status":200}` |

The deployment example takes a captured HTTP response as a fixture. An operator can register a read-only probe against an approved service to collect live evidence. Demo answers are deterministic fixtures.

| MCP tool | Purpose |
| --- | --- |
| `decide` | Judge evidence against named choice, yes/no, or score questions. |
| `workflow_list` | Discover the backend, workflow inputs, UTC schedules, and the latest scheduled run IDs. |
| `workflow_start` | Accept validated input and return a run ID. |
| `workflow_get` | Read one run's status, completed steps, and usage. |
| `workflow_cancel` | Cancel pending execution or signal a running step. Completed external effects may remain. |

## Local tools: grep and browse

`decision-tools local` serves `decide`, `grep` and `browse` on stdio for one machine. It needs no registry or database, so every client session can start its own copy. Run it where the source and the browser are, since the shared server cannot see your files or localhost.

| MCP tool | Purpose |
| --- | --- |
| `grep` | Find where a described behavior is implemented. The provider rates file previews, then line windows of the best files. Returns ranked paths, line ranges and excerpts. Key files, credential stores, lockfiles, hidden and ignored paths are never read. |
| `browse` | Check or operate a page in a private headless Chrome with a throwaway profile. Each step reads a numbered element table and asks for one operation and one target in a single request. Typing needs `allow_typing`. Set `DECISION_TOOLS_CHROME` when Chrome is not in a standard place. |

Both tools send data to OpenRouter only when the call sets `allow_provider`. They use `DECISION_TOOLS_JEV_MODEL` (default `typesafe/jev-1.14`). `browse` writes field values with `DECISION_TOOLS_TEXT_MODEL`. Register the client command as `decision-tools local` with `OPENROUTER_API_KEY_FILE` set.

For live decisions, grant the process an `OPENROUTER_API_KEY_FILE` or `OPENROUTER_API_KEY` and remove `--demo`. The default model is `liquid/d1.2-ultra`. Override it through `DECISION_TOOLS_MODEL`. A direct call accepts `evidence`, a `questions` map, and `allow_provider` after confirming where evidence can be sent. The answer includes provider usage when supplied.

## Register your own scripts

Use [the example registry](examples/workflows.toml) as a starting point. Each executable gets operator-defined arguments and one JSON object on stdin, with `input` and previous step `results`. Exit successfully with one JSON value on stdout. The service validates input before accepting a run and bounds output to 64 KiB. `decision-tools --config workflows.toml check-config` validates the graph.

Use `source = "$input"` or the name of an earlier step. Map each decision outcome to the next registered step or `"$done"`. Choice outcomes use criterion keys, yes/no uses a 0.5 threshold, and score uses `min_score` with `at_or_above` and `below`. An optional schedule starts a workflow on UTC epoch multiples of `every_secs`.

For a shared service, follow the [Fedora deployment and MCP guide](docs/deploy.md). See [execution and evidence policy](docs/policy.md) before deploying a script or enabling external judgments.
