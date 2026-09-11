-- The client mirror. See docs/PLAN.md §8.
--
-- This is the file that makes "all your messages are on your disk" true for
-- *members*, not just for the host. Everything a client has ever seen lands
-- here, in plaintext, so the app opens and reads with the network off and so
-- FTS5 search over it is possible at all in M7 (PLAN §10).
--
-- One mirror holds every server this client knows, keyed by `endpoint_id` —
-- the server's public key, which survives its IP changing, its ISP changing,
-- and its owner moving to a different city (PLAN §4).
--
-- Migrations are append-only: once this has shipped it is frozen.

CREATE TABLE servers (
  endpoint_id TEXT PRIMARY KEY NOT NULL,  -- identity key: survives IP changes
  name        TEXT NOT NULL,
  relay_url   TEXT,                       -- last known home relay
  username    TEXT NOT NULL,              -- which account this client uses here
  added_at    INTEGER NOT NULL,
  last_seen   INTEGER                     -- last successful connection
);

-- Cached so the channel rail renders with the network off. Not in PLAN §8's
-- sketch, which listed only messages — but a message pane with no channel list
-- is not an app you can open on a train.
CREATE TABLE cached_channels (
  endpoint_id TEXT NOT NULL REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  id          TEXT NOT NULL,
  name        TEXT NOT NULL,
  topic       TEXT,
  position    INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (endpoint_id, id)
);

CREATE TABLE cached_messages (
  endpoint_id TEXT NOT NULL REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  id          TEXT NOT NULL,              -- UUIDv7: also the sort key
  channel_id  TEXT NOT NULL,
  author_id   TEXT NOT NULL,
  author_name TEXT NOT NULL,              -- denormalised: renders offline, and
                                          -- after the account was renamed
  content     TEXT NOT NULL,              -- PLAINTEXT: makes FTS5 possible
  edited_at   INTEGER,
  deleted_at  INTEGER,
  PRIMARY KEY (endpoint_id, id)
);
CREATE INDEX idx_cached_messages_channel
  ON cached_messages (endpoint_id, channel_id, id DESC);

-- Resume point per channel, so reconnecting is not a reload (PLAN §9). M3
-- keeps it current; M5's `resume` request is what reads it.
CREATE TABLE sync_state (
  endpoint_id TEXT NOT NULL REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  channel_id  TEXT NOT NULL,
  last_id     TEXT NOT NULL,
  PRIMARY KEY (endpoint_id, channel_id)
);

-- Messages composed while offline. M3 writes and clears it around every send;
-- M5 is what drains it after a reconnect.
CREATE TABLE outbox (
  nonce       TEXT PRIMARY KEY NOT NULL,
  endpoint_id TEXT NOT NULL REFERENCES servers(endpoint_id) ON DELETE CASCADE,
  channel_id  TEXT NOT NULL,
  content     TEXT NOT NULL,
  created_at  INTEGER NOT NULL
);
CREATE INDEX idx_outbox_server ON outbox (endpoint_id, created_at);
