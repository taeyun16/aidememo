# Liquid AI LFM 실험

이 문서는 Liquid AI LFM 모델의 선택적 외부 모델 설정, 학습, 평가 절차를
보존합니다. LFM 모델은 AideMemo에 번들되지 않으며 전역 기본값도 아닙니다.
일반적인 임베딩 선택은 [운영](OPERATIONS.md#임베딩-mode-선택)을, 간결한
근거 요약은 [근거](EVIDENCE.md#모델-배치)를 참고합니다.

## 배치와 경계

LFM 모델은 AideMemo에 번들된 자산이 아니라 선택적 외부 모델로 취급합니다.
LFM Open License v1.0에는 상업적 사용 조건이 있으며, LFM 모델 계열마다
시스템에서 적합한 위치가 다릅니다.

LFM 계열을 하나의 전역 모델 전환이 아니라 계층형 소형 모델 스택으로
사용합니다.

| AideMemo surface | 첫 LFM 후보 | 사용할 때 | 사용하지 않을 때 |
|---|---|---|---|
| 1단계 semantic 검색 | `mlx-community/LFM2.5-Embedding-350M-4bit` | BM25가 약하거나 빈 후보를 반환할 때, 특히 강한 lexical anchor가 없는 다국어 쿼리에 사용합니다. 추적 문서 gate에서는 lexical 실패 구간만 승격해 `R@8`이 0.656에서 0.812로 향상됐고, HF agent-trace gate에서는 BM25가 이미 강할 때 순수 LFM dense가 더 낮은 성능을 보였습니다. `search.auto_hybrid=true`를 유지하고 반복 호출에는 warm daemon을 사용합니다. | 일반 lexical/code/doc 검색이 이미 포화됐거나 모든 쿼리에 LFM dense를 실행하게 될 때입니다. |
| BERT 계열 semantic baseline | `fastembed` / `bge-small-en-v1.5` | 더 무거운 BERT 스타일 encoder를 허용할 수 있는 offline 또는 high-stakes 비교에 사용합니다. HF 60-doc 구간에서 `R@8=0.994`를 기록해 BM25/BGE head 품질에 근접했으며 검증 baseline으로 유용합니다. | Agent hot path에는 사용하지 않습니다. 측정된 daemon query 평균은 쿼리당 약 1680 ms로, LFM query embedding 약 31 ms와 BM25/model2vec 경로보다 훨씬 느렸습니다. |
| Reranking | `mlx-community/LFM2.5-ColBERT-350M-4bit` 또는 `LiquidAI/LFM2.5-ColBERT-350M` sidecar | BM25/hybrid 후보 recall은 높지만 최상위 결과의 순서가 자주 잘못될 때 사용합니다. | 의도적으로 all-doc/multi-vector ColBERT index를 만들지 않는 한 올바른 팩트가 후보 집합에 없을 때는 사용하지 않습니다. |
| 팩트 추출/타입 분류 | fact-type LoRA adapter를 적용한 `LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit` | 잔여/default-note 사례나 pending review에 사용할 local high-confidence `fact_type_hint` 후보가 필요할 때 사용합니다. Corpus-only 240-iteration LoRA는 seed coding-agent corpus test와 이전 45-case holdout 모두에서 deterministic inference를 앞섰습니다. | 라벨이 없는 실제 트래픽에서 자동 고정밀 쓰기가 필요할 때는 사용하지 않습니다. 먼저 검토된 shadow corpus를 늘리고 검증합니다. |
| 쿼리 라우팅 | deterministic rules + search confidence | BM25-only, dense, ColBERT, aggregate 중 하나를 선택해야 할 때 사용합니다. | 아직 여기에 LFM text-generation 호출 비용을 쓰지 않습니다. MLX LM router micro-eval은 규칙보다 약했습니다. |
| Batch consolidation/conflict review | review hint만 사용하며 아직 자동 LFM 배치는 없음 | 사람이거나 더 강한 agent가 duplicate/supersede 제안을 검토할 때 사용합니다. | 검증된 LFM text-generation 모델이 supersede/archive 결정을 자동으로 쓰게 하지 않습니다. |
| 이미지/스크린샷 메모리 캡처 | 더 안정적인 `mlx-vlm` 호환성을 확보할 때까지 보류 | 스크린샷이나 diagram을 실험하며 review-only 출력을 허용할 수 있을 때 사용합니다. | 현재 VL smoke의 JSON/OCR 출력은 불안정하므로 screenshot-to-fact 캡처를 출시하지 않습니다. |
| 음성/회의 캡처 | 아직 Mac MLX 배치 없음 | 향후 local audio runtime에 WER와 fact-precision 측정값이 생겼을 때 사용합니다. | 기존 wiki의 text retrieval만 필요할 때는 사용하지 않습니다. |

검증해야 할 제품 차별점은 이것입니다. 모든 메모리 작업을 하나의 대형 원격
모델에 의존하는 대신, 소형 로컬 모델을 정확히 사용한 실패 지점에서 메모리
시스템이 개선된다는 점을 AideMemo가 입증할 수 있어야 합니다.

Dense 다국어 검색 실험에는 `LiquidAI/LFM2.5-Embedding-350M`을 사용합니다.
비대칭 embedding 모델이므로 vector를 다시 만들기 전에 query와 document
prefix를 설정합니다.

```bash
aidememo config set model.provider openai
aidememo config set model.endpoint http://127.0.0.1:8080/v1/embeddings
aidememo config set model.name LiquidAI/LFM2.5-Embedding-350M
aidememo config set model.dimension 1024
aidememo config set model.query_prefix "query: "
aidememo config set model.document_prefix "document: "
aidememo vector-rebuild --current-only
aidememo search "redis timeout root cause" --hybrid
```

이 모델을 `sentence-transformers`로 직접 실행할 때 Liquid model card는
bidirectional patch를 위해 `trust_remote_code=True`로 로드합니다. Hub code를
AideMemo 프로세스 안에서 실행하기보다, 감사하고 격리한 OpenAI-compatible
embedding endpoint 뒤에서 제공하는 방식을 권장합니다.

Production server를 연결하기 전에 dense가 workload에 도움이 되는지 확인하려면
scenario micro-eval을 실행합니다.

```bash
python3 scripts/lfm_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model LiquidAI/LFM2.5-Embedding-350M
```

`embedding_health.valid`가 false이면 해당 수치를 검색 근거로 사용하지 않습니다.
안전한 `sentence-transformers` 경로는 1024차원 vector를 로드하면서도
bidirectional patch 없이 동일한 embedding을 만들 수 있습니다.

Apple Silicon의 로컬 dense 검증에는 MLX 변환본을 우선 사용합니다.

```bash
hf download mlx-community/LFM2.5-Embedding-350M-4bit \
  --local-dir /private/tmp/lfm25-embedding-mlx-4bit

python3 scripts/lfm_mlx_dense_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit
```

MLX 경로에는 Metal 접근이 필요합니다. Headless 또는 sandbox session에서
`mlx`는 `No Metal device available`로 실패할 수 있으므로 Apple GPU가 보이는
프로세스에서 실행합니다.

실제 AideMemo semantic 경로 뒤에 MLX LFM embedder를 배치하려면
TEI-compatible sidecar를 실행하고 실험용 provider alias를 설정합니다.

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_embedding_sidecar.py \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --port 8088

aidememo config set model.provider lfm-sidecar
aidememo config set model.endpoint http://127.0.0.1:8088
aidememo config set model.name mlx-community/LFM2.5-Embedding-350M-4bit
aidememo config set model.dimension 1024
aidememo config set model.query_prefix "query: "
aidememo config set model.document_prefix "document: "
aidememo vector-rebuild --current-only
aidememo daemon start
aidememo search "레디스 장애 원인" -l 8
```

이 구성은 MLX와 Liquid model code를 Rust 프로세스 밖에 두면서도 기존
`auto_hybrid` + HNSW 흐름을 사용합니다. Sidecar가 실행 중이 아니면 semantic
승격은 치명적이지 않게 실패하고 AideMemo는 BM25 probe로 폴백합니다.

`LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit`를 로컬 팩트 추출과 타입 분류
실험에 사용할 때는 pending-review helper로만 둡니다. Mac MLX micro-eval은
자동 쓰기를 지지하지 않았습니다. 관측된 최상 추출 성능은
`fact_type_accuracy=0.63`, `entity_recall=0.88`이었고 router/consolidation
정확도는 여전히 너무 낮았습니다. 230M/350M MLX 모델은 추출이 더 빠르지만
정확도가 낮았습니다. Closed-label classifier 평가도 같은 배치를 지지합니다.
`lfm_mlx_fact_type_eval.py`로 고정 AideMemo label 중 하나를 scoring해도
45-case mixed-language fixture에서 deterministic strong-cue inference보다
낮았습니다(`0.33` best LFM 대 `0.69` baseline). 다만 최상 1.2B-Instruct
실행에는 매우 작은 high-confidence review 구간이 있었습니다
(`confidence >= 0.8`: 6/6 correct). 이를 default writer가 아니라
`fact_type_hint`/pending-review 근거로 사용합니다. Fine-tuning이 이 결론을
바꾸는지 시험하려면 supervised label dataset을 만들고 MLX LoRA adapter를
학습합니다. 첫 synthetic-only smoke는 240 iteration 뒤 45-case holdout에서
`0.84`를 기록했습니다. 이후 coding-agent seed corpus에는
`fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl`을 추가했습니다
(검토된 형태의 row 108개, label당 12개, train/valid/test = 72/18/18).
Corpus-only adapter는 더 어려운 18-case test에서 deterministic baseline
`0.39` 대비 `0.61`, 기존 45-case holdout에서 baseline `0.69` 대비 `0.82`를
기록했습니다. 실제 capture traffic이 threshold와 `claim`/`convention`/`note`
경계의 안정성을 확인할 때까지 shadow `fact_type_hint` 경로로 유지합니다.

```bash
hf download LiquidAI/LFM2.5-1.2B-Instruct-MLX-4bit \
  --local-dir /private/tmp/lfm25-12b-instruct-mlx-4bit

python3 scripts/lfm_mlx_lm_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --suite extraction \
  --prompt-style compact

python3 scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --prompt-style compact \
  --summary-only

python3 scripts/lfm_fact_type_sft_data.py \
  --out /private/tmp/aidememo-lfm-fact-type-corpus-sft

/private/tmp/aidememo-lfm-venv/bin/mlx_lm.lora \
  --model /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --train \
  --data /private/tmp/aidememo-lfm-fact-type-corpus-sft \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --fine-tune-type lora \
  --mask-prompt \
  --num-layers 8 \
  --batch-size 1 \
  --grad-accumulation-steps 8 \
  --iters 240 \
  --learning-rate 5e-5

python3 scripts/lfm_mlx_fact_type_eval.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --cases-file fixtures/fact_type_corpus/coding_agent_shadow_seed.jsonl \
  --case-split test \
  --prompt-style compact \
  --summary-only
```

지속적인 shadow loop에서는 seed fixture를 직접 수정하지 말고, agent가 라벨을
붙이고 검토된 팩트를 JSONL로 추가합니다.

```json
{"id":"agent-run-20260707-001","text":"Tried the daemon prewarm path, but the real issue was a stale HNSW sidecar.","fact_type":"lesson","scenario":"daemon_ops","language":"en","split":"train"}
```

실시간 MCP agent에서는 저장되는 팩트를 바꾸지 않으면서 성공한 쓰기를
supervised shadow corpus에 추가하게 합니다.

```bash
export AIDEMEMO_FACT_TYPE_SHADOW_LOG=~/.aidememo/fact-type-shadow.jsonl
aidememo mcp-serve --port 3000
```

Row는 append-only JSONL이며 저장된 fact id, text, fact_type,
`label_source`(`explicit`, `inferred`, `default` 중 하나), entities, source_id,
origin(`mcp_fact_add`/`mcp_fact_add_many`), timestamp를 포함합니다. 명시적으로
agent가 라벨을 붙인 row를 supervised training에 사용합니다. Inferred/default
row는 audit에 유용하지만 `--include-inferred-labels`를 전달하지 않으면
`lfm_fact_type_sft_data.py`가 건너뜁니다. `fact_type_hint`가 있는 row도 저장된
label과 strong-cue hint가 일치하지 않으므로 기본적으로 건너뜁니다. 검토한
뒤에만 `--include-disputed-labels`를 전달합니다.

그런 다음 seed corpus와 검토된 capture file을 함께 학습합니다.

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --corpus ~/.aidememo/fact-type-shadow.jsonl \
  --out /private/tmp/aidememo-lfm-fact-type-shadow-sft
```

Tuned adapter를 hint-only sidecar로 실행합니다.

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --confidence-threshold 0.8 \
  --input-jsonl ~/.aidememo/fact-type-shadow.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-fact-type-hints.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl ~/.aidememo/fact-type-shadow.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-fact-type-hints.jsonl \
  --min-precision 0.95 \
  --max-baseline-correct-harms 0
```

외부 LLM 호출 없이 local agent behavior log에서 같은 sidecar를 stress-test하려면
AgentStep/Hermes log에서 weak-labelled probe를 만들고 같은 threshold gate를
실행합니다.

```bash
python3 scripts/lfm_fact_type_log_fixture.py \
  --out-dir /private/tmp/aidememo-lfm-log-probes \
  --max-rows 72 \
  --max-per-label 12
```

더 넓은 public-trace gate를 실행하려면 Hugging Face agent trace에서
weak-labelled probe를 만듭니다. 이 과정은 Dataset Viewer API로 row를 가져오고,
email과 긴 identifier를 redact하며, `/private/tmp` 아래에 compact candidate-memory
JSONL만 기록합니다.

```bash
python3 scripts/lfm_fact_type_hf_probe.py \
  --out-dir /private/tmp/aidememo-lfm-hf-probes \
  --source-rows 100 \
  --max-rows-per-dataset 100 \
  --max-per-label 25

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --input-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --input-split test \
  --output-jsonl /private/tmp/aidememo-lfm-hf-probes/fact_type_hints_hf.jsonl

python3 scripts/lfm_fact_type_threshold_eval.py \
  --labels-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --label-split test \
  --predictions-jsonl /private/tmp/aidememo-lfm-hf-probes/fact_type_hints_hf.jsonl \
  --confidence-grid 0.80,0.90,0.95,0.98
```

이를 검토된 정답이 아니라 raw-trace stress test로 취급합니다. 7월 8일 public
HF 실행에서는 raw trace event에 `confidence >= 0.80`이 너무 느슨했지만,
deterministic extraction으로 stream을 durable memory-candidate category까지
먼저 걸러낸 뒤에는 high-confidence hint가 유용해졌습니다. Script는 이 두 번째
gate를 위한 `high_signal_hf_fact_type_probe.jsonl`도 기록합니다.

같은 HF trace에서 검색 배치를 시험하려면 compact probe row를 corpus/query/qrels
file로 변환한 뒤 BM25, model2vec, LFM dense, 선택적 fastembed/BGE profile을
실행합니다.

```bash
python3 scripts/lfm_hf_agent_trace_retrieval_fixture.py \
  --probe-jsonl /private/tmp/aidememo-lfm-hf-probes/combined_hf_fact_type_probe.jsonl \
  --out-dir /private/tmp/aidememo-lfm-hf-retrieval \
  --variants surface,paraphrase,cjk \
  --max-docs-per-source 60

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --no-default-docs \
  --corpus-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_corpus.jsonl \
  --queries-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_queries.jsonl \
  --qrels-tsv /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_qrels.tsv \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --candidate-limit 8 \
  --summary-only \
  --output-json /private/tmp/aidememo-lfm-hf-retrieval/eval_summary.json
```

BGE/BERT 계열 fastembed baseline을 평가할 때는 optional feature로 CLI를
build하고, offline batch가 아니라면 더 작은 구간을 사용합니다.

```bash
cargo build -p aidememo-cli --features fastembed

/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_mlx_docs_recall_eval.py \
  --aidememo target/debug/aidememo \
  --no-default-docs \
  --corpus-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_corpus.jsonl \
  --queries-jsonl /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_queries.jsonl \
  --qrels-tsv /private/tmp/aidememo-lfm-hf-retrieval/hf_agent_trace_qrels.tsv \
  --model-dir /private/tmp/lfm25-embedding-mlx-4bit \
  --fastembed-model bge-small-en-v1.5 \
  --candidate-limit 8 \
  --summary-only
```

일회성 manual hint에는 text를 직접 전달합니다.

```bash
/private/tmp/aidememo-lfm-venv/bin/python scripts/lfm_fact_type_sidecar.py \
  --model-dir /private/tmp/lfm25-12b-instruct-mlx-4bit \
  --adapter-path /private/tmp/aidememo-lfm-fact-type-corpus-lora \
  --confidence-threshold 0.8 \
  --text "Tried daemon prewarm, but the real issue was a stale HNSW sidecar."
```

Sidecar는 `suggested_fact_type`, confidence, margin, runner-up, `accepted`를
반환하며 AideMemo에 쓰지 않습니다. 먼저 pending review나 UI surface에
연결합니다. 더 큰 reviewed capture corpus가 낮은 baseline-correct harm을
입증하기 전에는 명시적인 agent `fact_type`을 덮어쓰게 하지 않습니다.

Synthetic augmentation은 실험으로만 사용합니다.

```bash
python3 scripts/lfm_fact_type_sft_data.py \
  --corpus /path/to/reviewed-agent-facts.jsonl \
  --examples-per-label 40 \
  --out /private/tmp/aidememo-lfm-fact-type-shadow-synth40-sft
```

7월 7일 비교에서 synthetic augmentation은 taxonomy를 편향시킬 수 있었습니다.
Mixed adapter는 corpus-test accuracy를 `0.61`로 유지했지만 이전 holdout은
`0.82`에서 `0.71`로 낮아졌고 원래의 모든 `note` 예제를 `lesson`으로
분류했습니다. Synthetic row는 training set을 늘리는 기본 방식이 아니라
stress test로 취급합니다.

Native multi-vector index를 검토하기 전에
`LiquidAI/LFM2.5-ColBERT-350M`을 sidecar reranker로 사용합니다.
`LiquidAI/LFM2-ColBERT-350M`도 `--model LiquidAI/LFM2-ColBERT-350M`으로
같은 script에서 사용할 수 있습니다. 원래 LFM2 결과를 재현할 때가 아니라면
2.5를 기본으로 유지합니다. 현재 AideMemo는 fact당 vector 하나를 저장하지만,
ColBERT는 token당 vector 하나를 저장하고 MaxSim으로 scoring합니다. Apple
Silicon의 local MaxSim 검증에는 MLX 변환본을 우선 사용합니다.

```bash
hf download mlx-community/LFM2.5-ColBERT-350M-4bit \
  --local-dir /private/tmp/lfm25-colbert-mlx-4bit

python3 scripts/lfm_mlx_colbert_eval.py \
  --aidememo target/debug/aidememo \
  --model-dir /private/tmp/lfm25-colbert-mlx-4bit \
  --candidate-limit 8
```

```bash
aidememo search "redis timeout root cause" --json -l 50 \
  | python3 scripts/lfm_colbert_rerank.py \
      --query "redis timeout root cause" \
      --top-k 10
```

Sidecar는 기본적으로 `trust_remote_code`를 끕니다. Model repository code를
감사하고 승인한 뒤에만 `--trust-remote-code`를 전달합니다.
