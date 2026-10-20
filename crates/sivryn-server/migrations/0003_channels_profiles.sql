-- Channel categories, voice bitrate, profiles and status, per-guild names,
-- notification levels, channel mutes and read states.

CREATE TABLE categories (
    id         UUID PRIMARY KEY,
    guild_id   UUID NOT NULL REFERENCES guilds (id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    position   INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX categories_guild ON categories (guild_id, position);

ALTER TABLE channels ADD COLUMN category_id UUID REFERENCES categories (id) ON DELETE SET NULL;

-- Existing guilds get the two categories their channel list used to show.
INSERT INTO categories (id, guild_id, name, position)
SELECT gen_random_uuid(), g.id, c.name, c.position
FROM guilds g CROSS JOIN (VALUES ('Text Channels', 0), ('Voice Channels', 1)) AS c (name, position);
UPDATE channels ch SET category_id = cat.id
FROM categories cat
WHERE cat.guild_id = ch.guild_id
  AND cat.name = CASE WHEN ch.kind = 'voice' THEN 'Voice Channels' ELSE 'Text Channels' END;

ALTER TABLE guilds ADD COLUMN audio_bitrate INT NOT NULL DEFAULT 128000;

ALTER TABLE users ADD COLUMN bio TEXT NOT NULL DEFAULT '';
ALTER TABLE users ADD COLUMN avatar_id UUID REFERENCES files (id) ON DELETE SET NULL;
ALTER TABLE users ADD COLUMN banner_id UUID REFERENCES files (id) ON DELETE SET NULL;
ALTER TABLE users ADD COLUMN status TEXT NOT NULL DEFAULT 'online'
    CHECK (status IN ('online', 'idle', 'dnd', 'invisible'));

-- Avatars and banners belong to a user and are public.
ALTER TABLE files ADD COLUMN owner_user_id UUID REFERENCES users (id) ON DELETE CASCADE;

ALTER TABLE guild_members ADD COLUMN nick TEXT;
ALTER TABLE guild_members ADD COLUMN notify_level TEXT NOT NULL DEFAULT 'all'
    CHECK (notify_level IN ('all', 'mentions', 'nothing'));

CREATE TABLE channel_mutes (
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel_id UUID NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    -- NULL: until unmuted.
    until      TIMESTAMPTZ,
    PRIMARY KEY (user_id, channel_id)
);

-- How far a user has read each channel (the newest message they saw).
CREATE TABLE read_states (
    user_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    channel_id   UUID NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    last_read_id UUID NOT NULL,
    PRIMARY KEY (user_id, channel_id)
);
