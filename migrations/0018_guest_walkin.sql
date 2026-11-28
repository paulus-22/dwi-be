-- Walk-in guests added directly at the scanner door (not on the invite list),
-- so the team can tell them apart from invited guests.
ALTER TABLE "guest" ADD COLUMN "isWalkIn" boolean NOT NULL DEFAULT false;
