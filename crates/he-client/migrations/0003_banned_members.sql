-- M6: the owner can ban an account, and the roster has to be able to say so.
--
-- Cached alongside the rest of the member, so an offline window still shows
-- who is banned rather than showing them as an ordinary member who happens
-- never to be online.
ALTER TABLE cached_members ADD COLUMN banned INTEGER NOT NULL DEFAULT 0;
