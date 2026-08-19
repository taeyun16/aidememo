use crate::{PostgresCommandStore, storage};
use aidememo_domain::{
    ActorId, ActorKind, ActorRecord, AuthenticatedActor, DomainError, MembershipRole,
    MembershipStatus, ProjectEpoch, ProjectId, ProjectMembership, ProjectRecord, ProjectScope,
    RecordStatus, Revision, ServerIdentityStore, TenantId, TenantRecord,
};

impl ServerIdentityStore for PostgresCommandStore {
    fn schema_version(&self) -> Result<u32, DomainError> {
        PostgresCommandStore::schema_version(self)
    }

    fn project_epoch(&self, scope: &ProjectScope) -> Result<Option<ProjectEpoch>, DomainError> {
        let mut client = self.lock_client()?;
        let row = client
            .query_opt(
                "SELECT project_epoch FROM ssot_projects
                 WHERE tenant_id = $1 AND project_id = $2",
                &[&scope.tenant_id.as_str(), &scope.project_id.as_str()],
            )
            .map_err(|error| storage("project_epoch_read", error))?;
        row.map(|row| ProjectEpoch::try_from(row.get::<_, String>(0)))
            .transpose()
    }

    fn bootstrap_project(
        &mut self,
        tenant: &TenantRecord,
        project: &ProjectRecord,
    ) -> Result<(), DomainError> {
        validate_tenant_project(tenant, project)?;
        let mut client = self.lock_client()?;
        let mut tx = client
            .transaction()
            .map_err(|error| storage("bootstrap_project_begin", error))?;
        tx.execute(
            "INSERT INTO ssot_tenants
                (tenant_id, display_name, status, revision, created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (tenant_id) DO NOTHING",
            &[
                &tenant.tenant_id.as_str(),
                &tenant.display_name,
                &record_status_text(tenant.status),
                &to_i64(tenant.revision.get(), "tenant_revision")?,
                &tenant.created_at_ms,
                &tenant.updated_at_ms,
            ],
        )
        .map_err(|error| storage("tenant_bootstrap_write", error))?;
        ensure_tenant_matches(&mut tx, tenant)?;

        tx.execute(
            "INSERT INTO ssot_projects
                (tenant_id, project_id, project_epoch, next_seq, display_name,
                 status, revision, created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, 0, $4, $5, $6, $7, $8)
             ON CONFLICT (tenant_id, project_id) DO NOTHING",
            &[
                &project.tenant_id.as_str(),
                &project.project_id.as_str(),
                &project.project_epoch.as_str(),
                &project.display_name,
                &record_status_text(project.status),
                &to_i64(project.revision.get(), "project_revision")?,
                &project.created_at_ms,
                &project.updated_at_ms,
            ],
        )
        .map_err(|error| storage("project_bootstrap_write", error))?;
        tx.execute(
            "UPDATE ssot_projects
             SET display_name = $4, status = $5, revision = $6,
                 created_at_ms = $7, updated_at_ms = $8
             WHERE tenant_id = $1 AND project_id = $2 AND project_epoch = $3
               AND display_name = '' AND status = 'active' AND revision = 1
               AND created_at_ms = 0 AND updated_at_ms = 0",
            &[
                &project.tenant_id.as_str(),
                &project.project_id.as_str(),
                &project.project_epoch.as_str(),
                &project.display_name,
                &record_status_text(project.status),
                &to_i64(project.revision.get(), "project_revision")?,
                &project.created_at_ms,
                &project.updated_at_ms,
            ],
        )
        .map_err(|error| storage("project_bootstrap_adopt", error))?;
        ensure_project_matches(&mut tx, project)?;
        tx.commit()
            .map_err(|error| storage("bootstrap_project_commit", error))
    }

    fn provision_actor(
        &mut self,
        actor: &ActorRecord,
        membership: &ProjectMembership,
        token_sha256: &[u8],
        created_at_ms: i64,
    ) -> Result<(), DomainError> {
        validate_actor_membership(actor, membership, token_sha256)?;
        let mut client = self.lock_client()?;
        let mut tx = client
            .transaction()
            .map_err(|error| storage("provision_actor_begin", error))?;
        ensure_active_tenant_project(&mut tx, &membership.tenant_id, &membership.project_id)?;
        tx.execute(
            "INSERT INTO ssot_actors
                (tenant_id, actor_id, display_name, kind, status, revision,
                 created_at_ms, updated_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (tenant_id, actor_id) DO NOTHING",
            &[
                &actor.tenant_id.as_str(),
                &actor.actor_id.as_str(),
                &actor.display_name,
                &actor_kind_text(actor.kind),
                &record_status_text(actor.status),
                &to_i64(actor.revision.get(), "actor_revision")?,
                &actor.created_at_ms,
                &actor.updated_at_ms,
            ],
        )
        .map_err(|error| storage("actor_provision_write", error))?;
        ensure_actor_matches(&mut tx, actor)?;
        tx.execute(
            "INSERT INTO ssot_memberships
                (tenant_id, project_id, actor_id, role, status)
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (tenant_id, project_id, actor_id) DO NOTHING",
            &[
                &membership.tenant_id.as_str(),
                &membership.project_id.as_str(),
                &membership.actor_id.as_str(),
                &membership_role_text(membership.role),
                &membership_status_text(membership.status),
            ],
        )
        .map_err(|error| storage("membership_provision_write", error))?;
        ensure_membership_matches(&mut tx, membership)?;
        tx.execute(
            "INSERT INTO ssot_token_bindings
                (token_sha256, tenant_id, actor_id, active, created_at_ms)
             VALUES ($1, $2, $3, TRUE, $4)
             ON CONFLICT (token_sha256) DO NOTHING",
            &[
                &token_sha256,
                &actor.tenant_id.as_str(),
                &actor.actor_id.as_str(),
                &created_at_ms,
            ],
        )
        .map_err(|error| storage("token_provision_write", error))?;
        ensure_token_matches(&mut tx, token_sha256, actor)?;
        tx.commit()
            .map_err(|error| storage("provision_actor_commit", error))
    }

    fn authenticate_token(
        &self,
        token_sha256: &[u8],
    ) -> Result<Option<AuthenticatedActor>, DomainError> {
        if token_sha256.len() != 32 {
            return Err(DomainError::InvalidCommand(
                "bearer token digest must contain 32 bytes".to_owned(),
            ));
        }
        let mut client = self.lock_client()?;
        let row = client
            .query_opt(
                "SELECT binding.tenant_id, binding.actor_id
                 FROM ssot_token_bindings AS binding
                 JOIN ssot_tenants AS tenant
                   ON tenant.tenant_id = binding.tenant_id
                 JOIN ssot_actors AS actor
                   ON actor.tenant_id = binding.tenant_id
                  AND actor.actor_id = binding.actor_id
                 WHERE binding.token_sha256 = $1 AND binding.active = TRUE
                   AND tenant.status = 'active' AND actor.status = 'active'",
                &[&token_sha256],
            )
            .map_err(|error| storage("token_authenticate", error))?;
        row.map(|row| {
            Ok(AuthenticatedActor::new(
                TenantId::try_from(row.get::<_, String>(0))?,
                ActorId::try_from(row.get::<_, String>(1))?,
            ))
        })
        .transpose()
    }

    fn project_membership(
        &self,
        scope: &ProjectScope,
        actor_id: &ActorId,
    ) -> Result<Option<ProjectMembership>, DomainError> {
        let mut client = self.lock_client()?;
        let row = client
            .query_opt(
                "SELECT membership.role, membership.status
                 FROM ssot_memberships AS membership
                 JOIN ssot_tenants AS tenant
                   ON tenant.tenant_id = membership.tenant_id
                 JOIN ssot_projects AS project
                   ON project.tenant_id = membership.tenant_id
                  AND project.project_id = membership.project_id
                 JOIN ssot_actors AS actor
                   ON actor.tenant_id = membership.tenant_id
                  AND actor.actor_id = membership.actor_id
                 WHERE membership.tenant_id = $1 AND membership.project_id = $2
                   AND membership.actor_id = $3 AND membership.status = 'active'
                   AND tenant.status = 'active' AND project.status = 'active'
                   AND actor.status = 'active'",
                &[
                    &scope.tenant_id.as_str(),
                    &scope.project_id.as_str(),
                    &actor_id.as_str(),
                ],
            )
            .map_err(|error| storage("membership_read", error))?;
        row.map(|row| {
            Ok(ProjectMembership {
                tenant_id: scope.tenant_id.clone(),
                project_id: scope.project_id.clone(),
                actor_id: actor_id.clone(),
                role: parse_membership_role(row.get::<_, String>(0).as_str())?,
                status: parse_membership_status(row.get::<_, String>(1).as_str())?,
            })
        })
        .transpose()
    }
}

fn validate_tenant_project(
    tenant: &TenantRecord,
    project: &ProjectRecord,
) -> Result<(), DomainError> {
    if tenant.tenant_id != project.tenant_id {
        return Err(DomainError::InvalidCommand(
            "project tenant does not match tenant record".to_owned(),
        ));
    }
    validate_record_text("tenant display_name", &tenant.display_name)?;
    validate_record_text("project display_name", &project.display_name)?;
    validate_record_times("tenant", tenant.created_at_ms, tenant.updated_at_ms)?;
    validate_record_times("project", project.created_at_ms, project.updated_at_ms)
}

fn validate_actor_membership(
    actor: &ActorRecord,
    membership: &ProjectMembership,
    token_sha256: &[u8],
) -> Result<(), DomainError> {
    if actor.tenant_id != membership.tenant_id || actor.actor_id != membership.actor_id {
        return Err(DomainError::InvalidCommand(
            "actor identity does not match membership".to_owned(),
        ));
    }
    if token_sha256.len() != 32 {
        return Err(DomainError::InvalidCommand(
            "bearer token digest must contain 32 bytes".to_owned(),
        ));
    }
    validate_record_text("actor display_name", &actor.display_name)?;
    validate_record_times("actor", actor.created_at_ms, actor.updated_at_ms)
}

fn validate_record_text(name: &str, value: &str) -> Result<(), DomainError> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        return Err(DomainError::InvalidCommand(format!(
            "{name} must contain 1 to 256 bytes and no control characters"
        )));
    }
    Ok(())
}

fn validate_record_times(name: &str, created: i64, updated: i64) -> Result<(), DomainError> {
    if created < 0 || updated < created {
        return Err(DomainError::InvalidCommand(format!(
            "{name} timestamps must be non-negative and updated_at must not precede created_at"
        )));
    }
    Ok(())
}

fn ensure_tenant_matches(
    tx: &mut postgres::Transaction<'_>,
    tenant: &TenantRecord,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM ssot_tenants
             WHERE tenant_id = $1 AND status = $2 AND revision = $3",
            &[
                &tenant.tenant_id.as_str(),
                &record_status_text(tenant.status),
                &to_i64(tenant.revision.get(), "tenant_revision")?,
            ],
        )
        .map_err(|error| storage("tenant_bootstrap_read", error))?
        .get(0);
    ensure_exact_match(matched, "tenant_bootstrap", "tenant record conflicts")
}

fn ensure_project_matches(
    tx: &mut postgres::Transaction<'_>,
    project: &ProjectRecord,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM ssot_projects
             WHERE tenant_id = $1 AND project_id = $2 AND project_epoch = $3
               AND status = $4 AND revision = $5",
            &[
                &project.tenant_id.as_str(),
                &project.project_id.as_str(),
                &project.project_epoch.as_str(),
                &record_status_text(project.status),
                &to_i64(project.revision.get(), "project_revision")?,
            ],
        )
        .map_err(|error| storage("project_bootstrap_read", error))?
        .get(0);
    ensure_exact_match(matched, "project_bootstrap", "project record conflicts")
}

fn ensure_actor_matches(
    tx: &mut postgres::Transaction<'_>,
    actor: &ActorRecord,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM ssot_actors
             WHERE tenant_id = $1 AND actor_id = $2
               AND kind = $3 AND status = $4 AND revision = $5",
            &[
                &actor.tenant_id.as_str(),
                &actor.actor_id.as_str(),
                &actor_kind_text(actor.kind),
                &record_status_text(actor.status),
                &to_i64(actor.revision.get(), "actor_revision")?,
            ],
        )
        .map_err(|error| storage("actor_provision_read", error))?
        .get(0);
    ensure_exact_match(matched, "actor_provision", "actor record conflicts")
}

fn ensure_membership_matches(
    tx: &mut postgres::Transaction<'_>,
    membership: &ProjectMembership,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM ssot_memberships
             WHERE tenant_id = $1 AND project_id = $2 AND actor_id = $3
               AND role = $4 AND status = $5",
            &[
                &membership.tenant_id.as_str(),
                &membership.project_id.as_str(),
                &membership.actor_id.as_str(),
                &membership_role_text(membership.role),
                &membership_status_text(membership.status),
            ],
        )
        .map_err(|error| storage("membership_provision_read", error))?
        .get(0);
    ensure_exact_match(
        matched,
        "membership_provision",
        "membership record conflicts",
    )
}

fn ensure_token_matches(
    tx: &mut postgres::Transaction<'_>,
    token_sha256: &[u8],
    actor: &ActorRecord,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM ssot_token_bindings
             WHERE token_sha256 = $1 AND tenant_id = $2 AND actor_id = $3 AND active = TRUE",
            &[
                &token_sha256,
                &actor.tenant_id.as_str(),
                &actor.actor_id.as_str(),
            ],
        )
        .map_err(|error| storage("token_provision_read", error))?
        .get(0);
    ensure_exact_match(matched, "token_provision", "token binding conflicts")
}

fn ensure_active_tenant_project(
    tx: &mut postgres::Transaction<'_>,
    tenant_id: &TenantId,
    project_id: &ProjectId,
) -> Result<(), DomainError> {
    let matched: i64 = tx
        .query_one(
            "SELECT COUNT(*) FROM ssot_tenants AS tenant
             JOIN ssot_projects AS project ON project.tenant_id = tenant.tenant_id
             WHERE tenant.tenant_id = $1 AND project.project_id = $2
               AND tenant.status = 'active' AND project.status = 'active'",
            &[&tenant_id.as_str(), &project_id.as_str()],
        )
        .map_err(|error| storage("active_project_read", error))?
        .get(0);
    ensure_exact_match(
        matched,
        "active_project",
        "active tenant and project were not found",
    )
}

fn ensure_exact_match(
    matched: i64,
    operation: &'static str,
    detail: &'static str,
) -> Result<(), DomainError> {
    if matched == 1 {
        Ok(())
    } else {
        Err(DomainError::StorageFailure {
            operation,
            detail: detail.to_owned(),
        })
    }
}

fn to_i64(value: u64, field: &'static str) -> Result<i64, DomainError> {
    i64::try_from(value).map_err(|error| storage(field, error))
}

fn record_status_text(status: RecordStatus) -> &'static str {
    match status {
        RecordStatus::Active => "active",
        RecordStatus::Suspended => "suspended",
        RecordStatus::Archived => "archived",
    }
}

fn actor_kind_text(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Human => "human",
        ActorKind::Agent => "agent",
        ActorKind::Service => "service",
    }
}

fn membership_role_text(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::Admin => "admin",
        MembershipRole::Writer => "writer",
        MembershipRole::Reader => "reader",
    }
}

fn parse_membership_role(value: &str) -> Result<MembershipRole, DomainError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "writer" => Ok(MembershipRole::Writer),
        "reader" => Ok(MembershipRole::Reader),
        other => Err(decode_error("membership role", other)),
    }
}

fn membership_status_text(status: MembershipStatus) -> &'static str {
    match status {
        MembershipStatus::Active => "active",
        MembershipStatus::Suspended => "suspended",
    }
}

fn parse_membership_status(value: &str) -> Result<MembershipStatus, DomainError> {
    match value {
        "active" => Ok(MembershipStatus::Active),
        "suspended" => Ok(MembershipStatus::Suspended),
        other => Err(decode_error("membership status", other)),
    }
}

fn decode_error(kind: &'static str, value: &str) -> DomainError {
    DomainError::StorageFailure {
        operation: "record_decode",
        detail: format!("unknown {kind} '{value}'"),
    }
}
