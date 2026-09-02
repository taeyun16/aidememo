# Liquid AI LFM Experiments

The LFM model evaluation scripts and LongMemEval benchmarks have been moved outside this repository.

For historical context and model placement guidance, see:
- [Operations](OPERATIONS.md#pick-embedding-mode) for embedding model selection
- [Evidence](EVIDENCE.md#model-placement) for the concise evidence summary
- [Measurements](MEASUREMENTS.md) for retrieval and model trade-offs

The privacy filter sidecars (`privacy_filter_sidecar.py` and `privacy_filter_mlx_sidecar.py`) remain in the `scripts/` directory for users who wish to run them locally. The LFM-specific sidecars were removed with the evaluation scripts.
