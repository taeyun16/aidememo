//! Background worker for maintaining rebuildable lexical/semantic projections.
//!
//! Projections are derived state built from canonical snapshots. Each projection
//! tracks the exact project sequence (`index_seq`) it represents. The worker
//! periodically refreshes projections when new canonical data arrives, using the
//! bounded executor to avoid blocking Axum runtime workers.

use crate::{
    executor::BlockingStoreExecutor,
    lexical::LexicalProjection,
};
#[cfg(feature = "semantic")]
use crate::semantic::{EmbeddingProvider, SemanticProjection, SharedEmbeddingProvider};
use aidememo_domain::{DomainError, ProjectEpoch, ProjectScope, ProjectSequence, ProjectSnapshot};
use std::{
    sync::Arc,
    time::Duration,
};
use tokio::{
    sync::RwLock,
    time::{interval, MissedTickBehavior},
};

/// Minimum refresh interval to avoid excessive rebuild pressure.
const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Default refresh check interval.
const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(5);

/// Cached projection state with sequence watermark.
#[derive(Clone)]
pub(crate) struct CachedLexicalProjection {
    projection: Arc<LexicalProjection>,
    scope: ProjectScope,
}

impl CachedLexicalProjection {
    #[must_use]
    pub(crate) fn projection(&self) -> &Arc<LexicalProjection> {
        &self.projection
    }

    #[must_use]
    pub(crate) fn scope(&self) -> &ProjectScope {
        &self.scope
    }

    #[must_use]
    pub(crate) fn index_seq(&self) -> ProjectSequence {
        self.projection.index_seq()
    }

    #[must_use]
    pub(crate) fn project_epoch(&self) -> &ProjectEpoch {
        self.projection.project_epoch()
    }
}

#[cfg(feature = "semantic")]
#[derive(Clone)]
pub(crate) struct CachedSemanticProjection {
    projection: Arc<SemanticProjection>,
    scope: ProjectScope,
}

#[cfg(feature = "semantic")]
impl CachedSemanticProjection {
    #[must_use]
    pub(crate) fn projection(&self) -> &Arc<SemanticProjection> {
        &self.projection
    }

    #[must_use]
    pub(crate) fn scope(&self) -> &ProjectScope {
        &self.scope
    }

    #[must_use]
    pub(crate) fn index_seq(&self) -> ProjectSequence {
        self.projection.index_seq()
    }

    #[must_use]
    pub(crate) fn project_epoch(&self) -> &ProjectEpoch {
        self.projection.project_epoch()
    }
}

/// Worker state for maintaining projection indexes.
#[derive(Clone)]
pub(crate) struct ProjectionWorker {
    executor: BlockingStoreExecutor,
    lexical_cache: Arc<RwLock<Option<CachedLexicalProjection>>>,
    #[cfg(feature = "semantic")]
    semantic_cache: Arc<RwLock<Option<CachedSemanticProjection>>>,
    #[cfg(feature = "semantic")]
    semantic_provider: Option<SharedEmbeddingProvider>,
}

impl ProjectionWorker {
    /// Create a new projection worker for the given executor.
    #[must_use]
    pub(crate) fn new(
        executor: BlockingStoreExecutor,
        #[cfg(feature = "semantic")] semantic_provider: Option<SharedEmbeddingProvider>,
    ) -> Self {
        Self {
            executor,
            lexical_cache: Arc::new(RwLock::new(None)),
            #[cfg(feature = "semantic")]
            semantic_cache: Arc::new(RwLock::new(None)),
            #[cfg(feature = "semantic")]
            semantic_provider,
        }
    }

    /// Get the current cached lexical projection for a scope, if available and fresh.
    pub(crate) async fn get_lexical(
        &self,
        scope: &ProjectScope,
    ) -> Option<Arc<LexicalProjection>> {
        let cache = self.lexical_cache.read().await;
        cache
            .as_ref()
            .filter(|cached| cached.scope == *scope)
            .map(|cached| Arc::clone(&cached.projection))
    }

    #[cfg(feature = "semantic")]
    pub(crate) async fn get_semantic(
        &self,
        scope: &ProjectScope,
    ) -> Option<Arc<SemanticProjection>> {
        let cache = self.semantic_cache.read().await;
        cache
            .as_ref()
            .filter(|cached| cached.scope == *scope)
            .map(|cached| Arc::clone(&cached.projection))
    }

    /// Rebuild projections for a specific scope from a fresh snapshot.
    ///
    /// This operation runs on the bounded executor to avoid blocking Axum workers.
    pub(crate) async fn rebuild_for_scope(
        &self,
        scope: ProjectScope,
    ) -> Result<(), DomainError> {
        let scope_for_snapshot = scope.clone();
        let snapshot = self
            .executor
            .run(move |store| store.snapshot(&scope_for_snapshot))
            .await
            .map_err(|error| DomainError::StorageFailure {
                operation: "projection_worker_snapshot",
                detail: error.to_string(),
            })?;

        self.rebuild_from_snapshot(scope, snapshot).await?;
        Ok(())
    }

    /// Rebuild projections from an already-fetched snapshot.
    async fn rebuild_from_snapshot(
        &self,
        scope: ProjectScope,
        snapshot: ProjectSnapshot,
    ) -> Result<(), DomainError> {
        // Rebuild lexical on blocking pool
        let lexical_snapshot = snapshot.clone();
        let lexical_projection = tokio::task::spawn_blocking(move || {
            LexicalProjection::rebuild(&lexical_snapshot)
        })
        .await
        .map_err(|error| DomainError::StorageFailure {
            operation: "projection_worker_lexical_task",
            detail: error.to_string(),
        })??;

        {
            let mut cache = self.lexical_cache.write().await;
            *cache = Some(CachedLexicalProjection {
                projection: Arc::new(lexical_projection),
                scope: scope.clone(),
            });
        }

        #[cfg(feature = "semantic")]
        {
            if let Some(provider) = &self.semantic_provider {
                let semantic_snapshot = snapshot;
                let semantic_provider = Arc::clone(provider);
                let semantic_projection = tokio::task::spawn_blocking(move || {
                    SemanticProjection::rebuild(&semantic_snapshot, semantic_provider.as_ref())
                })
                .await
                .map_err(|error| DomainError::StorageFailure {
                    operation: "projection_worker_semantic_task",
                    detail: error.to_string(),
                })??;

                let mut cache = self.semantic_cache.write().await;
                *cache = Some(CachedSemanticProjection {
                    projection: Arc::new(semantic_projection),
                    scope,
                });
            }
        }

        Ok(())
    }

    /// Spawn a background task that periodically refreshes projections.
    ///
    /// The task checks for new canonical data and rebuilds projections when the
    /// project sequence advances. Refresh failures are logged but do not crash
    /// the task.
    pub(crate) fn spawn_refresh_task(
        self: Arc<Self>,
        scope: ProjectScope,
        refresh_interval: Option<Duration>,
    ) -> tokio::task::JoinHandle<()> {
        let interval_duration = refresh_interval
            .unwrap_or(DEFAULT_REFRESH_INTERVAL)
            .max(MIN_REFRESH_INTERVAL);

        tokio::spawn(async move {
            let mut ticker = interval(interval_duration);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

            // Initial bootstrap
            if let Err(error) = self.rebuild_for_scope(scope.clone()).await {
                tracing::warn!(
                    scope = ?scope,
                    error = %error,
                    "projection worker initial bootstrap failed"
                );
            }

            loop {
                ticker.tick().await;

                // Check if rebuild is needed by comparing sequences
                let should_rebuild = {
                    let cached = self.lexical_cache.read().await;
                    match cached.as_ref() {
                        Some(cache) if cache.scope == scope => {
                            // Query current canonical sequence
                            let check_scope = scope.clone();
                            match self
                                .executor
                                .run(move |store| {
                                    store.snapshot(&check_scope).map(|snap| snap.at_seq)
                                })
                                .await
                            {
                                Ok(canonical_seq) => canonical_seq > cache.index_seq(),
                                Err(error) => {
                                    tracing::debug!(
                                        error = %error,
                                        "projection worker sequence check failed"
                                    );
                                    false
                                }
                            }
                        }
                        _ => true, // No cache or wrong scope, rebuild
                    }
                };

                if should_rebuild {
                    if let Err(error) = self.rebuild_for_scope(scope.clone()).await {
                        tracing::warn!(
                            scope = ?scope,
                            error = %error,
                            "projection worker refresh failed"
                        );
                    } else {
                        let cached = self.lexical_cache.read().await;
                        if let Some(cache) = cached.as_ref() {
                            tracing::debug!(
                                scope = ?scope,
                                index_seq = cache.index_seq().get(),
                                "projection worker refreshed"
                            );
                        }
                    }
                }
            }
        })
    }
}
