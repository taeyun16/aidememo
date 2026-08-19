CREATE TABLE IF NOT EXISTS ssot_tenants (
    tenant_id TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'archived')),
    revision BIGINT NOT NULL CHECK (revision > 0),
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);

ALTER TABLE ssot_projects ADD COLUMN IF NOT EXISTS display_name TEXT NOT NULL DEFAULT '';
ALTER TABLE ssot_projects ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active';
ALTER TABLE ssot_projects ADD COLUMN IF NOT EXISTS revision BIGINT NOT NULL DEFAULT 1;
ALTER TABLE ssot_projects ADD COLUMN IF NOT EXISTS created_at_ms BIGINT NOT NULL DEFAULT 0;
ALTER TABLE ssot_projects ADD COLUMN IF NOT EXISTS updated_at_ms BIGINT NOT NULL DEFAULT 0;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'ssot_projects_status_check'
          AND conrelid = 'ssot_projects'::regclass
    ) THEN
        ALTER TABLE ssot_projects
            ADD CONSTRAINT ssot_projects_status_check
            CHECK (status IN ('active', 'suspended', 'archived'));
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'ssot_projects_revision_check'
          AND conrelid = 'ssot_projects'::regclass
    ) THEN
        ALTER TABLE ssot_projects
            ADD CONSTRAINT ssot_projects_revision_check CHECK (revision > 0);
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS ssot_actors (
    tenant_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('human', 'agent', 'service')),
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended', 'archived')),
    revision BIGINT NOT NULL CHECK (revision > 0),
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, actor_id),
    FOREIGN KEY (tenant_id) REFERENCES ssot_tenants (tenant_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS ssot_memberships (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('owner', 'admin', 'writer', 'reader')),
    status TEXT NOT NULL CHECK (status IN ('active', 'suspended')),
    PRIMARY KEY (tenant_id, project_id, actor_id),
    FOREIGN KEY (tenant_id, project_id)
        REFERENCES ssot_projects (tenant_id, project_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, actor_id)
        REFERENCES ssot_actors (tenant_id, actor_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS ssot_token_bindings (
    token_sha256 BYTEA PRIMARY KEY CHECK (octet_length(token_sha256) = 32),
    tenant_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    active BOOLEAN NOT NULL,
    created_at_ms BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, actor_id)
        REFERENCES ssot_actors (tenant_id, actor_id) ON DELETE CASCADE
);
