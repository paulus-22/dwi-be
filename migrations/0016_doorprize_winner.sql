-- Persisted doorprize winners drawn from checked-in (scanned) guests.
-- Idempotent so manual pre-deploy psql runs never break sqlx playback.
CREATE TABLE IF NOT EXISTS doorprize_winner (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id       UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    check_in_id    UUID,
    guest_code     TEXT NOT NULL,
    winner_name    TEXT NOT NULL,
    household_name TEXT,
    prize          TEXT,
    drawn_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    drawn_by       UUID REFERENCES "admin_user"("id") ON DELETE SET NULL
);

CREATE INDEX IF NOT EXISTS doorprize_winner_event_idx
    ON doorprize_winner (event_id, drawn_at DESC);
