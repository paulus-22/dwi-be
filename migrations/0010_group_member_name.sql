-- Group (Kelompok) check-in member name. When a GROUP guest's QR is scanned
-- at the door, the usher types which member arrived (e.g. "Paul dari Lulusan
-- IT"). That name is stored on the check-in so the admin can see who actually
-- showed up from a group.
-- Additive; individual guests simply leave it null (unchanged behaviour).
ALTER TABLE "check_in" ADD COLUMN IF NOT EXISTS "memberName" TEXT;
