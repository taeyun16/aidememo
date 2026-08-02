//! S3-compatible immutable-body and direct-transfer adapter.
//!
//! The adapter deliberately owns only random-generation object keys. Logical
//! path authorization and publication CAS remain in the metadata coordinator.

use super::{ArtifactStoreError, digest_bytes};
use aidememo_domain::{
    ArtifactBodyRef, ArtifactObservation, ArtifactReference, ArtifactReservation, ProjectScope,
};
use aws_sdk_s3::{Client, config::Region, presigning::PresigningConfig};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fmt,
    time::{Duration, UNIX_EPOCH},
};

const MAX_PRESIGN_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_S3_KEY_BYTES: usize = 1_024;
const MAX_CONTENT_TYPE_BYTES: usize = 256;
const GENERATION_METADATA_KEY: &str = "aidememo-generation";

/// Validated S3-compatible bucket, prefix, endpoint, and signing region.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S3BodyStoreConfig {
    bucket: String,
    prefix: String,
    region: String,
    endpoint_url: Option<String>,
    force_path_style: bool,
}

impl S3BodyStoreConfig {
    /// Build a provider-neutral S3 configuration.
    ///
    /// Use region `auto` and the account S3 endpoint for Cloudflare R2. Set
    /// `force_path_style` for providers such as local MinIO when required.
    ///
    /// # Errors
    ///
    /// Returns a validation error for empty or unsafe bucket/prefix/region/
    /// endpoint values.
    pub fn new(
        bucket: impl Into<String>,
        prefix: impl Into<String>,
        region: impl Into<String>,
        endpoint_url: Option<String>,
        force_path_style: bool,
    ) -> Result<Self, ArtifactStoreError> {
        let bucket = bucket.into();
        let prefix = prefix.into().trim_matches('/').to_owned();
        let region = region.into();
        validate_component("bucket", &bucket, 255)?;
        validate_component("region", &region, 128)?;
        if !prefix.is_empty() {
            validate_key(&prefix)?;
        }
        if let Some(endpoint) = &endpoint_url {
            let authority = endpoint
                .strip_prefix("https://")
                .or_else(|| endpoint.strip_prefix("http://"))
                .and_then(|rest| rest.split('/').next());
            if endpoint.len() > 2_048
                || authority.is_none_or(|value| value.is_empty() || value.contains('@'))
                || endpoint.contains(['?', '#'])
                || endpoint
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
            {
                return Err(invalid(
                    "S3 endpoint must be a bounded HTTP(S) URL without credentials, query, or fragment",
                ));
            }
        }
        Ok(Self {
            bucket,
            prefix,
            region,
            endpoint_url,
            force_path_style,
        })
    }

    /// Configured bucket name.
    #[must_use]
    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Adapter-owned object prefix without leading or trailing slash.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// Signing region (`auto` for R2).
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Optional S3-compatible endpoint.
    #[must_use]
    pub fn endpoint_url(&self) -> Option<&str> {
        self.endpoint_url.as_deref()
    }

    /// Whether the SDK must use path-style bucket addressing.
    #[must_use]
    pub fn force_path_style(&self) -> bool {
        self.force_path_style
    }
}

/// Short-lived bearer capability for one exact body-store operation.
#[derive(Clone, Eq, PartialEq, Serialize)]
pub struct DirectBodyGrant {
    /// HTTP method signed into the capability.
    pub method: String,
    /// Presigned S3 API URL. Treat as a bearer credential.
    pub url: String,
    /// Required signed headers excluding `Host`.
    pub headers: BTreeMap<String, String>,
    /// Exact adapter-owned immutable object key.
    pub object_key: String,
    /// Server-selected capability expiry in Unix milliseconds.
    pub expires_at_ms: i64,
}

impl fmt::Debug for DirectBodyGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DirectBodyGrant")
            .field("method", &self.method)
            .field("url", &"<redacted bearer capability>")
            .field("header_names", &self.headers.keys().collect::<Vec<_>>())
            .field("object_key", &self.object_key)
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

/// S3-compatible body-store client for AWS S3, Cloudflare R2, and MinIO.
#[derive(Clone)]
pub struct S3BodyStore {
    client: Client,
    config: S3BodyStoreConfig,
}

impl S3BodyStore {
    /// Wrap an explicitly configured SDK client.
    #[must_use]
    pub fn from_client(client: Client, config: S3BodyStoreConfig) -> Self {
        Self { client, config }
    }

    /// Load credentials from the standard AWS provider chain and apply the
    /// validated S3-compatible endpoint/region configuration.
    pub async fn from_environment(config: S3BodyStoreConfig) -> Self {
        let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(Region::new(config.region.clone()))
            .load()
            .await;
        let mut builder = aws_sdk_s3::config::Builder::from(&shared)
            .region(Region::new(config.region.clone()))
            .force_path_style(config.force_path_style);
        if let Some(endpoint) = &config.endpoint_url {
            builder = builder.endpoint_url(endpoint);
        }
        Self::from_client(Client::from_conf(builder.build()), config)
    }

    /// Return the exact provider object key for one reservation.
    ///
    /// # Errors
    ///
    /// Returns a validation error if the reservation or resulting S3 key is invalid.
    pub fn object_key(
        &self,
        reservation: &ArtifactReservation,
    ) -> Result<String, ArtifactStoreError> {
        reservation.validate()?;
        self.generation_key(&reservation.scope, &reservation.generation)
    }

    /// Sign a conditional single-part upload for one immutable generation.
    ///
    /// The signed request includes `If-None-Match: *`, exact content length,
    /// content type, and generation metadata. The grant never outlives the
    /// metadata reservation.
    ///
    /// # Errors
    ///
    /// Returns validation or provider signing errors.
    pub async fn presign_put(
        &self,
        reservation: &ArtifactReservation,
        content_length: u64,
        content_type: &str,
        now_ms: i64,
        ttl_ms: i64,
    ) -> Result<DirectBodyGrant, ArtifactStoreError> {
        reservation.validate()?;
        validate_content_type(content_type)?;
        let expires_at_ms = validate_grant_window(now_ms, ttl_ms, Some(reservation.expires_at_ms))?;
        let content_length =
            i64::try_from(content_length).map_err(|_| invalid("S3 content length exceeds i64"))?;
        let object_key = self.object_key(reservation)?;
        let presigning = presigning_config(now_ms, ttl_ms)?;
        let request = self
            .client
            .put_object()
            .bucket(&self.config.bucket)
            .key(&object_key)
            .content_length(content_length)
            .content_type(content_type)
            .metadata(GENERATION_METADATA_KEY, &reservation.generation)
            .if_none_match("*")
            .presigned(presigning)
            .await
            .map_err(|_| provider("presign_put", None))?;
        let grant = grant_from_request(request, object_key, expires_at_ms)?;
        let expected_content_length = content_length.to_string();
        if grant.method != "PUT"
            || grant.headers.get("if-none-match").map(String::as_str) != Some("*")
            || grant.headers.get("content-length").map(String::as_str)
                != Some(expected_content_length.as_str())
            || grant.headers.get("content-type").map(String::as_str) != Some(content_type)
            || grant
                .headers
                .get("x-amz-meta-aidememo-generation")
                .map(String::as_str)
                != Some(reservation.generation.as_str())
        {
            return Err(provider(
                "presign_put",
                Some("required upload constraint was not signed"),
            ));
        }
        Ok(grant)
    }

    /// Observe provider-owned metadata for a completed upload.
    ///
    /// # Errors
    ///
    /// Returns not-found, body-mismatch, validation, or sanitized provider errors.
    pub async fn observe(
        &self,
        reservation: &ArtifactReservation,
        expected_size: u64,
    ) -> Result<ArtifactObservation, ArtifactStoreError> {
        let object_key = self.object_key(reservation)?;
        let output = self
            .client
            .head_object()
            .bucket(&self.config.bucket)
            .key(&object_key)
            .send()
            .await
            .map_err(|error| {
                match error
                    .raw_response()
                    .map(|response| response.status().as_u16())
                {
                    Some(404) => ArtifactStoreError::NotFound,
                    status => provider("head", status.map(|value| value.to_string()).as_deref()),
                }
            })?;
        let size = output
            .content_length()
            .ok_or(ArtifactStoreError::BodyMismatch)
            .and_then(|value| u64::try_from(value).map_err(|_| ArtifactStoreError::BodyMismatch))?;
        let generation = output
            .metadata()
            .and_then(|metadata| metadata.get(GENERATION_METADATA_KEY))
            .ok_or(ArtifactStoreError::BodyMismatch)?;
        if size != expected_size || generation != &reservation.generation {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let etag = normalize_etag(output.e_tag().ok_or(ArtifactStoreError::BodyMismatch)?)?;
        let version = output
            .version_id()
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let observation = ArtifactObservation {
            object_key,
            generation: reservation.generation.clone(),
            size_bytes: size,
            etag,
            version,
            digest: None,
        };
        observation.body_ref()?;
        Ok(observation)
    }

    /// Sign an exact-generation GET using the published ETag and optional version.
    /// The caller must first persist read retention through `retained_until_ms`;
    /// the grant is rejected when it would outlive that durable boundary.
    ///
    /// # Errors
    ///
    /// Returns validation, body-mismatch, or provider signing errors.
    pub async fn presign_get(
        &self,
        reference: &ArtifactReference,
        now_ms: i64,
        ttl_ms: i64,
        retained_until_ms: i64,
    ) -> Result<DirectBodyGrant, ArtifactStoreError> {
        reference.validate()?;
        let expires_at_ms = validate_grant_window(now_ms, ttl_ms, None)?;
        if retained_until_ms < expires_at_ms {
            return Err(invalid(
                "download grant must not outlive durable read retention",
            ));
        }
        let (object_key, generation, etag, version) = object_fields(reference)?;
        let expected_key = self.generation_key(&reference.scope, generation)?;
        if object_key != expected_key {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let mut builder = self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(object_key)
            .if_match(etag);
        if let Some(version) = version {
            builder = builder.version_id(version);
        }
        let request = builder
            .presigned(presigning_config(now_ms, ttl_ms)?)
            .await
            .map_err(|_| provider("presign_get", None))?;
        let grant = grant_from_request(request, object_key.to_owned(), expires_at_ms)?;
        if grant.method != "GET" || grant.headers.get("if-match").map(String::as_str) != Some(etag)
        {
            return Err(provider(
                "presign_get",
                Some("required exact-generation condition was not signed"),
            ));
        }
        Ok(grant)
    }

    /// Read and verify one exact published object through the S3 API.
    ///
    /// This bounded helper is for conformance and controlled server paths;
    /// hosted large downloads should use [`Self::presign_get`].
    ///
    /// # Errors
    ///
    /// Returns not-found, body-mismatch, or sanitized provider errors.
    pub async fn read(
        &self,
        reference: &ArtifactReference,
        max_bytes: usize,
    ) -> Result<Vec<u8>, ArtifactStoreError> {
        reference.validate()?;
        let (object_key, generation, etag, version) = object_fields(reference)?;
        let ArtifactBodyRef::Object {
            size_bytes, digest, ..
        } = &reference.body
        else {
            return Err(ArtifactStoreError::BodyMismatch);
        };
        if max_bytes == 0 || *size_bytes > max_bytes as u64 {
            return Err(invalid("exact S3 read exceeds its caller-provided bound"));
        }
        if object_key != self.generation_key(&reference.scope, generation)? {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let last_byte = max_bytes.saturating_sub(1);
        let mut builder = self
            .client
            .get_object()
            .bucket(&self.config.bucket)
            .key(object_key)
            .if_match(etag)
            .range(format!("bytes=0-{last_byte}"));
        if let Some(version) = version {
            builder = builder.version_id(version);
        }
        let output = builder.send().await.map_err(|error| {
            match error
                .raw_response()
                .map(|response| response.status().as_u16())
            {
                Some(404) => ArtifactStoreError::NotFound,
                Some(412) => ArtifactStoreError::BodyMismatch,
                status => provider("get", status.map(|value| value.to_string()).as_deref()),
            }
        })?;
        let bytes = output
            .body
            .collect()
            .await
            .map_err(|_| provider("read_body", None))?
            .into_bytes()
            .to_vec();
        if bytes.len() as u64 != *size_bytes {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        if let Some(expected) = digest
            && digest_bytes(&bytes)? != *expected
        {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        Ok(bytes)
    }

    /// Idempotently delete one immutable random-generation key.
    ///
    /// R2 does not document conditional `DeleteObject` headers. Exactness comes
    /// from the coordinator invariant that generation keys are never reused;
    /// an available provider version ID is still included.
    ///
    /// # Errors
    ///
    /// Returns validation, body-mismatch, or sanitized provider errors.
    pub async fn delete(&self, reference: &ArtifactReference) -> Result<(), ArtifactStoreError> {
        reference.validate()?;
        let (object_key, generation, _, version) = object_fields(reference)?;
        if object_key != self.generation_key(&reference.scope, generation)? {
            return Err(ArtifactStoreError::BodyMismatch);
        }
        let mut builder = self
            .client
            .delete_object()
            .bucket(&self.config.bucket)
            .key(object_key);
        if let Some(version) = version {
            builder = builder.version_id(version);
        }
        builder.send().await.map_err(|error| {
            provider(
                "delete",
                error
                    .raw_response()
                    .map(|response| response.status().as_u16())
                    .map(|value| value.to_string())
                    .as_deref(),
            )
        })?;
        Ok(())
    }

    fn generation_key(
        &self,
        scope: &ProjectScope,
        generation: &str,
    ) -> Result<String, ArtifactStoreError> {
        validate_component("generation", generation, 256)?;
        let relative = format!(
            "objects/{}/{}/{}.blob",
            scope.tenant_id.as_str(),
            scope.project_id.as_str(),
            generation
        );
        let key = if self.config.prefix.is_empty() {
            relative
        } else {
            format!("{}/{relative}", self.config.prefix)
        };
        validate_key(&key)?;
        Ok(key)
    }
}

fn grant_from_request(
    request: aws_sdk_s3::presigning::PresignedRequest,
    object_key: String,
    expires_at_ms: i64,
) -> Result<DirectBodyGrant, ArtifactStoreError> {
    let mut headers = BTreeMap::new();
    for (name, value) in request.headers() {
        if headers
            .insert(name.to_ascii_lowercase(), value.to_owned())
            .is_some()
        {
            return Err(provider("presign", Some("duplicate signed header")));
        }
    }
    Ok(DirectBodyGrant {
        method: request.method().to_owned(),
        url: request.uri().to_owned(),
        headers,
        object_key,
        expires_at_ms,
    })
}

fn object_fields(
    reference: &ArtifactReference,
) -> Result<(&str, &str, &str, Option<&str>), ArtifactStoreError> {
    let ArtifactBodyRef::Object {
        object_key,
        generation,
        etag,
        version,
        ..
    } = &reference.body
    else {
        return Err(ArtifactStoreError::BodyMismatch);
    };
    Ok((object_key, generation, etag, version.as_deref()))
}

fn presigning_config(now_ms: i64, ttl_ms: i64) -> Result<PresigningConfig, ArtifactStoreError> {
    let start = UNIX_EPOCH
        .checked_add(Duration::from_millis(
            u64::try_from(now_ms).map_err(|_| invalid("grant time must be positive"))?,
        ))
        .ok_or_else(|| invalid("grant start time overflow"))?;
    PresigningConfig::builder()
        .start_time(start)
        .expires_in(Duration::from_millis(
            u64::try_from(ttl_ms).map_err(|_| invalid("grant TTL must be positive"))?,
        ))
        .build()
        .map_err(|_| invalid("grant TTL exceeds provider presigning limit"))
}

fn validate_grant_window(
    now_ms: i64,
    ttl_ms: i64,
    reservation_expiry: Option<i64>,
) -> Result<i64, ArtifactStoreError> {
    if now_ms <= 0 || ttl_ms <= 0 || ttl_ms > MAX_PRESIGN_TTL_MS {
        return Err(invalid(
            "grant time must be positive and TTL must not exceed seven days",
        ));
    }
    let expires_at_ms = now_ms
        .checked_add(ttl_ms)
        .ok_or_else(|| invalid("grant expiry overflow"))?;
    if reservation_expiry.is_some_and(|expiry| expires_at_ms > expiry) {
        return Err(invalid("upload grant must not outlive its reservation"));
    }
    Ok(expires_at_ms)
}

fn validate_content_type(content_type: &str) -> Result<(), ArtifactStoreError> {
    if content_type.is_empty()
        || content_type.len() > MAX_CONTENT_TYPE_BYTES
        || content_type.trim() != content_type
        || content_type.chars().any(char::is_control)
        || !content_type.contains('/')
    {
        return Err(invalid("content type must be a bounded MIME type"));
    }
    Ok(())
}

fn validate_component(name: &str, value: &str, max_bytes: usize) -> Result<(), ArtifactStoreError> {
    if value.is_empty()
        || value.len() > max_bytes
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(invalid(&format!("S3 {name} is empty or unsafe")));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), ArtifactStoreError> {
    if key.is_empty()
        || key.len() > MAX_S3_KEY_BYTES
        || key.starts_with('/')
        || key.ends_with('/')
        || key.contains('\\')
        || key
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
        || key.chars().any(char::is_control)
    {
        return Err(invalid("S3 object key is empty, oversized, or unsafe"));
    }
    Ok(())
}

fn normalize_etag(value: &str) -> Result<String, ArtifactStoreError> {
    let normalized = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value);
    if normalized.is_empty() || normalized.len() > 1_024 || normalized.chars().any(char::is_control)
    {
        return Err(ArtifactStoreError::BodyMismatch);
    }
    Ok(normalized.to_owned())
}

fn invalid(detail: &str) -> ArtifactStoreError {
    aidememo_domain::DomainError::InvalidArtifactReference(detail.to_owned()).into()
}

fn provider(operation: &'static str, detail: Option<&str>) -> ArtifactStoreError {
    ArtifactStoreError::Provider {
        operation,
        detail: detail.unwrap_or("provider request failed").to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aidememo_domain::{ArtifactId, ArtifactPath, ContentDigest, ProjectId, Revision, TenantId};
    use aws_credential_types::Credentials;
    use axum::{
        Router,
        body::Body,
        extract::{Request, State},
        http::{HeaderMap, HeaderValue, Method, StatusCode, header},
        response::{IntoResponse, Response},
        routing::any,
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;

    const NOW_MS: i64 = 1_700_000_000_000;

    fn store() -> Result<S3BodyStore, ArtifactStoreError> {
        let config = S3BodyStoreConfig::new(
            "artifacts",
            "aidememo/v1",
            "auto",
            Some("https://account.r2.cloudflarestorage.com".to_owned()),
            true,
        )?;
        let sdk = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .credentials_provider(Credentials::for_tests())
            .region(Region::new("auto"))
            .endpoint_url("https://account.r2.cloudflarestorage.com")
            .force_path_style(true)
            .build();
        Ok(S3BodyStore::from_client(Client::from_conf(sdk), config))
    }

    fn reservation() -> Result<ArtifactReservation, ArtifactStoreError> {
        Ok(ArtifactReservation {
            artifact_id: ArtifactId::try_from("artifact_s3")?,
            scope: ProjectScope::new(
                TenantId::try_from("tenant_s3")?,
                ProjectId::try_from("project_s3")?,
            ),
            path: ArtifactPath::try_from("/sessions/s3/result.bin")?,
            revision: Revision::new(1)?,
            mutation_token: "mut_s3".to_owned(),
            generation: "gen_s3".to_owned(),
            expires_at_ms: NOW_MS + 60_000,
        })
    }

    #[tokio::test]
    async fn conditional_put_grant_is_exact_bounded_and_redacted()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = store()?;
        let reservation = reservation()?;
        let grant = store
            .presign_put(&reservation, 12, "application/octet-stream", NOW_MS, 30_000)
            .await?;
        assert_eq!(grant.method, "PUT");
        assert_eq!(
            grant.headers.get("if-none-match").map(String::as_str),
            Some("*")
        );
        assert_eq!(
            grant.headers.get("content-length").map(String::as_str),
            Some("12")
        );
        assert_eq!(
            grant
                .headers
                .get("x-amz-meta-aidememo-generation")
                .map(String::as_str),
            Some("gen_s3")
        );
        assert!(grant.url.contains("X-Amz-Signature="));
        assert!(
            grant
                .object_key
                .ends_with("objects/tenant_s3/project_s3/gen_s3.blob")
        );
        assert_eq!(grant.expires_at_ms, NOW_MS + 30_000);
        let debug = format!("{grant:?}");
        assert!(debug.contains("redacted bearer capability"));
        assert!(!debug.contains("X-Amz-Signature="));
        Ok(())
    }

    #[tokio::test]
    async fn grants_cannot_outlive_reservation_or_escape_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = store()?;
        let reservation = reservation()?;
        assert!(matches!(
            store
                .presign_put(&reservation, 1, "application/octet-stream", NOW_MS, 60_001,)
                .await,
            Err(ArtifactStoreError::Domain(_))
        ));
        assert!(S3BodyStoreConfig::new("artifacts", "../escape", "auto", None, false,).is_err());
        assert!(
            S3BodyStoreConfig::new(
                "artifacts",
                "safe",
                "auto",
                Some("https://user:secret@example.com".to_owned()),
                false,
            )
            .is_err()
        );
        Ok(())
    }

    #[tokio::test]
    async fn exact_get_grant_binds_etag_and_version() -> Result<(), Box<dyn std::error::Error>> {
        let store = store()?;
        let reservation = reservation()?;
        let object_key = store.object_key(&reservation)?;
        let reference = ArtifactReference {
            artifact_id: reservation.artifact_id.clone(),
            scope: reservation.scope.clone(),
            path: reservation.path.clone(),
            revision: reservation.revision,
            mutation_token: reservation.mutation_token.clone(),
            body: ArtifactBodyRef::Object {
                object_key,
                generation: reservation.generation,
                size_bytes: 12,
                etag: "etag-s3".to_owned(),
                version: Some("version-s3".to_owned()),
                digest: Some(ContentDigest::try_from("a".repeat(64))?),
            },
        };
        assert!(
            store
                .presign_get(&reference, NOW_MS, 30_000, NOW_MS + 29_999)
                .await
                .is_err()
        );
        let grant = store
            .presign_get(&reference, NOW_MS, 30_000, NOW_MS + 30_000)
            .await?;
        assert_eq!(grant.method, "GET");
        assert_eq!(
            grant.headers.get("if-match").map(String::as_str),
            Some("etag-s3")
        );
        assert!(grant.url.contains("versionId=version-s3"));
        Ok(())
    }

    #[derive(Clone, Default)]
    struct MockS3State {
        requests: Arc<Mutex<Vec<(Method, String, HeaderMap)>>>,
    }

    async fn mock_s3(State(state): State<MockS3State>, request: Request) -> Response {
        let method = request.method().clone();
        let uri = request.uri().to_string();
        let headers = request.headers().clone();
        state
            .requests
            .lock()
            .await
            .push((method.clone(), uri, headers));
        match method {
            Method::HEAD => {
                let mut headers = HeaderMap::new();
                headers.insert(header::CONTENT_LENGTH, HeaderValue::from_static("12"));
                headers.insert(header::ETAG, HeaderValue::from_static("\"etag-s3\""));
                headers.insert(
                    "x-amz-meta-aidememo-generation",
                    HeaderValue::from_static("gen_s3"),
                );
                headers.insert("x-amz-version-id", HeaderValue::from_static("version-s3"));
                (StatusCode::OK, headers, Body::empty()).into_response()
            }
            Method::GET => (
                StatusCode::OK,
                [(header::CONTENT_LENGTH, HeaderValue::from_static("12"))],
                Body::from(&b"artifact-s3!"[..]),
            )
                .into_response(),
            Method::DELETE => StatusCode::NO_CONTENT.into_response(),
            _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
        }
    }

    #[tokio::test]
    async fn head_read_and_delete_follow_exact_generation_wire_contract()
    -> Result<(), Box<dyn std::error::Error>> {
        let state = MockS3State::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let app = Router::new()
            .route("/{*path}", any(mock_s3))
            .with_state(state.clone());
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let config = S3BodyStoreConfig::new(
            "artifacts",
            "aidememo/v1",
            "us-east-1",
            Some(format!("http://{address}")),
            true,
        )?;
        let sdk = aws_sdk_s3::Config::builder()
            .behavior_version(aws_sdk_s3::config::BehaviorVersion::latest())
            .credentials_provider(Credentials::for_tests())
            .region(Region::new("us-east-1"))
            .endpoint_url(format!("http://{address}"))
            .force_path_style(true)
            .build();
        let store = S3BodyStore::from_client(Client::from_conf(sdk), config);
        let reservation = reservation()?;
        assert!(matches!(
            store.observe(&reservation, 11).await,
            Err(ArtifactStoreError::BodyMismatch)
        ));
        let observation = store.observe(&reservation, 12).await?;
        assert_eq!(observation.etag, "etag-s3");
        assert_eq!(observation.version.as_deref(), Some("version-s3"));
        assert!(observation.digest.is_none());

        let reference = ArtifactReference {
            artifact_id: reservation.artifact_id,
            scope: reservation.scope,
            path: reservation.path,
            revision: reservation.revision,
            mutation_token: reservation.mutation_token,
            body: observation.body_ref()?,
        };
        assert!(store.read(&reference, 11).await.is_err());
        assert_eq!(store.read(&reference, 12).await?, b"artifact-s3!");
        store.delete(&reference).await?;

        let requests = state.requests.lock().await;
        let get = requests
            .iter()
            .find(|(method, _, _)| method == Method::GET)
            .ok_or_else(|| std::io::Error::other("missing exact GET request"))?;
        assert_eq!(
            get.2.get("if-match").and_then(|value| value.to_str().ok()),
            Some("etag-s3")
        );
        assert!(get.1.contains("versionId=version-s3"));
        let delete = requests
            .iter()
            .find(|(method, _, _)| method == Method::DELETE)
            .ok_or_else(|| std::io::Error::other("missing exact DELETE request"))?;
        assert!(delete.1.contains("versionId=version-s3"));
        drop(requests);
        server.abort();
        let _ = server.await;
        Ok(())
    }
}
