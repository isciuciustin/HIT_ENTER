-- M5: the three things a client needs to survive losing its connection.
--
-- Migrations are append-only: 0001 is frozen, and everything here is additive.

-- Which account this client is on a server. `servers.username` was enough to
-- log in with, and it is not enough to answer "is this message mine?" — an
-- unread count that includes your own messages is a badge that never clears.
ALTER TABLE servers ADD COLUMN user_id TEXT;

-- When this client last had a complete picture of a server.
--
-- The cursors in `sync_state` say what is *new*; this says what may have
-- *changed*. A message edited or deleted while the app was closed keeps its
-- id, so it is older than every cursor and no "give me what is new" would
-- ever mention it — it would sit on screen with its original text forever.
CREATE TABLE sync_marks (
  endpoint_id TEXT PRIMARY KEY NOT NULL
              REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  resumed_at  INTEGER NOT NULL          -- unix seconds
);

-- How far the user has read in each channel, for the unread badge.
--
-- A message id rather than a count or a timestamp: ids are UUIDv7, so
-- "unread" is `id > last_read_id`, which stays correct when history arrives
-- out of order and needs no recount when a page is backfilled.
CREATE TABLE read_state (
  endpoint_id  TEXT NOT NULL REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  channel_id   TEXT NOT NULL,
  last_read_id TEXT NOT NULL,
  PRIMARY KEY (endpoint_id, channel_id)
);

-- The member list, cached for the same reason the channel list is: a roster
-- that only exists while connected is a roster that is blank exactly when you
-- are trying to work out who said something.
--
-- Presence is *not* here. Who is online right now is live state and belongs to
-- the connection; a stored "online" would be a lie every time this app was
-- closed (PLAN §6).
CREATE TABLE cached_members (
  endpoint_id  TEXT NOT NULL REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  id           TEXT NOT NULL,
  username     TEXT NOT NULL,
  display_name TEXT,
  is_owner     INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (endpoint_id, id)
);
