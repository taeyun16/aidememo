from pathlib import Path

patch = Path('.github/scripts/patch_server_hybrid.py')
text = patch.read_text()

replacements = [
    (
        'use std::{collections::HashMap, sync::Arc};\\n\\n#[cfg(feature = "semantic")]\\npub use semantic::{EmbeddingProvider, HttpEmbeddingProvider, SharedEmbeddingProvider};',
        'use std::sync::Arc;\\n#[cfg(feature = "semantic")]\\nuse std::collections::HashMap;\\n\\n#[cfg(feature = "semantic")]\\npub use semantic::{EmbeddingProvider, HttpEmbeddingProvider, SharedEmbeddingProvider};',
    ),
    (
        '            artifacts: Some(Arc::new(ArtifactState {\\n                catalog: Mutex::new(catalog),\\n                bodies: ArtifactBodies::S3(bodies),\\n            })),\\n            semantic_provider: None,\\n            semantic_projection: Arc::new(Mutex::new(None)),\\n        })',
        '            artifacts: Some(Arc::new(ArtifactState {\\n                catalog: Mutex::new(catalog),\\n                bodies: ArtifactBodies::S3(bodies),\\n            })),\\n            #[cfg(feature = "semantic")]\\n            semantic_provider: None,\\n            #[cfg(feature = "semantic")]\\n            semantic_projection: Arc::new(Mutex::new(None)),\\n        })',
    ),
    (
        '    let lexical_strong = lexical_hits.len() >= limit.min(3)\\n        && lexical_hits.first().is_some_and(|hit| hit.score >= 1.25);\\n\\n    #[cfg(feature = "semantic")]',
        '    #[cfg(feature = "semantic")]\\n    let lexical_strong = lexical_hits.len() >= limit.min(3)\\n        && lexical_hits.first().is_some_and(|hit| hit.score >= 1.25);\\n\\n    #[cfg(feature = "semantic")]',
    ),
]
for old, new in replacements:
    if old not in text:
        raise SystemExit(f'missing hybrid patch snippet: {old[:90]}')
    text = text.replace(old, new, 1)
patch.write_text(text)

semantic = Path('crates/aidememo-server/src/semantic.rs')
text = semantic.read_text()
old = '''    #[must_use]\n    pub(crate) const fn project_epoch(&self) -> &ProjectEpoch {\n        &self.project_epoch\n    }\n\n    #[must_use]\n    pub(crate) const fn index_seq(&self) -> ProjectSequence {\n        self.index_seq\n    }\n\n'''
if old not in text:
    raise SystemExit('semantic accessor block changed')
text = text.replace(old, '', 1)
old = '''        assert_eq!(projection.index_seq(), ProjectSequence::new(2));\n        Ok(())'''
new = '''        assert!(projection.matches(&snapshot, &provider));\n        Ok(())'''
if old not in text:
    raise SystemExit('semantic sequence assertion changed')
semantic.write_text(text.replace(old, new, 1))
