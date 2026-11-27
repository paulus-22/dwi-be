-- 0014_scanner_slideshow.sql
-- Images uploaded from the dashboard and shown, cycling, on every scanner
-- device in slideshow mode. Stored in R2 like selfies but under a separate
-- per-event prefix so the two kinds never mix. Dashboard uploads are WebP.
CREATE TABLE IF NOT EXISTS scanner_slideshow (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id     UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    seq          BIGSERIAL NOT NULL UNIQUE,
    object_key   TEXT NOT NULL UNIQUE,
    content_type TEXT NOT NULL,
    created_by   UUID REFERENCES "admin_user"("id") ON DELETE SET NULL,
    hidden_at    TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS scanner_slideshow_event_idx ON scanner_slideshow (event_id, seq);
