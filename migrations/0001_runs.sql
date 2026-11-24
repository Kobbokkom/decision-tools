CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    workflow TEXT NOT NULL,
    revision TEXT NOT NULL,
    input TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN (
        'queued', 'running', 'cancelling', 'succeeded', 'failed', 'cancelled', 'interrupted'
    )),
    next_step TEXT,
    results TEXT NOT NULL DEFAULT '{}',
    error TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch()),
    schedule TEXT,
    scheduled_at INTEGER,
    CHECK ((schedule IS NULL) = (scheduled_at IS NULL)),
    UNIQUE (schedule, scheduled_at)
);

CREATE INDEX runs_queue ON runs (created_at, id) WHERE status = 'queued';
