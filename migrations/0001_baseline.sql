-- Baseline: the schema as built by the original app's Prisma migrations,
-- concatenated verbatim and in order (20260925101603_init … 20261028090000_event_gift_address).
-- On a database that already has these tables (production), this migration is
-- recorded as applied without running — see `dwi-api migrate` / src/db.rs.
-- Column names stay camelCase and quoted, exactly as Prisma created them.

-- ===== 20260925101603_init
-- CreateEnum
CREATE TYPE "Locale" AS ENUM ('ID', 'EN');

-- CreateEnum
CREATE TYPE "EventStatus" AS ENUM ('DRAFT', 'PUBLISHED', 'ARCHIVED');

-- CreateEnum
CREATE TYPE "AdminRole" AS ENUM ('OWNER', 'ORGANIZER');

-- CreateEnum
CREATE TYPE "GiftAccountKind" AS ENUM ('BANK', 'EWALLET');

-- CreateTable
CREATE TABLE "admin_user" (
    "id" UUID NOT NULL,
    "email" TEXT NOT NULL,
    "name" TEXT NOT NULL,
    "passwordHash" TEXT NOT NULL,
    "isSuperAdmin" BOOLEAN NOT NULL DEFAULT false,
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "admin_user_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "event_membership" (
    "id" UUID NOT NULL,
    "adminId" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "role" "AdminRole" NOT NULL DEFAULT 'ORGANIZER',
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "event_membership_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "event" (
    "id" UUID NOT NULL,
    "slug" TEXT NOT NULL,
    "status" "EventStatus" NOT NULL DEFAULT 'DRAFT',
    "brideShortName" TEXT NOT NULL,
    "brideFullName" TEXT NOT NULL,
    "brideParents" TEXT,
    "brideInstagram" TEXT,
    "groomShortName" TEXT NOT NULL,
    "groomFullName" TEXT NOT NULL,
    "groomParents" TEXT,
    "groomInstagram" TEXT,
    "headlineDate" TIMESTAMP(3) NOT NULL,
    "timezone" TEXT NOT NULL DEFAULT 'Asia/Jakarta',
    "defaultLocale" "Locale" NOT NULL DEFAULT 'ID',
    "themeTokens" JSONB,
    "storyIntroId" TEXT,
    "storyIntroEn" TEXT,
    "closingNoteId" TEXT,
    "closingNoteEn" TEXT,
    "dressCodeNoteId" TEXT,
    "dressCodeNoteEn" TEXT,
    "dressCodeColors" TEXT[] DEFAULT ARRAY[]::TEXT[],
    "musicUrl" TEXT,
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "event_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "ceremony" (
    "id" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "nameId" TEXT NOT NULL,
    "nameEn" TEXT NOT NULL,
    "startsAt" TIMESTAMP(3) NOT NULL,
    "endsAt" TIMESTAMP(3),
    "venueName" TEXT NOT NULL,
    "venueAddress" TEXT,
    "mapsUrl" TEXT,
    "mapsEmbedUrl" TEXT,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,

    CONSTRAINT "ceremony_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "story_milestone" (
    "id" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "titleId" TEXT NOT NULL,
    "titleEn" TEXT NOT NULL,
    "bodyId" TEXT NOT NULL,
    "bodyEn" TEXT NOT NULL,
    "periodLabel" TEXT,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,

    CONSTRAINT "story_milestone_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "gift_account" (
    "id" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "kind" "GiftAccountKind" NOT NULL DEFAULT 'BANK',
    "institution" TEXT NOT NULL,
    "accountNumber" TEXT NOT NULL,
    "accountHolder" TEXT NOT NULL,
    "qrImageUrl" TEXT,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,

    CONSTRAINT "gift_account_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "guest" (
    "id" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "householdName" TEXT NOT NULL,
    "token" TEXT NOT NULL,
    "plusGuestAllowance" INTEGER NOT NULL DEFAULT 0,
    "category" TEXT,
    "phone" TEXT,
    "notes" TEXT,
    "invitedAt" TIMESTAMP(3),
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "guest_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "rsvp" (
    "id" UUID NOT NULL,
    "guestId" UUID NOT NULL,
    "attending" BOOLEAN NOT NULL,
    "guestCount" INTEGER NOT NULL DEFAULT 1,
    "message" TEXT,
    "submittedAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "updatedAt" TIMESTAMP(3) NOT NULL,

    CONSTRAINT "rsvp_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "wish" (
    "id" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "guestId" UUID,
    "name" TEXT NOT NULL,
    "message" TEXT NOT NULL,
    "isHidden" BOOLEAN NOT NULL DEFAULT false,
    "createdAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "wish_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "check_in" (
    "id" UUID NOT NULL,
    "guestId" UUID NOT NULL,
    "scannedAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
    "scannedBy" TEXT,
    "guestCount" INTEGER NOT NULL DEFAULT 1,

    CONSTRAINT "check_in_pkey" PRIMARY KEY ("id")
);

-- CreateTable
CREATE TABLE "photo" (
    "id" UUID NOT NULL,
    "checkInId" UUID NOT NULL,
    "storageUrl" TEXT NOT NULL,
    "capturedAt" TIMESTAMP(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,

    CONSTRAINT "photo_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE UNIQUE INDEX "admin_user_email_key" ON "admin_user"("email");

-- CreateIndex
CREATE INDEX "event_membership_eventId_idx" ON "event_membership"("eventId");

-- CreateIndex
CREATE UNIQUE INDEX "event_membership_adminId_eventId_key" ON "event_membership"("adminId", "eventId");

-- CreateIndex
CREATE UNIQUE INDEX "event_slug_key" ON "event"("slug");

-- CreateIndex
CREATE INDEX "event_status_idx" ON "event"("status");

-- CreateIndex
CREATE INDEX "ceremony_eventId_sortOrder_idx" ON "ceremony"("eventId", "sortOrder");

-- CreateIndex
CREATE INDEX "story_milestone_eventId_sortOrder_idx" ON "story_milestone"("eventId", "sortOrder");

-- CreateIndex
CREATE INDEX "gift_account_eventId_sortOrder_idx" ON "gift_account"("eventId", "sortOrder");

-- CreateIndex
CREATE UNIQUE INDEX "guest_token_key" ON "guest"("token");

-- CreateIndex
CREATE INDEX "guest_eventId_idx" ON "guest"("eventId");

-- CreateIndex
CREATE INDEX "guest_eventId_category_idx" ON "guest"("eventId", "category");

-- CreateIndex
CREATE UNIQUE INDEX "rsvp_guestId_key" ON "rsvp"("guestId");

-- CreateIndex
CREATE INDEX "wish_eventId_isHidden_createdAt_idx" ON "wish"("eventId", "isHidden", "createdAt");

-- CreateIndex
CREATE INDEX "check_in_guestId_idx" ON "check_in"("guestId");

-- CreateIndex
CREATE INDEX "photo_checkInId_idx" ON "photo"("checkInId");

-- AddForeignKey
ALTER TABLE "event_membership" ADD CONSTRAINT "event_membership_adminId_fkey" FOREIGN KEY ("adminId") REFERENCES "admin_user"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "event_membership" ADD CONSTRAINT "event_membership_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "ceremony" ADD CONSTRAINT "ceremony_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "story_milestone" ADD CONSTRAINT "story_milestone_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "gift_account" ADD CONSTRAINT "gift_account_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "guest" ADD CONSTRAINT "guest_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "rsvp" ADD CONSTRAINT "rsvp_guestId_fkey" FOREIGN KEY ("guestId") REFERENCES "guest"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "wish" ADD CONSTRAINT "wish_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "wish" ADD CONSTRAINT "wish_guestId_fkey" FOREIGN KEY ("guestId") REFERENCES "guest"("id") ON DELETE SET NULL ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "check_in" ADD CONSTRAINT "check_in_guestId_fkey" FOREIGN KEY ("guestId") REFERENCES "guest"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- AddForeignKey
ALTER TABLE "photo" ADD CONSTRAINT "photo_checkInId_fkey" FOREIGN KEY ("checkInId") REFERENCES "check_in"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- ===== 20260925111110_add_quote_hashtag_rundown
-- AlterTable
ALTER TABLE "event" ADD COLUMN     "hashtag" TEXT,
ADD COLUMN     "quoteAuthor" TEXT,
ADD COLUMN     "quoteTextEn" TEXT,
ADD COLUMN     "quoteTextId" TEXT;

-- CreateTable
CREATE TABLE "rundown_item" (
    "id" UUID NOT NULL,
    "eventId" UUID NOT NULL,
    "timeLabel" TEXT NOT NULL,
    "titleId" TEXT NOT NULL,
    "titleEn" TEXT NOT NULL,
    "sortOrder" INTEGER NOT NULL DEFAULT 0,

    CONSTRAINT "rundown_item_pkey" PRIMARY KEY ("id")
);

-- CreateIndex
CREATE INDEX "rundown_item_eventId_sortOrder_idx" ON "rundown_item"("eventId", "sortOrder");

-- AddForeignKey
ALTER TABLE "rundown_item" ADD CONSTRAINT "rundown_item_eventId_fkey" FOREIGN KEY ("eventId") REFERENCES "event"("id") ON DELETE CASCADE ON UPDATE CASCADE;

-- ===== 20260925221331_rsvp_respondent_name
-- AlterTable
ALTER TABLE "rsvp" ADD COLUMN     "respondentName" TEXT;

-- ===== 20260926120000_invite_code_prefix_and_sequence
-- Structured invite codes: an event-level prefix plus a per-event sequence.

-- AlterTable
ALTER TABLE "event" ADD COLUMN     "codePrefix" TEXT NOT NULL DEFAULT 'INV';

-- AlterTable
ALTER TABLE "guest" ADD COLUMN     "sequence" INTEGER NOT NULL DEFAULT 0;

-- Backfill before the unique index exists. Every existing guest defaults to
-- sequence 0, so creating the index first would fail on any event with more
-- than one guest. Number them by name so the order is stable and repeatable.
WITH numbered AS (
  SELECT id, ROW_NUMBER() OVER (
    PARTITION BY "eventId" ORDER BY "householdName", "createdAt", id
  ) AS seq
  FROM "guest"
)
UPDATE "guest" AS g
SET "sequence" = numbered.seq
FROM numbered
WHERE g.id = numbered.id;

-- CreateIndex
CREATE UNIQUE INDEX "guest_eventId_sequence_key" ON "guest"("eventId", "sequence");

-- ===== 20260926140000_code_lock_and_rbac
-- Code immutability and role-based access.

-- AlterEnum: a read-only role for people who need to see the list but must not
-- be able to change it.
ALTER TYPE "AdminRole" ADD VALUE 'VIEWER';

-- AlterTable: disabled accounts keep their audit trail but cannot sign in.
ALTER TABLE "admin_user" ADD COLUMN     "isActive" BOOLEAN NOT NULL DEFAULT true;

-- AlterTable: set once a code has been printed or sent, after which reissuing
-- must skip it — a card already in someone's hand cannot be edited.
ALTER TABLE "guest" ADD COLUMN     "codeLockedAt" TIMESTAMP(3);

-- ===== 20260926160000_event_theme
-- Named invitation palette. Existing events keep the original look.
ALTER TABLE "event" ADD COLUMN     "theme" TEXT NOT NULL DEFAULT 'sage-peach';

-- ===== 20260926180000_event_bilingual
-- Per-invitation language choice. Existing events keep both languages.
ALTER TABLE "event" ADD COLUMN     "bilingual" BOOLEAN NOT NULL DEFAULT true;

-- ===== 20261016120000_event_photos
-- Optional imagery. Every section that gains a photo hides it when null, so
-- existing invitations are unaffected.
ALTER TABLE "event" ADD COLUMN     "coverPhotoUrl" TEXT;
ALTER TABLE "event" ADD COLUMN     "closingPhotoUrl" TEXT;
ALTER TABLE "event" ADD COLUMN     "dressCodeImageUrl" TEXT;
ALTER TABLE "story_milestone" ADD COLUMN     "photoUrl" TEXT;

-- ===== 20261017090000_event_interludes
-- Full-bleed photos between sections. Defaults to empty, so existing
-- invitations render exactly as before.
ALTER TABLE "event" ADD COLUMN     "interludePhotos" TEXT[] DEFAULT ARRAY[]::TEXT[];

-- ===== 20261026120000_admin_must_change_password
-- Temporary passwords.

-- AlterTable: a generated password (new account, or a reset) is only good for
-- one sign-in; the holder must choose their own before the dashboard opens.
-- Existing accounts keep working as they are.
ALTER TABLE "admin_user" ADD COLUMN     "mustChangePassword" BOOLEAN NOT NULL DEFAULT false;

-- ===== 20261026130000_event_gift_image
-- AlterTable: an optional picture at the head of the gift section, above the
-- accounts, set the same way as the other event photos.
ALTER TABLE "event" ADD COLUMN     "giftImageUrl" TEXT;

-- ===== 20261027120000_event_cover_background
-- AlterTable: an optional full-bleed photo behind the opening cover. Covers
-- that do not use one ignore it; without it the keepsake cover falls back to
-- the plain band.
ALTER TABLE "event" ADD COLUMN     "coverBackgroundUrl" TEXT;

-- ===== 20261028090000_event_gift_address
-- AlterTable: a postal address for guests who would rather send a physical
-- gift than transfer, shown in the gift section with a copy button.
ALTER TABLE "event" ADD COLUMN     "giftAddress" TEXT;
