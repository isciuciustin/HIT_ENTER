-- The event store (M2). Append-only: a schema change is a new numbered file.
--
-- Events are kept as their own JSON, plus the columns NIP-01 filters query.
-- Single-letter tags get their own table so a `#h` / `#e` / `#p` filter is an
-- indexed lookup, not a scan (spike S1).

CREATE TABLE events (
    id         BLOB    PRIMARY KEY NOT NULL,   -- 32 bytes
    pubkey     BLOB    NOT NULL,               -- 32 bytes
    created_at INTEGER NOT NULL,
    kind       INTEGER NOT NULL,
    -- The replaceable slot: '' for kinds 0, 3, 10000–19999, the `d` tag for
    -- 30000–39999, NULL for regular events. At most one event per slot.
    slot_d     TEXT,
    json       TEXT    NOT NULL
) STRICT;

CREATE INDEX events_time   ON events (created_at, id);
CREATE INDEX events_author ON events (pubkey, created_at);
CREATE INDEX events_kind   ON events (kind, created_at);
CREATE UNIQUE INDEX events_slot ON events (kind, pubkey, slot_d) WHERE slot_d IS NOT NULL;

CREATE TABLE tags (
    event_id BLOB NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    name     TEXT NOT NULL,
    value    TEXT NOT NULL
) STRICT;

CREATE INDEX tags_lookup ON tags (name, value, event_id);
CREATE INDEX tags_event  ON tags (event_id);
