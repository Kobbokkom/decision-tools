# Architecture

Decision Tools is one Rust process on one host. Its public contract is MCP. The process can speak MCP over stdio or authenticate clients over Streamable HTTP. The HTTP listener also exposes liveness and database readiness.

`src/features` owns decision-provider calls, registration validation, and subprocess execution. `src/orchestration` interprets the registered graph and owns workflow state. `src/mcp.rs` adapts tools to those services. `src/main.rs` owns startup, credentials, the database lease, and transports.

SQLite is the durable store for the single-host deployment. Ordered SQLx migrations own its schema. Each run row is both the execution record and the durable intent for the next step. Claim the step before external work. Commit its result and successor together. Recover queued work against the same registry and record unresolved in-flight work as interrupted. Schedule slots share the same transaction and use a unique database key.

Each configured step either runs a trusted registered program or asks a typed question about selected evidence. The registry defines all transitions. The direct `decide` call returns the provider's validated judgment and usage without creating a workflow run.
