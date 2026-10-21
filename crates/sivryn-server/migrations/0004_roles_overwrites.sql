-- Role colors and grouping, and permission overwrites for channels and
-- categories (private channels, read-only channels and the like).

ALTER TABLE roles ADD COLUMN color INT;
ALTER TABLE roles ADD COLUMN hoist BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE roles ADD COLUMN created_at TIMESTAMPTZ NOT NULL DEFAULT now();

CREATE TABLE permission_overwrites (
    guild_id    UUID NOT NULL REFERENCES guilds (id) ON DELETE CASCADE,
    channel_id  UUID REFERENCES channels (id) ON DELETE CASCADE,
    category_id UUID REFERENCES categories (id) ON DELETE CASCADE,
    role_id     UUID REFERENCES roles (id) ON DELETE CASCADE,
    user_id     UUID REFERENCES users (id) ON DELETE CASCADE,
    allow_bits  BIGINT NOT NULL DEFAULT 0,
    deny_bits   BIGINT NOT NULL DEFAULT 0,
    -- Exactly one place and exactly one target.
    CHECK ((channel_id IS NULL) <> (category_id IS NULL)),
    CHECK ((role_id IS NULL) <> (user_id IS NULL))
);
CREATE UNIQUE INDEX permission_overwrites_one
    ON permission_overwrites (COALESCE(channel_id, category_id), COALESCE(role_id, user_id));
CREATE INDEX permission_overwrites_guild ON permission_overwrites (guild_id);
