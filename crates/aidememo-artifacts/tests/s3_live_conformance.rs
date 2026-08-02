#![cfg(feature = "s3")]

use aidememo_artifacts::{ArtifactStoreError, DirectBodyGrant, S3BodyStore, S3BodyStoreConfig};
use aidememo_domain::{
    ArtifactId, ArtifactPath, ArtifactReference, ArtifactReservation, ProjectId, ProjectScope,
    Revision, TenantId,
};
use reqwest::{Client, Method, StatusCode, header::HeaderMap};
use std::{
    env,
    error::Error,
    io,
    time::{Duration, SystemTime},
};
use ulid::Ulid;

const BODY: &[u8] = b"aidememo-s3-live-conformance";
const CONTENT_TYPE: &str = "application/octet-stream";
const GRANT_TTL_MS: i64 = 60_000;

struct LiveConfig {
    bucket: String,
    prefix: String,
    region: String,
    endpoint: String,
    force_path_style: bool,
}

impl LiveConfig {
    fn from_environment(run_id: &str) -> Result<Self, Box<dyn Error>> {
        required_env("AWS_ACCESS_KEY_ID")?;
        required_env("AWS_SECRET_ACCESS_KEY")?;
        let bucket = required_env("AIDEMEMO_S3_TEST_BUCKET")?;
        let base_prefix = env::var("AIDEMEMO_S3_TEST_PREFIX")
            .unwrap_or_else(|_| "aidememo/conformance".to_owned());
        let region = env::var("AIDEMEMO_S3_TEST_REGION").unwrap_or_else(|_| "auto".to_owned());
        let endpoint = required_env("AIDEMEMO_S3_TEST_ENDPOINT")?;
        let force_path_style = match env::var("AIDEMEMO_S3_TEST_FORCE_PATH_STYLE") {
            Ok(value) if matches!(value.as_str(), "1" | "true") => true,
            Ok(value) if matches!(value.as_str(), "0" | "false") => false,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "AIDEMEMO_S3_TEST_FORCE_PATH_STYLE must be true, false, 1, or 0",
                )
                .into());
            }
            Err(env::VarError::NotPresent) => false,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            bucket,
            prefix: format!("{}/{run_id}", base_prefix.trim_matches('/')),
            region,
            endpoint,
            force_path_style,
        })
    }
}

#[tokio::test]
#[ignore = "requires an explicitly configured disposable S3-compatible test bucket"]
async fn s3_provider_presigned_lifecycle_conforms() -> Result<(), Box<dyn Error>> {
    let run_id = Ulid::new().to_string().to_ascii_lowercase();
    let live = LiveConfig::from_environment(&run_id)?;
    let config = S3BodyStoreConfig::new(
        live.bucket,
        live.prefix,
        live.region,
        Some(live.endpoint),
        live.force_path_style,
    )?;
    let store = S3BodyStore::from_environment(config).await;
    let now_ms = unix_time_ms()?;
    let reservation = reservation(&run_id, now_ms)?;

    let result = exercise_lifecycle(&store, &reservation, now_ms).await;
    let cleanup = store
        .delete_generation(&reservation.scope, &reservation.generation)
        .await;
    match result {
        Err(error) => {
            let _ = cleanup;
            Err(error)
        }
        Ok(()) => {
            cleanup?;
            Ok(())
        }
    }
}

async fn exercise_lifecycle(
    store: &S3BodyStore,
    reservation: &ArtifactReservation,
    now_ms: i64,
) -> Result<(), Box<dyn Error>> {
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;
    let upload = store
        .presign_put(
            reservation,
            BODY.len() as u64,
            CONTENT_TYPE,
            now_ms,
            GRANT_TTL_MS,
        )
        .await?;
    let first_status = send_put(&client, &upload, BODY).await?;
    if !first_status.is_success() {
        return Err(provider_status("conditional presigned PUT", first_status));
    }
    let replay_status = send_put(&client, &upload, BODY).await?;
    if replay_status != StatusCode::PRECONDITION_FAILED {
        return Err(provider_status("immutable-key PUT replay", replay_status));
    }

    let observation = store.observe(reservation, BODY.len() as u64).await?;
    if observation.generation != reservation.generation || observation.digest.is_some() {
        return Err(
            io::Error::other("trusted provider observation changed generation or digest").into(),
        );
    }
    let reference = ArtifactReference {
        artifact_id: reservation.artifact_id.clone(),
        scope: reservation.scope.clone(),
        path: reservation.path.clone(),
        revision: reservation.revision,
        mutation_token: reservation.mutation_token.clone(),
        body: observation.body_ref()?,
    };
    let retained_until_ms = now_ms
        .checked_add(GRANT_TTL_MS)
        .ok_or_else(|| io::Error::other("retention time overflow"))?;
    let download = store
        .presign_get(&reference, now_ms, GRANT_TTL_MS, retained_until_ms)
        .await?;
    let downloaded = send_get(&client, &download).await?;
    if downloaded != BODY {
        return Err(io::Error::other("presigned GET returned different bytes").into());
    }
    if store.read(&reference, BODY.len()).await? != BODY {
        return Err(io::Error::other("exact SDK GET returned different bytes").into());
    }

    store
        .delete_generation(&reservation.scope, &reservation.generation)
        .await?;
    store
        .delete_generation(&reservation.scope, &reservation.generation)
        .await?;
    if !matches!(
        store.observe(reservation, BODY.len() as u64).await,
        Err(ArtifactStoreError::NotFound)
    ) {
        return Err(io::Error::other("deleted generation remained observable").into());
    }
    Ok(())
}

async fn send_put(
    client: &Client,
    grant: &DirectBodyGrant,
    body: &[u8],
) -> Result<StatusCode, Box<dyn Error>> {
    if grant.method != "PUT" {
        return Err(io::Error::other("upload grant did not select PUT").into());
    }
    let response = client
        .request(Method::PUT, &grant.url)
        .headers(grant_headers(grant)?)
        .body(body.to_vec())
        .send()
        .await
        .map_err(|_| io::Error::other("presigned PUT request failed"))?;
    Ok(response.status())
}

async fn send_get(client: &Client, grant: &DirectBodyGrant) -> Result<Vec<u8>, Box<dyn Error>> {
    if grant.method != "GET" {
        return Err(io::Error::other("download grant did not select GET").into());
    }
    let response = client
        .request(Method::GET, &grant.url)
        .headers(grant_headers(grant)?)
        .send()
        .await
        .map_err(|_| io::Error::other("presigned GET request failed"))?;
    if !response.status().is_success() {
        return Err(provider_status("presigned GET", response.status()));
    }
    Ok(response
        .bytes()
        .await
        .map_err(|_| io::Error::other("presigned GET body read failed"))?
        .to_vec())
}

fn grant_headers(grant: &DirectBodyGrant) -> Result<HeaderMap, Box<dyn Error>> {
    let mut headers = HeaderMap::new();
    for (name, value) in &grant.headers {
        headers.insert(
            reqwest::header::HeaderName::from_bytes(name.as_bytes())?,
            reqwest::header::HeaderValue::from_str(value)?,
        );
    }
    Ok(headers)
}

fn reservation(run_id: &str, now_ms: i64) -> Result<ArtifactReservation, Box<dyn Error>> {
    Ok(ArtifactReservation {
        artifact_id: ArtifactId::try_from(format!("artifact_{run_id}"))?,
        scope: ProjectScope::new(
            TenantId::try_from("tenant_conformance")?,
            ProjectId::try_from("project_conformance")?,
        ),
        path: ArtifactPath::try_from(format!("/conformance/{run_id}.bin"))?,
        revision: Revision::new(1)?,
        mutation_token: format!("mutation_{run_id}"),
        generation: format!("generation_{run_id}"),
        expires_at_ms: now_ms
            .checked_add(GRANT_TTL_MS)
            .ok_or_else(|| io::Error::other("reservation expiry overflow"))?,
    })
}

fn required_env(name: &str) -> Result<String, Box<dyn Error>> {
    let value = env::var(name).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("{name} is required for live S3 conformance"),
        )
    })?;
    if value.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must not be empty"),
        )
        .into());
    }
    Ok(value)
}

fn provider_status(operation: &str, status: StatusCode) -> Box<dyn Error> {
    io::Error::other(format!(
        "{operation} returned unexpected provider status {}",
        status.as_u16()
    ))
    .into()
}

fn unix_time_ms() -> Result<i64, Box<dyn Error>> {
    let duration = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?;
    Ok(i64::try_from(duration.as_millis())?)
}
