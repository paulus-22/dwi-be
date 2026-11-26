-- Pre-wedding photo gallery for the invitation slideshow. A URL per image,
-- served from public/photos/<slug>/ (or R2), rendered as a slideshow section.
-- Additive; matches the existing interludePhotos array column.
ALTER TABLE "event" ADD COLUMN IF NOT EXISTS "galleryPhotos" TEXT[] DEFAULT ARRAY[]::TEXT[];
