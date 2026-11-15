-- Ticket types: quota'd, pre-generated QR pools for open/allowlist self-
-- registration on GENERIC events. Fully additive; WEDDING's closed,
-- admin-pre-seeded guest model is untouched.
--
-- Design: a ticket type does NOT reuse the legacy `CategoryCode` enum baked
-- into the signed QR payload format (DWI1.<prefix>.<category>.<seq>.<admits>)
-- — that's a closed 5-value set (FAM/FRN/WRK/VIP/GEN) wired into the scanner's
-- category colors and the HMAC signing code in dwi-domain, too risky to widen
-- for an open-ended admin-named list. Every ticket-type guest is signed under
-- category GEN; `guest.ticket_type_id` is the real source of truth for which
-- type it belongs to and how quota is tracked.
--
-- Quota is pre-generated, not checked at registration time: creating a
-- ticket type with quota N immediately inserts N placeholder `guest` rows
-- (unclaimed: ticket_claimed_at IS NULL, householdName is a placeholder).
-- Registering atomically claims one row (`FOR UPDATE SKIP LOCKED`), so the
-- QR a registrant receives was already fully formed and signable before
-- they ever registered, matching "QR needs to be pregenerated".

CREATE TABLE "ticket_type" (
    "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    "event_id" uuid NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    "name" text NOT NULL,
    "description" text,
    -- NULL = unlimited (no pre-generated pool; registrations create guests
    -- on the fly instead of claiming a pre-made one).
    "quota" integer CHECK ("quota" IS NULL OR "quota" >= 0),
    "sort_order" integer NOT NULL DEFAULT 0,
    "created_at" timestamp(3) NOT NULL DEFAULT now(),
    "updated_at" timestamp(3) NOT NULL DEFAULT now()
);
CREATE INDEX "ticket_type_event_id_idx" ON "ticket_type" ("event_id");

ALTER TABLE "guest" ADD COLUMN "ticket_type_id" uuid REFERENCES "ticket_type"("id") ON DELETE SET NULL;
CREATE INDEX "guest_ticket_type_id_idx" ON "guest" ("ticket_type_id");

-- A pre-generated row is "claimed" the moment a real registrant takes it.
-- Unclaimed rows are the pool; `SELECT ... WHERE ticket_type_id = $1 AND
-- ticket_claimed_at IS NULL ... FOR UPDATE SKIP LOCKED LIMIT 1` is the quota
-- check — no separate counter to keep in sync.
ALTER TABLE "guest" ADD COLUMN "ticket_claimed_at" timestamp(3);

-- Open vs allowlist-gated self-registration, per event.
CREATE TYPE "RegistrationMode" AS ENUM ('OPEN', 'ALLOWLIST');
ALTER TABLE "event" ADD COLUMN "registrationMode" "RegistrationMode" NOT NULL DEFAULT 'OPEN';

-- Allowlist entries: a phone or email pre-approved to self-register when
-- registrationMode = ALLOWLIST. Checked, not consumed by itself — the
-- actual guest row (and its one-time claim) is what prevents a double
-- registration; this table only gates *entry*, matching "based on
-- email/phone list" without inventing a one-use-token system yet.
CREATE TABLE "registration_allowlist_entry" (
    "id" uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    "event_id" uuid NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    "phone" text,
    "email" text,
    "used_at" timestamp(3),
    "created_at" timestamp(3) NOT NULL DEFAULT now(),
    CHECK ("phone" IS NOT NULL OR "email" IS NOT NULL)
);
CREATE INDEX "registration_allowlist_entry_event_id_idx" ON "registration_allowlist_entry" ("event_id");
CREATE INDEX "registration_allowlist_entry_phone_idx" ON "registration_allowlist_entry" ("event_id", "phone");
CREATE INDEX "registration_allowlist_entry_email_idx" ON "registration_allowlist_entry" ("event_id", "email");
