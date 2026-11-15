-- Generic events: same engine, second product line alongside The Moments
-- (wedding). One login, one event list, split by `kind`. Fully additive —
-- existing wedding rows are untouched and keep behaving exactly as before.

-- ===== Event kind
-- Every existing row defaults to WEDDING, so no backfill needed.
CREATE TYPE "EventKind" AS ENUM ('WEDDING', 'GENERIC');
ALTER TABLE "event" ADD COLUMN "kind" "EventKind" NOT NULL DEFAULT 'WEDDING';
CREATE INDEX "event_kind_idx" ON "event" ("kind");

-- The wedding-only identity fields (bride/groom names, parents, instagram)
-- were NOT NULL because every row used to be a wedding. A GENERIC event has
-- no bride/groom, so loosen them; the app layer enforces "required when
-- kind = WEDDING" instead of the database.
ALTER TABLE "event" ALTER COLUMN "brideShortName" DROP NOT NULL;
ALTER TABLE "event" ALTER COLUMN "brideFullName" DROP NOT NULL;
ALTER TABLE "event" ALTER COLUMN "groomShortName" DROP NOT NULL;
ALTER TABLE "event" ALTER COLUMN "groomFullName" DROP NOT NULL;

-- Generic events need their own display name/description instead of
-- "bride & groom". headlineDate/timezone/theme/bilingual/codePrefix etc.
-- stay shared — they're already kind-agnostic (event date, invite codes...).
ALTER TABLE "event" ADD COLUMN "title" TEXT;
ALTER TABLE "event" ADD COLUMN "subtitle" TEXT;
ALTER TABLE "event" ADD COLUMN "description" TEXT;

CREATE OR REPLACE FUNCTION check_event_kind_fields() RETURNS trigger AS $$
BEGIN
  IF NEW."kind" = 'WEDDING' THEN
    IF NEW."brideShortName" IS NULL OR NEW."groomShortName" IS NULL THEN
      RAISE EXCEPTION 'WEDDING events require brideShortName and groomShortName';
    END IF;
  ELSIF NEW."kind" = 'GENERIC' THEN
    IF NEW."title" IS NULL THEN
      RAISE EXCEPTION 'GENERIC events require a title';
    END IF;
  END IF;
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER event_kind_fields_check
  BEFORE INSERT OR UPDATE ON "event"
  FOR EACH ROW EXECUTE FUNCTION check_event_kind_fields();

-- ===== Dynamic registration form (per event, GENERIC mostly, but usable by
-- WEDDING too if an organizer wants an extra question).
CREATE TABLE registration_form_field (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id      UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    field_key     TEXT NOT NULL,              -- stable machine key, e.g. "dietary"
    label_id      TEXT NOT NULL,
    label_en      TEXT,
    field_type    TEXT NOT NULL CHECK (field_type IN
                    ('TEXT', 'TEXTAREA', 'NUMBER', 'SELECT', 'RADIO', 'CHECKBOX', 'DATE')),
    options       JSONB NOT NULL DEFAULT '[]'::jsonb, -- [{value,labelId,labelEn}] for SELECT/RADIO/CHECKBOX
    required      BOOLEAN NOT NULL DEFAULT false,
    sort_order    INTEGER NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (event_id, field_key)
);
CREATE INDEX registration_form_field_event_idx ON registration_form_field (event_id, sort_order);

-- One guest's answer to one dynamic field. Guest table itself (name, phone,
-- category, token, plusGuestAllowance) is already generic enough — reused
-- as-is for GENERIC events ("householdName" just reads as "participant name").
CREATE TABLE guest_response (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    guest_id      UUID NOT NULL REFERENCES "guest"("id") ON DELETE CASCADE,
    field_id      UUID NOT NULL REFERENCES registration_form_field(id) ON DELETE CASCADE,
    value         JSONB NOT NULL,             -- scalar or array, matches field_type
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (guest_id, field_id)
);
CREATE INDEX guest_response_guest_idx ON guest_response (guest_id);

-- ===== Dynamic page/section builder for the public invite/registration
-- page. WEDDING events keep rendering their existing fixed sections
-- (ceremony/story_milestone/gift_account/rundown_item) unchanged; this table
-- only drives GENERIC event pages (and, later, a fully custom wedding page
-- if ever wanted).
CREATE TABLE event_page_block (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id      UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    block_type    TEXT NOT NULL CHECK (block_type IN
                    ('COVER', 'COUNTDOWN', 'SCHEDULE', 'MAP', 'GALLERY',
                     'RSVP_FORM', 'REGISTRATION_FORM', 'CUSTOM_HTML', 'SPONSOR', 'CONTACT')),
    config        JSONB NOT NULL DEFAULT '{}'::jsonb, -- block-specific content/props
    sort_order    INTEGER NOT NULL DEFAULT 0,
    is_visible    BOOLEAN NOT NULL DEFAULT true,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX event_page_block_event_idx ON event_page_block (event_id, sort_order);

-- ===== WhatsApp message log (outbound invite + automatic RSVP confirmation).
-- Provider-agnostic: `provider`/`provider_message_id` let us swap Fonnte/
-- Wablas/WA Cloud API later without a schema change.
CREATE TABLE whatsapp_message (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id            UUID NOT NULL REFERENCES "event"("id") ON DELETE CASCADE,
    guest_id            UUID REFERENCES "guest"("id") ON DELETE SET NULL,
    kind                TEXT NOT NULL CHECK (kind IN ('INVITE', 'RSVP_CONFIRMATION', 'REMINDER')),
    phone               TEXT NOT NULL,
    template_key        TEXT NOT NULL,
    payload             JSONB NOT NULL DEFAULT '{}'::jsonb,
    provider             TEXT,
    provider_message_id TEXT,
    status              TEXT NOT NULL DEFAULT 'PENDING' CHECK (status IN
                          ('PENDING', 'SENT', 'DELIVERED', 'FAILED')),
    error               TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    sent_at             TIMESTAMPTZ
);
CREATE INDEX whatsapp_message_event_idx ON whatsapp_message (event_id, kind, status);
CREATE INDEX whatsapp_message_guest_idx ON whatsapp_message (guest_id);
