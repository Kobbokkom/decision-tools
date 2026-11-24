# Policy

## Evidence and execution authority

An operator controls installed executable paths, input contracts, schedule definitions, and branch targets. A workflow's scripts run with the service account's filesystem and network authority. Install reviewed scripts and keep deployed config and executable files immutable while runs are active.

An external judgment requires approval to send its evidence to the selected provider. Enable that route in a workflow only for inputs and script output cleared for use with that provider. Treat a judgment as evidence, and apply the workflow's separate authorization rules before any consequential action.

## State

A deployment serves a single trusted operator group. Authenticated clients share access to the registered workflows and can read results by run ID. Protect the SQLite volume and backups as production data.
