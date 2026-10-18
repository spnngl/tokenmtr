-- Rewrites and lookups by working directory (project re-resolution, unresolved cwds).
CREATE INDEX usage_event_cwd ON usage_event (cwd);
