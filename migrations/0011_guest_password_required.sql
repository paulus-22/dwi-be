-- Per-guest password-gate exemption.
--
-- An event may be PHONE_6 (invitation protected by a password) while a
-- subset of guests are exempt (no password needed to open their invite).
-- GROUP guests are exempt regardless of this flag.
--
-- Additive; existing guests keep the default "password required" behaviour.
ALTER TABLE "guest" ADD COLUMN IF NOT EXISTS "passwordRequired" BOOLEAN NOT NULL DEFAULT TRUE;
