-- Export a WEDDING (or GENERIC) event's guest list for WhatsApp invitation
-- sending: name, phone, category, personal invite link.
--
-- Usage:
--   psql "$DATABASE_URL" -v slug="'tari-saefulloh'" -v site_url="'https://preview-tm.luspau.my.id'" \
--     -f export_guests_for_wa.sql -A -F',' -t -o guests.csv
--
-- `-A -F',' -t` makes psql emit plain unaligned CSV-ish rows with no header/
-- footer; add a header line yourself if you want one (the Node script below
-- doesn't need one — it reads columns positionally).
--
-- Guests with no phone number are skipped (nothing to send to). Already
-- checked-in / already-attending guests are NOT filtered out here — this is
-- for the *invitation* blast, independent of RSVP status; filter separately
-- if you only want to (re-)invite a subset.
select
  g."householdName" as name,
  g.phone,
  coalesce(g.category, 'umum') as category,
  :site_url || '/e/' || e.slug || '?to=' || g.token as link
from guest g
join event e on e.id = g."eventId"
where e.slug = :slug
  and g.phone is not null
  and g.phone <> ''
order by g.sequence, g."householdName";
