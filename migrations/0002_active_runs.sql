CREATE INDEX runs_active ON runs (status) WHERE status IN ('queued', 'running', 'cancelling');
