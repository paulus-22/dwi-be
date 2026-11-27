-- 0015_event_greeting_image.sql
-- Per-event "greeting" background for the scanner TV/greeter: when a guest
-- checks in, the greeter shows this chosen image behind the guest's name (+
-- selfie). Stored in R2 under events/<id>/greeting/*.webp; the key lives here.
ALTER TABLE "event"
    ADD COLUMN IF NOT EXISTS "greetingImageKey" TEXT;
