# Policy

## Evidence and execution authority

An operator controls installed executable paths, input contracts, schedule definitions, and branch targets. A workflow's scripts run with the service account's filesystem and network authority. Install reviewed scripts and keep deployed config and executable files immutable while runs are active.

An external judgment requires approval to send its evidence to the selected provider. Enable that route in a workflow only for inputs and script output cleared for use with that provider. Treat a judgment as evidence, and apply the workflow's separate authorization rules before any consequential action.

## State

A deployment serves a single trusted operator group. Authenticated clients share access to the registered workflows and can read results by run ID. Protect the SQLite volume and backups as production data.

## Local tools

`grep` sends excerpts of files under the requested root, and `browse` sends the task, URLs, page text and controls, to the selected provider. Call them only for source and pages cleared for that provider. `browse` runs a throwaway Chrome profile with no saved sign-ins, and it never types unless the call allows it. Treat both results as leads and check them against the source or the page.
