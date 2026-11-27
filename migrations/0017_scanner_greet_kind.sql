-- Allow explicit GREET records in scanner_event so the scanner signals TV
-- greeting readiness strictly after the photo step is confirmed (or skipped).
ALTER TABLE scanner_event DROP CONSTRAINT IF EXISTS scanner_event_kind_check;
ALTER TABLE scanner_event ADD CONSTRAINT scanner_event_kind_check
    CHECK (kind IN ('CHECK_IN', 'PHOTO', 'GUESTBOOK', 'SOUVENIR', 'GREET'));
