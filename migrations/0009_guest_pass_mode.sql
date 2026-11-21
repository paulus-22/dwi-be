-- Per-guest pass + gate modes.
--
-- passMode  (varchar, default 'NORMAL'):
--   NORMAL — default: confirmation (RSVP) required to reach the issued QR pass.
--   DIRECT — pass issues immediately, no confirmation needed (the old qrDirect).
--   GROUP  — 'no individual entry pass / open group' (e.g. a class/angkatan
--            whose whole group comes off one link). The QR pass is never shown.
--
-- passcode (text, nullable): a custom password for THIS guest, used by the
-- invitation gate. When set it overrides the default derivation (last 6
-- digits of phone). Empty/null → fall back to phone-last-6; a guest with no
-- phone and no passcode cannot be locked out (gate passes).
--
-- Additive + idempotent. Existing qrDirect=true rows become passMode='DIRECT'.
ALTER TABLE "guest" ADD COLUMN IF NOT EXISTS "passMode" TEXT NOT NULL DEFAULT 'NORMAL';
ALTER TABLE "guest" ADD COLUMN IF NOT EXISTS "passcode" TEXT;
UPDATE "guest" SET "passMode" = 'DIRECT' WHERE "qrDirect" = true AND "passMode" = 'NORMAL';
