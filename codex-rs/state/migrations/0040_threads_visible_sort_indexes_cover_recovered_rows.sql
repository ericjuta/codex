DROP INDEX IF EXISTS idx_threads_visible_created_at_ms;
DROP INDEX IF EXISTS idx_threads_visible_updated_at_ms;
DROP INDEX IF EXISTS idx_threads_visible_recency_at_ms;
DROP INDEX IF EXISTS idx_threads_archived;

CREATE INDEX idx_threads_visible_created_at_ms
    ON threads(archived, created_at_ms DESC);

CREATE INDEX idx_threads_visible_updated_at_ms
    ON threads(archived, updated_at_ms DESC);

CREATE INDEX idx_threads_visible_recency_at_ms
    ON threads(archived, recency_at_ms DESC, id DESC);
