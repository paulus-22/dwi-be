-- Per-event invitation gate: a password the guest must enter before the
-- invitation page renders (full-page gate).
--   'OPEN'     (default) — no password; current behaviour.
--   'PHONE_6'  — invitation requires the last 6 digits of the guest's phone
--                number as the password. Guests with no phone stored can't
--                be gated (no key to derive), so the invite stays open for
--                them (same as OPEN) — gating only applies when a phone
--                exists.
-- Additive; existing events fall back to 'OPEN'.
ALTER TABLE "event" ADD COLUMN "inviteGate" TEXT NOT NULL DEFAULT 'OPEN';
