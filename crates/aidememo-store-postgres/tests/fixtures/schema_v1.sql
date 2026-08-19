CREATE TABLE aidememo_schema (
    component TEXT PRIMARY KEY,
    version INTEGER NOT NULL CHECK (version > 0)
);
INSERT INTO aidememo_schema (component, version) VALUES ('canonical_store', 1);

CREATE TABLE ssot_projects (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    project_epoch TEXT NOT NULL,
    next_seq BIGINT NOT NULL CHECK (next_seq >= 0),
    PRIMARY KEY (tenant_id, project_id)
);

CREATE TABLE ssot_resources (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    deleted BOOLEAN NOT NULL,
    body_json BYTEA,
    CHECK ((NOT deleted AND body_json IS NOT NULL) OR (deleted AND body_json IS NULL)),
    PRIMARY KEY (tenant_id, project_id, resource_kind, resource_id),
    FOREIGN KEY (tenant_id, project_id)
        REFERENCES ssot_projects (tenant_id, project_id) ON DELETE CASCADE
);

CREATE TABLE ssot_receipts (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    command_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    project_seq BIGINT NOT NULL CHECK (project_seq > 0),
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    revision BIGINT NOT NULL CHECK (revision > 0),
    committed_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, command_id),
    UNIQUE (tenant_id, project_id, project_seq),
    FOREIGN KEY (tenant_id, project_id)
        REFERENCES ssot_projects (tenant_id, project_id) ON DELETE CASCADE
);

CREATE TABLE ssot_changes (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    project_epoch TEXT NOT NULL,
    seq BIGINT NOT NULL CHECK (seq > 0),
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    operation TEXT NOT NULL CHECK (operation IN ('upsert', 'delete')),
    revision BIGINT NOT NULL CHECK (revision > 0),
    actor_id TEXT NOT NULL,
    committed_at_ms BIGINT NOT NULL,
    body_json BYTEA,
    CHECK ((operation = 'upsert' AND body_json IS NOT NULL)
        OR (operation = 'delete' AND body_json IS NULL)),
    PRIMARY KEY (tenant_id, project_id, seq),
    FOREIGN KEY (tenant_id, project_id)
        REFERENCES ssot_projects (tenant_id, project_id) ON DELETE CASCADE
);

CREATE TABLE ssot_audit (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    project_seq BIGINT NOT NULL CHECK (project_seq > 0),
    command_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    resource_kind TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    committed_at_ms BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, project_id, project_seq),
    UNIQUE (tenant_id, project_id, command_id),
    FOREIGN KEY (tenant_id, project_id)
        REFERENCES ssot_projects (tenant_id, project_id) ON DELETE CASCADE
);

CREATE TABLE ssot_handoff_index (
    tenant_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    handoff_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL DEFAULT 'handoff' CHECK (resource_kind = 'handoff'),
    from_actor TEXT NOT NULL,
    to_actor TEXT NOT NULL,
    source_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'accepted', 'completed')),
    revision BIGINT NOT NULL CHECK (revision > 0),
    updated_seq BIGINT NOT NULL CHECK (updated_seq > 0),
    PRIMARY KEY (tenant_id, project_id, handoff_id),
    FOREIGN KEY (tenant_id, project_id, resource_kind, handoff_id)
        REFERENCES ssot_resources (tenant_id, project_id, resource_kind, resource_id)
        ON DELETE CASCADE
);

CREATE INDEX ssot_handoff_inbox_idx
    ON ssot_handoff_index (tenant_id, project_id, to_actor, updated_seq DESC);
CREATE INDEX ssot_handoff_outbox_idx
    ON ssot_handoff_index (tenant_id, project_id, from_actor, updated_seq DESC);
