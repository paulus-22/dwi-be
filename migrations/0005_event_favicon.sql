-- Per-event custom favicon: lets each event's public page carry its own
-- browser-tab icon (a small "custom vibe" cue distinct from the site-wide
-- default) instead of always falling back to the shared favicon.
-- Additive, nullable: existing events are unaffected and fall back to the
-- app's own favicon.ico as before.
ALTER TABLE "event" ADD COLUMN "faviconUrl" TEXT;
