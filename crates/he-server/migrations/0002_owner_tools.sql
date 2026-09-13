-- M6: the owner's tools. See docs/PLAN.md §12.
--
-- Migrations are append-only: 0001 is frozen, and everything here is additive.

-- A banned account cannot authenticate at all — not with a password, not with
-- an enrolled device.
--
-- On the account rather than on the device, because a device key costs nothing
-- to regenerate: banning one would be banning a string the other side can
-- change at will. Banning the account works because the account is the thing
-- that took a password and an invite to create (PLAN §3).
--
-- Nullable rather than a boolean so the row records *when*, which is the only
-- thing an owner ever wants to know about a ban they are looking at later.
-- The account itself survives: its messages keep their author, and un-banning
-- is one UPDATE rather than a re-registration.
ALTER TABLE users ADD COLUMN banned_at INTEGER;
