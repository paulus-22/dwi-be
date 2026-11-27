-- 0013_scanner_pairing_gate.sql
-- Gate name is now governed by the dashboard: each pairing code is bound to one
-- gate. The scanner device paired with that code adopts this name (its local
-- door pick is ignored for online pairing), so check-ins are attributed to the
-- correct entrance without the operator choosing on the device.
ALTER TABLE scanner_pairing_code
    ADD COLUMN gate TEXT NOT NULL DEFAULT 'Pintu Utama';
