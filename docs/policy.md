# Policy

## Evidence and execution authority

An operator controls installed executable paths, input contracts, schedule definitions, and branch targets. A workflow's scripts run with the service account's filesystem and network authority. Install reviewed scripts and keep deployed config and executable files immutable while runs are active.

An external judgment requires approval to send its evidence to the selected provider. Manual starts require the caller to approve OpenRouter evidence for that run. Operators are responsible for approving the configured inputs and script output of scheduled workflows. Treat a judgment as evidence, and apply the workflow's separate authorization rules before any consequential action.

## State

A deployment serves a single trusted operator group. Authenticated clients share access to the registered workflows and can read results by run ID. Protect the SQLite volume and backups as production data.

## Local tools

`grep` sends excerpts of files under the requested root, and `browse` sends the task, URLs, page text and controls, to the selected provider. Call them only for source and pages cleared for that provider. `browse` runs a throwaway Chrome profile with no saved sign-ins and never types unless the call allows it. Clicks and dropdown choices always run and can submit or change things, so state only safe actions in the goal. A public page cannot lead it to a loopback or private-network address. Treat both results as leads and check them against the source or the page.
