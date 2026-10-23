-- Messages sealed with a channel's history key as well, so members who join
-- later can read earlier messages (clients hand them the key over MLS).
ALTER TABLE messages ADD COLUMN history TEXT;

-- Whether a guild's clients share history keys with new members.
ALTER TABLE guilds ADD COLUMN share_history BOOLEAN NOT NULL DEFAULT TRUE;

-- Key handovers are messages of their own kind.
ALTER TABLE messages DROP CONSTRAINT messages_kind_check;
ALTER TABLE messages ADD CONSTRAINT messages_kind_check CHECK (kind IN ('message', 'edit', 'reaction', 'history_keys'));
