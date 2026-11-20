-- Per-guest direct-QR exemption: mark a single guest so their QR ticket
-- shows without them having to fill the confirmation (RSVP) form first.
--   false (default) — existing behaviour: confirmation required to reach
--                      the issued (real QR) state.
--   true             — this guest is treated as `issued` immediately: the
--                      pass shows a real scannable QR and the form is
--                      bypassed. `admits` defaults to plusGuestAllowance+1.
-- Additive; existing guests are unaffected (default false).
ALTER TABLE "guest" ADD COLUMN IF NOT EXISTS "qrDirect" BOOLEAN NOT NULL DEFAULT false;
