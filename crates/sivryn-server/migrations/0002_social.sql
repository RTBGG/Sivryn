-- Friends, blocks, direct and group messages, moderation, guild settings,
-- message edits/reactions and file uploads.

-- Direct and group messages are channels without a guild.
ALTER TABLE channels ALTER COLUMN guild_id DROP NOT NULL;
ALTER TABLE channels DROP CONSTRAINT channels_kind_check;
ALTER TABLE channels ADD CONSTRAINT channels_kind_check CHECK (kind IN ('text', 'voice', 'dm', 'group'));
ALTER TABLE channels ADD CONSTRAINT channels_guild_or_private
    CHECK ((guild_id IS NULL) = (kind IN ('dm', 'group')));
ALTER TABLE channels ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE TABLE channel_members (
    channel_id UUID NOT NULL REFERENCES channels (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    joined_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (channel_id, user_id)
);
CREATE INDEX channel_members_user ON channel_members (user_id);

-- A pending friend request from one user to another.
CREATE TABLE friend_requests (
    from_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    to_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (from_id, to_id),
    CHECK (from_id <> to_id)
);
CREATE INDEX friend_requests_to ON friend_requests (to_id);

-- Friendships are stored once per direction so lookups stay simple.
CREATE TABLE friends (
    user_id   UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    friend_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    since     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, friend_id)
);

CREATE TABLE blocks (
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    blocked_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, blocked_id)
);

ALTER TABLE users ADD COLUMN allow_server_dms BOOLEAN NOT NULL DEFAULT TRUE;

CREATE TABLE guild_bans (
    guild_id   UUID NOT NULL REFERENCES guilds (id) ON DELETE CASCADE,
    user_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    banned_by  UUID REFERENCES users (id) ON DELETE SET NULL,
    reason     TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (guild_id, user_id)
);

-- Uploaded files. Attachments are encrypted by the client; the server only
-- knows their size and which channel may read them.
CREATE TABLE files (
    id           UUID PRIMARY KEY,
    uploader_id  UUID REFERENCES users (id) ON DELETE SET NULL,
    channel_id   UUID REFERENCES channels (id) ON DELETE CASCADE,
    guild_id     UUID REFERENCES guilds (id) ON DELETE CASCADE,
    size         BIGINT NOT NULL,
    content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE guilds ADD COLUMN icon_id UUID REFERENCES files (id) ON DELETE SET NULL;
ALTER TABLE guilds ADD COLUMN afk_channel_id UUID REFERENCES channels (id) ON DELETE SET NULL;
ALTER TABLE guilds ADD COLUMN afk_timeout_minutes INT NOT NULL DEFAULT 15;

-- Edits and reactions are encrypted messages pointing at their target, so
-- deleting a message removes them too.
ALTER TABLE messages ADD COLUMN kind TEXT NOT NULL DEFAULT 'message'
    CHECK (kind IN ('message', 'edit', 'reaction'));
ALTER TABLE messages ADD COLUMN target_id UUID REFERENCES messages (id) ON DELETE CASCADE;
CREATE INDEX messages_target ON messages (target_id) WHERE target_id IS NOT NULL;
