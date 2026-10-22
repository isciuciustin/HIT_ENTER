-- Files (M8). Append-only: a schema change is a new numbered file.
--
-- Which messages carry which file, by its BLAKE3 hash. Filled when a message
-- with `imeta` tags is stored and emptied with it (a withdrawal, a
-- moderator's removal, a ban's silence): the one question a node answers
-- before serving a file — "is it attached to something this person may
-- read?" — and the live set its blob store keeps.
CREATE TABLE attachments (
    hash     TEXT    NOT NULL,                -- BLAKE3, 64 lowercase hex
    event_id BLOB    NOT NULL REFERENCES events (id) ON DELETE CASCADE,
    size     INTEGER NOT NULL
) STRICT;

CREATE INDEX attachments_hash  ON attachments (hash);
CREATE INDEX attachments_event ON attachments (event_id);

-- This node's own use of each file: when it last used it, and whether it
-- posted it. What the cache keeps when over its size, newest first; a file
-- you posted is kept until its message goes.
CREATE TABLE blob_use (
    hash      TEXT    PRIMARY KEY NOT NULL,
    last_used INTEGER NOT NULL,
    mine      INTEGER NOT NULL DEFAULT 0
) STRICT;
