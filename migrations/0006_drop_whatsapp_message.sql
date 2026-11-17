-- Drop the outbound WhatsApp message log. The Fonnte provider integration
-- (crates/api/src/whatsapp.rs) is removed: outbound invitations are now sent
-- by a standalone local script (scripts/wa-invite-sender) over WhatsApp Web,
-- and RSVP/registration confirmations surface their QR link in the web UI
-- instead of being pushed over WhatsApp automatically. With no writer left,
-- the table is dead weight and this removes it cleanly.
DROP TABLE IF EXISTS "whatsapp_message";
