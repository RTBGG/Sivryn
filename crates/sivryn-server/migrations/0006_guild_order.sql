-- Where a server sits in its member's server list (dragged there in the client).
-- NULL: never placed, listed after the placed ones by join time.
ALTER TABLE guild_members ADD COLUMN position INT;
