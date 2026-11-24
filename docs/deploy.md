# Fedora deployment

Use a Fedora account with rootless Podman, user systemd and lingering enabled. Port 8789 must be available on loopback. Establish the target and service credential storage with your operator.

## Start the service

From a verified release checkout, build and record the local image ID.

```sh
podman build --tag localhost/decision-tools:0.1.0 .
image_id="$(podman image inspect --format '{{.Id}}' localhost/decision-tools:0.1.0)"
install -d -m 700 ~/.config/containers/systemd
sed "s|@IMAGE_ID@|${image_id}|" deploy/decision-tools.container \
  > ~/.config/containers/systemd/decision-tools.container
```

Create the `decision-tools-token` Podman secret from a strong operator-generated bearer token of at least 32 visible ASCII characters. Pass the value on stdin, such as `podman secret create decision-tools-token - < /path/to/protected-token`. Give authorized clients the token through your secure channel.

Create `decision-tools-openrouter` in the same way from the approved provider key. Add these entries under `[Container]` in the installed unit.

```ini
Secret=decision-tools-openrouter,target=decision-tools-openrouter,uid=10001,gid=10001,mode=0400
Environment=OPENROUTER_API_KEY_FILE=/run/secrets/decision-tools-openrouter
```

For a demo deployment use `Environment=DECISION_TOOLS_DEMO=true` in place of those two entries. For a custom registry and scripts, mount an operator-owned directory read-only with `:ro,Z` and set `DECISION_TOOLS_CONFIG` to the mounted registry path.

```sh
systemctl --user daemon-reload
systemctl --user start decision-tools.service
curl -fsS http://127.0.0.1:8789/healthz
curl -fsS http://127.0.0.1:8789/readyz
```

Readiness reports the configured backend and checks SQLite without issuing a decision. Verify a real provider installation with one authorized sample. Check service logs with `journalctl --user -u decision-tools.service`.

## Connect an MCP client

Open a port forward over an authorized SSH connection. Replace `<server-alias>` with its configured SSH host.

```sh
ssh -NT -o ExitOnForwardFailure=yes -L 127.0.0.1:8789:127.0.0.1:8789 <server-alias>
```

Point the client's Streamable HTTP MCP entry at `http://127.0.0.1:8789/mcp`. Set `Authorization` to `Bearer <token>` from the protected token source. For Codex, configure the URL and `bearer_token_env_var = "DECISION_TOOLS_TOKEN"`, then start Codex from the environment holding that variable. Run `workflow_list` and a sample workflow. On a demo deployment the repo's HTTP smoke check verifies both examples end to end.

```sh
python3 tests/smoke.py --url http://127.0.0.1:8789/mcp
```

For example, register Codex from a shell with the authorized tunnel active.

```sh
codex mcp add decision-tools --url http://127.0.0.1:8789/mcp \
  --bearer-token-env-var DECISION_TOOLS_TOKEN
```

## Backup and upgrade

Stop new work and wait for active runs to reach a terminal state. Stop the user service before exporting the `decision-tools-state` volume with `podman volume export`. Store the archive with owner-only permissions. Validate a backup by importing into a new named volume with `podman volume import` and checking it through a service instance on a separate loopback port.

After a new release passes checks, replace the installed image ID and restart the service. Keep the prior image ID and the stopped-volume backup for a schema-changing upgrade. Use the earlier image with a compatible database snapshot for rollback. Preserve newer run data before rolling back to a snapshot.
