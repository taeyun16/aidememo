from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    return text.replace(old, new)


path = Path('crates/aidememo-server/src/lib.rs')
text = path.read_text()
text = replace_once(
    text,
    'mod lexical;\nmod product;',
    '#[cfg(feature = "semantic")]\nmod semantic;\nmod lexical;\nmod product;',
    'semantic module',
)
text = replace_once(
    text,
    'use std::sync::Arc;',
    'use std::{collections::HashMap, sync::Arc};\n\n#[cfg(feature = "semantic")]\npub use semantic::{EmbeddingProvider, HttpEmbeddingProvider, SharedEmbeddingProvider};',
    'semantic exports',
)
text = replace_once(
    text,
    '''pub struct ServerState {\n    service: Arc<Mutex<CommandService<SqliteCommandStore>>>,\n    artifacts: Option<Arc<ArtifactState>>,\n}''',
    '''pub struct ServerState {\n    service: Arc<Mutex<CommandService<SqliteCommandStore>>>,\n    artifacts: Option<Arc<ArtifactState>>,\n    #[cfg(feature = "semantic")]\n    semantic_provider: Option<SharedEmbeddingProvider>,\n    #[cfg(feature = "semantic")]\n    semantic_projection: Arc<Mutex<Option<Arc<semantic::SemanticProjection>>>>,\n}''',
    'server state fields',
)
text = text.replace(
    '''            service: Arc::new(Mutex::new(CommandService::new(store))),\n            artifacts: None,\n        }''',
    '''            service: Arc::new(Mutex::new(CommandService::new(store))),\n            artifacts: None,\n            #[cfg(feature = "semantic")]\n            semantic_provider: None,\n            #[cfg(feature = "semantic")]\n            semantic_projection: Arc::new(Mutex::new(None)),\n        }''',
    1,
)
text = text.replace(
    '''            artifacts: Some(Arc::new(ArtifactState {\n                catalog: Mutex::new(artifacts),\n                bodies: ArtifactBodies::Local,\n            })),\n        })''',
    '''            artifacts: Some(Arc::new(ArtifactState {\n                catalog: Mutex::new(artifacts),\n                bodies: ArtifactBodies::Local,\n            })),\n            #[cfg(feature = "semantic")]\n            semantic_provider: None,\n            #[cfg(feature = "semantic")]\n            semantic_projection: Arc::new(Mutex::new(None)),\n        })''',
    1,
)
text = text.replace(
    '''            artifacts: Some(Arc::new(ArtifactState {\n                catalog: Mutex::new(catalog),\n                bodies: ArtifactBodies::S3(bodies),\n            })),\n        })''',
    '''            artifacts: Some(Arc::new(ArtifactState {\n                catalog: Mutex::new(catalog),\n                bodies: ArtifactBodies::S3(bodies),\n            })),\n            semantic_provider: None,\n            semantic_projection: Arc::new(Mutex::new(None)),\n        })''',
    1,
)
method_anchor = '''    /// Run one bounded artifact garbage-collection pass when artifacts are configured.\n'''
method = '''    /// Attach a semantic embedding provider to this server state.\n    ///\n    /// The provider owns no canonical data; cached HNSW state is rebuilt from\n    /// canonical project snapshots whenever sequence or model identity changes.\n    #[cfg(feature = "semantic")]\n    #[must_use]\n    pub fn with_semantic_provider(mut self, provider: SharedEmbeddingProvider) -> Self {\n        self.semantic_provider = Some(provider);\n        self\n    }\n\n'''
text = replace_once(text, method_anchor, method + method_anchor, 'semantic state method')
text = replace_once(
    text,
    '''struct SearchQuery {\n    q: String,\n    source_id: Option<aidememo_domain::SourceId>,\n    limit: Option<usize>,\n    at_least_seq: Option<u64>,\n}\n\n#[derive(Serialize)]\nstruct SearchResponse {\n    project_epoch: ProjectEpoch,\n    index_seq: ProjectSequence,\n    results: Vec<lexical::LexicalHit>,\n}''',
    '''struct SearchQuery {\n    q: String,\n    source_id: Option<aidememo_domain::SourceId>,\n    limit: Option<usize>,\n    at_least_seq: Option<u64>,\n    mode: Option<String>,\n}\n\n#[derive(Clone, Serialize)]\nstruct SearchHit {\n    fact_id: aidememo_domain::FactId,\n    session_id: aidememo_domain::SessionId,\n    source_id: Option<aidememo_domain::SourceId>,\n    actor_id: aidememo_domain::ActorId,\n    content: String,\n    score: f64,\n    #[serde(skip_serializing_if = "Option::is_none")]\n    lexical_score: Option<f64>,\n    #[serde(skip_serializing_if = "Option::is_none")]\n    semantic_score: Option<f64>,\n}\n\n#[derive(Serialize)]\nstruct SearchResponse {\n    project_epoch: ProjectEpoch,\n    index_seq: ProjectSequence,\n    mode: &'static str,\n    semantic_model: Option<String>,\n    results: Vec<SearchHit>,\n}''',
    'search types',
)
old_search = '''    let projection = lexical::LexicalProjection::rebuild(&snapshot)?;\n    let results = projection.search(\n        query_text,\n        query.source_id.as_ref().map(|source_id| source_id.as_str()),\n        limit,\n    );\n    Ok((\n        StatusCode::OK,\n        Json(SearchResponse {\n            project_epoch: projection.project_epoch().clone(),\n            index_seq: projection.index_seq(),\n            results,\n        }),\n    ))\n}'''
new_search = '''    let requested_mode = query\n        .mode\n        .as_deref()\n        .unwrap_or("auto")\n        .trim()\n        .to_ascii_lowercase();\n    if !matches!(requested_mode.as_str(), "auto" | "lexical" | "semantic" | "hybrid") {\n        return Err(ApiError(DomainError::InvalidCommand(\n            "search mode must be auto, lexical, semantic, or hybrid".to_owned(),\n        )));\n    }\n    let projection = lexical::LexicalProjection::rebuild(&snapshot)?;\n    let candidate_limit = limit.saturating_mul(8).max(limit).min(512);\n    let lexical_hits = projection.search(\n        query_text,\n        query.source_id.as_ref().map(|source_id| source_id.as_str()),\n        candidate_limit,\n    );\n    let lexical_strong = lexical_hits.len() >= limit.min(3)\n        && lexical_hits.first().is_some_and(|hit| hit.score >= 1.25);\n\n    #[cfg(feature = "semantic")]\n    let (effective_mode, semantic_model, mut results) = {\n        let wants_semantic = match requested_mode.as_str() {\n            "lexical" => false,\n            "auto" => !lexical_strong,\n            "semantic" | "hybrid" => true,\n            _ => unreachable!(),\n        };\n        if !wants_semantic {\n            ("lexical", None, lexical_search_hits(&lexical_hits, limit))\n        } else if let Some(provider) = state.semantic_provider.clone() {\n            let semantic_projection = semantic_projection_for(&state, &snapshot, &provider).await?;\n            let semantic_hits = semantic_projection.search(\n                provider.as_ref(),\n                query_text,\n                query.source_id.as_ref().map(|source_id| source_id.as_str()),\n                candidate_limit,\n            )?;\n            let model = Some(semantic_projection.model().to_owned());\n            if requested_mode == "semantic" {\n                ("semantic", model, semantic_search_hits(&semantic_hits, limit))\n            } else {\n                (\n                    "hybrid",\n                    model,\n                    hybrid_search_hits(&lexical_hits, &semantic_hits, limit),\n                )\n            }\n        } else if requested_mode == "auto" {\n            ("lexical", None, lexical_search_hits(&lexical_hits, limit))\n        } else {\n            return Err(ApiError(DomainError::InvalidCommand(\n                "semantic retrieval is not configured on this server; use mode=lexical or configure an embedding endpoint".to_owned(),\n            )));\n        }\n    };\n\n    #[cfg(not(feature = "semantic"))]\n    let (effective_mode, semantic_model, mut results) = {\n        if matches!(requested_mode.as_str(), "semantic" | "hybrid") {\n            return Err(ApiError(DomainError::InvalidCommand(\n                "semantic retrieval requires an aidememo-server build with --features semantic"\n                    .to_owned(),\n            )));\n        }\n        ("lexical", None, lexical_search_hits(&lexical_hits, limit))\n    };\n\n    results.truncate(limit);\n    Ok((\n        StatusCode::OK,\n        Json(SearchResponse {\n            project_epoch: projection.project_epoch().clone(),\n            index_seq: projection.index_seq(),\n            mode: effective_mode,\n            semantic_model,\n            results,\n        }),\n    ))\n}'''
text = replace_once(text, old_search, new_search, 'search implementation')
helper_anchor = '''#[derive(Serialize)]\nstruct ResourceResponse {\n'''
helpers = '''fn lexical_search_hits(hits: &[lexical::LexicalHit], limit: usize) -> Vec<SearchHit> {\n    hits.iter()\n        .take(limit)\n        .map(|hit| SearchHit {\n            fact_id: hit.fact_id.clone(),\n            session_id: hit.session_id.clone(),\n            source_id: hit.source_id.clone(),\n            actor_id: hit.actor_id.clone(),\n            content: hit.content.clone(),\n            score: hit.score,\n            lexical_score: Some(hit.score),\n            semantic_score: None,\n        })\n        .collect()\n}\n\n#[cfg(feature = "semantic")]\nfn semantic_search_hits(hits: &[semantic::SemanticHit], limit: usize) -> Vec<SearchHit> {\n    hits.iter()\n        .take(limit)\n        .map(|hit| SearchHit {\n            fact_id: hit.fact_id.clone(),\n            session_id: hit.session_id.clone(),\n            source_id: hit.source_id.clone(),\n            actor_id: hit.actor_id.clone(),\n            content: hit.content.clone(),\n            score: hit.score,\n            lexical_score: None,\n            semantic_score: Some(hit.score),\n        })\n        .collect()\n}\n\n#[cfg(feature = "semantic")]\nfn hybrid_search_hits(\n    lexical_hits: &[lexical::LexicalHit],\n    semantic_hits: &[semantic::SemanticHit],\n    limit: usize,\n) -> Vec<SearchHit> {\n    const RRF_K: f64 = 60.0;\n    let mut merged = HashMap::<String, SearchHit>::new();\n    for (rank, hit) in lexical_hits.iter().enumerate() {\n        let score = 1.0 / (RRF_K + rank as f64 + 1.0);\n        merged.insert(\n            hit.fact_id.as_str().to_owned(),\n            SearchHit {\n                fact_id: hit.fact_id.clone(),\n                session_id: hit.session_id.clone(),\n                source_id: hit.source_id.clone(),\n                actor_id: hit.actor_id.clone(),\n                content: hit.content.clone(),\n                score,\n                lexical_score: Some(hit.score),\n                semantic_score: None,\n            },\n        );\n    }\n    for (rank, hit) in semantic_hits.iter().enumerate() {\n        let rrf = 1.0 / (RRF_K + rank as f64 + 1.0);\n        let entry = merged\n            .entry(hit.fact_id.as_str().to_owned())\n            .or_insert_with(|| SearchHit {\n                fact_id: hit.fact_id.clone(),\n                session_id: hit.session_id.clone(),\n                source_id: hit.source_id.clone(),\n                actor_id: hit.actor_id.clone(),\n                content: hit.content.clone(),\n                score: 0.0,\n                lexical_score: None,\n                semantic_score: None,\n            });\n        entry.score += rrf;\n        entry.semantic_score = Some(hit.score);\n    }\n    let mut results = merged.into_values().collect::<Vec<_>>();\n    results.sort_by(|left, right| {\n        right\n            .score\n            .total_cmp(&left.score)\n            .then_with(|| left.fact_id.as_str().cmp(right.fact_id.as_str()))\n    });\n    results.truncate(limit);\n    results\n}\n\n#[cfg(feature = "semantic")]\nasync fn semantic_projection_for(\n    state: &ServerState,\n    snapshot: &ProjectSnapshot,\n    provider: &SharedEmbeddingProvider,\n) -> Result<Arc<semantic::SemanticProjection>, ApiError> {\n    {\n        let cached = state.semantic_projection.lock().await;\n        if let Some(projection) = cached.as_ref()\n            && projection.matches(snapshot, provider.as_ref())\n        {\n            return Ok(projection.clone());\n        }\n    }\n    let projection = Arc::new(semantic::SemanticProjection::rebuild(\n        snapshot,\n        provider.as_ref(),\n    )?);\n    let mut cached = state.semantic_projection.lock().await;\n    if let Some(current) = cached.as_ref()\n        && current.matches(snapshot, provider.as_ref())\n    {\n        return Ok(current.clone());\n    }\n    *cached = Some(projection.clone());\n    Ok(projection)\n}\n\n'''
text = replace_once(text, helper_anchor, helpers + helper_anchor, 'search helpers')
path.write_text(text)

main = Path('crates/aidememo-server/src/main.rs')
text = main.read_text()
text = replace_once(
    text,
    'use aidememo_server::{ServerState, bearer_token_digest, router};',
    '''use aidememo_server::{ServerState, bearer_token_digest, router};\n#[cfg(feature = "semantic")]\nuse aidememo_server::HttpEmbeddingProvider;''',
    'main semantic import',
)
text = replace_once(
    text,
    '''    /// HTTP bind address. Loopback is required unless explicitly overridden.\n    #[arg(long, default_value = "127.0.0.1:3030")]\n    bind: SocketAddr,''',
    '''    /// OpenAI-compatible embedding endpoint used by semantic/hybrid retrieval.\n    #[arg(long)]\n    embedding_endpoint: Option<String>,\n    /// Embedding model sent to the configured endpoint.\n    #[arg(long, default_value = "text-embedding-3-small")]\n    embedding_model: String,\n    /// Exact embedding dimension expected from the configured model.\n    #[arg(long, default_value_t = 0)]\n    embedding_dimension: usize,\n    /// Optional environment variable containing the embedding API key.\n    #[arg(long)]\n    embedding_api_key_env: Option<String>,\n    /// HTTP bind address. Loopback is required unless explicitly overridden.\n    #[arg(long, default_value = "127.0.0.1:3030")]\n    bind: SocketAddr,''',
    'serve embedding args',
)
text = replace_once(
    text,
    '''    let listener = tokio::net::TcpListener::bind(args.bind).await?;''',
    '''    #[cfg(feature = "semantic")]\n    let state = if let Some(endpoint) = args.embedding_endpoint.as_deref() {\n        if args.embedding_dimension == 0 {\n            return Err(std::io::Error::new(\n                std::io::ErrorKind::InvalidInput,\n                "--embedding-dimension must be greater than zero when --embedding-endpoint is set",\n            )\n            .into());\n        }\n        let api_key = args\n            .embedding_api_key_env\n            .as_deref()\n            .and_then(|name| std::env::var(name).ok());\n        let provider = HttpEmbeddingProvider::new(\n            endpoint,\n            &args.embedding_model,\n            args.embedding_dimension,\n            api_key,\n        )?;\n        state.with_semantic_provider(std::sync::Arc::new(provider))\n    } else {\n        state\n    };\n    #[cfg(not(feature = "semantic"))]\n    let state = {\n        if args.embedding_endpoint.is_some() {\n            return Err(std::io::Error::new(\n                std::io::ErrorKind::Unsupported,\n                "embedding endpoint requires an aidememo-server build with --features semantic",\n            )\n            .into());\n        }\n        state\n    };\n    let listener = tokio::net::TcpListener::bind(args.bind).await?;''',
    'serve semantic provider',
)
text = replace_once(
    text,
    '''fn validate_serve_args(args: &ServeArgs) -> Result<(), std::io::Error> {\n    if !args.bind.ip().is_loopback() && !args.allow_insecure_http {''',
    '''fn validate_serve_args(args: &ServeArgs) -> Result<(), std::io::Error> {\n    if args.embedding_endpoint.is_none()\n        && (args.embedding_dimension != 0 || args.embedding_api_key_env.is_some())\n    {\n        return Err(std::io::Error::new(\n            std::io::ErrorKind::InvalidInput,\n            "embedding dimension/API-key options require --embedding-endpoint",\n        ));\n    }\n    if !args.bind.ip().is_loopback() && !args.allow_insecure_http {''',
    'serve semantic validation',
)
main.write_text(text)
