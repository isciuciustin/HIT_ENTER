-- Initial schema. See docs/PLAN.md §8.
--
-- Migrations are append-only: once this file has shipped it is frozen, and a
-- change means 0002_*.sql. Editing it would silently diverge from every
-- database already in the wild.
--
-- IDs are UUIDv7 stored as TEXT. They sort chronologically, so `ORDER BY id`
-- is `ORDER BY time`, a pagination cursor is just an ID, and nothing has to
-- coordinate a sequence.
--
-- Timestamps are unix seconds in INTEGER columns.
--
-- Every TEXT PRIMARY KEY is also declared NOT NULL. SQLite otherwise allows
-- NULL in a non-INTEGER primary key — a documented compatibility bug it will
-- not fix — and a row with a NULL id would be unreachable and unreferenceable.

CREATE TABLE users (
  id            TEXT PRIMARY KEY NOT NULL,
  username      TEXT NOT NULL,
  username_ci   TEXT NOT NULL UNIQUE,   -- lowercased; the real uniqueness key
  password_hash TEXT NOT NULL,          -- argon2id PHC string, never reversible
  display_name  TEXT,
  is_owner      INTEGER NOT NULL DEFAULT 0,
  created_at    INTEGER NOT NULL
);

-- The bridge between iroh's device identity and our account identity.
-- endpoint_id is always read from the iroh connection, never from a message
-- body (PLAN §11).
CREATE TABLE devices (
  endpoint_id TEXT NOT NULL,            -- iroh EndpointId (z32 public key)
  user_id     TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  label       TEXT,                     -- "Justin's laptop"
  enrolled_at INTEGER NOT NULL,
  last_seen   INTEGER NOT NULL,
  revoked_at  INTEGER,
  PRIMARY KEY (endpoint_id, user_id)
);

CREATE TABLE channels (
  id         TEXT PRIMARY KEY NOT NULL,
  name       TEXT NOT NULL,
  topic      TEXT,
  position   INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE messages (
  id         TEXT PRIMARY KEY NOT NULL, -- UUIDv7 == creation time
  channel_id TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  author_id  TEXT NOT NULL REFERENCES users(id),
  content    TEXT NOT NULL,             -- PLAINTEXT, by design — see PLAN §10
  edited_at  INTEGER,
  deleted_at INTEGER                    -- soft delete: clients must be told
);
CREATE INDEX idx_messages_channel_id ON messages(channel_id, id DESC);

CREATE TABLE invites (
  code       TEXT PRIMARY KEY NOT NULL,
  created_by TEXT NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL,
  expires_at INTEGER,                   -- NULL = never
  max_uses   INTEGER,                   -- NULL = unlimited
  uses       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE server_meta (              -- single row, id = 1
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  name        TEXT NOT NULL,
  secret_key  BLOB NOT NULL,            -- iroh SecretKey: THE server's identity
  created_at  INTEGER NOT NULL
);
