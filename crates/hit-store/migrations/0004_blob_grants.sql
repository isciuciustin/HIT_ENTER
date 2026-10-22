-- DM files (M8b). Append-only.
--
-- Who may be handed a DM file's ciphertext. A group file's readers are
-- whoever may read the message carrying it; a DM file's message is sealed,
-- so no node can tell from the store alone. The two people's own nodes
-- record it when they send or open the file: these two, nobody else.
CREATE TABLE blob_grants (
    hash   TEXT NOT NULL,   -- BLAKE3 of the ciphertext
    person BLOB NOT NULL,   -- 32 bytes
    PRIMARY KEY (hash, person)
) STRICT;
