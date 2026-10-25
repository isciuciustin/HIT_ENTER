-- When this node stored each event (M13). Append-only.
--
-- `created_at` is the author's word, and NIP-59 dates a DM's wrap up to two
-- days back at random: an anchor's DM inbox expires by when a wrap
-- *arrived*. Rows from before this migration count from the migration.
ALTER TABLE events ADD COLUMN received_at INTEGER NOT NULL DEFAULT 0;
UPDATE events SET received_at = CAST(strftime('%s', 'now') AS INTEGER);
CREATE INDEX events_received ON events (kind, received_at);
