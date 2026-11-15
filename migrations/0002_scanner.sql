-- Scanner: paired devices, the append-only log of what happens at the door,
-- and uploaded photos.
--
-- New tables use snake_case. The Prisma-era tables keep their camelCase
-- columns until the old app is retired.

-- Codes an organiser reads out (or shows) to pair door devices. One code can
-- pair every tablet for the day; it stops working when it expires or is revoked.
CREATE TABLE scanner_pairing_code (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id      UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    code          TEXT NOT NULL UNIQUE,
    label         TEXT,
    created_by    UUID REFERENCES "admin_user"("id") ON DELETE SET NULL,
    expires_at    TIMESTAMPTZ NOT NULL,
    uses          INTEGER NOT NULL DEFAULT 0,
    last_used_at  TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX scanner_pairing_code_event_idx ON scanner_pairing_code (event_id);

-- A paired phone/tablet. Its bearer token is stored only as a SHA-256 hash.
CREATE TABLE scanner_device (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id         UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    pairing_code_id  UUID REFERENCES scanner_pairing_code(id) ON DELETE SET NULL,
    name             TEXT NOT NULL,
    token_hash       BYTEA NOT NULL UNIQUE,
    paired_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at     TIMESTAMPTZ,
    -- server clock minus device clock at the last sync, in milliseconds.
    clock_offset_ms  BIGINT NOT NULL DEFAULT 0,
    revoked_at       TIMESTAMPTZ
);
CREATE INDEX scanner_device_event_idx ON scanner_device (event_id);

-- Everything that happens at the door, from every device. Rows are never
-- updated. `id` is generated on the device, so a retried upload is a no-op;
-- `seq` is the server-side order devices pull by.
--
-- Two devices may check in the same guest while offline. Both rows are kept;
-- "first check-in per guest" is decided when reading (earliest occurred_at),
-- because a late-syncing device can bring an earlier scan.
CREATE TABLE scanner_event (
    id            UUID PRIMARY KEY,
    seq           BIGSERIAL NOT NULL UNIQUE,
    event_id      UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    device_id     UUID NOT NULL REFERENCES scanner_device(id) ON DELETE CASCADE,
    kind          TEXT NOT NULL CHECK (kind IN ('CHECK_IN', 'PHOTO', 'GUESTBOOK', 'SOUVENIR')),
    guest_code    TEXT NOT NULL,
    guest_id      UUID REFERENCES "guest"("id") ON DELETE SET NULL,
    -- For activities: the check-in they belong to (a scanner_event id).
    check_in_id   UUID,
    headcount     INTEGER,
    override      BOOLEAN NOT NULL DEFAULT false,
    data          JSONB NOT NULL DEFAULT '{}'::jsonb,
    occurred_at   TIMESTAMPTZ NOT NULL,
    received_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX scanner_event_event_seq_idx ON scanner_event (event_id, seq);
CREATE INDEX scanner_event_guest_idx ON scanner_event (event_id, kind, guest_code, occurred_at);

-- Photos uploaded straight to object storage. The row is written once the
-- device reports the upload finished.
CREATE TABLE scanner_photo (
    id            UUID PRIMARY KEY,
    seq           BIGSERIAL NOT NULL UNIQUE,
    event_id      UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    device_id     UUID NOT NULL REFERENCES scanner_device(id) ON DELETE CASCADE,
    check_in_id   UUID,
    guest_code    TEXT NOT NULL,
    guest_id      UUID REFERENCES "guest"("id") ON DELETE SET NULL,
    object_key    TEXT NOT NULL,
    content_type  TEXT NOT NULL,
    occurred_at   TIMESTAMPTZ NOT NULL,
    received_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    hidden_at     TIMESTAMPTZ
);
CREATE INDEX scanner_photo_event_seq_idx ON scanner_photo (event_id, seq);
