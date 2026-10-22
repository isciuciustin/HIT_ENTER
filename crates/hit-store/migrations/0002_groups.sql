-- The groups this node carries (M3): created here, or joined. A group is
-- carried from the moment we create it or send a join, before any of its
-- state has arrived — that is how a joining node knows to accept the group
-- key's events at all.

CREATE TABLE groups (
    id       BLOB    PRIMARY KEY NOT NULL,   -- the group's public key, 32 bytes
    added_at INTEGER NOT NULL
) STRICT;
