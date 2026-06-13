#!/usr/bin/env bash

# Resolve a Python interpreter supported by PyO3 0.29. Prefer stable CPython
# minors used by release CI, but accept newer supported local interpreters.

aidememo_pyo3_python_is_supported() {
    local python_bin="$1"
    "$python_bin" - <<'PY' >/dev/null 2>&1
import sys

version = sys.version_info
raise SystemExit(0 if (3, 9) <= version[:2] <= (3, 15) else 1)
PY
}

aidememo_pyo3_python_version() {
    local python_bin="$1"
    "$python_bin" - <<'PY'
import sys

print(f"{sys.version_info.major}.{sys.version_info.minor}.{sys.version_info.micro}")
PY
}

aidememo_resolve_pyo3_python() {
    local override="${AIDEMEMO_PYO3_PYTHON:-${PYO3_PYTHON:-}}"
    local candidate

    if [[ -n "$override" ]]; then
        if ! command -v "$override" >/dev/null 2>&1; then
            echo "AIDEMEMO_PYO3_PYTHON/PYO3_PYTHON points to a missing interpreter: $override" >&2
            return 1
        fi
        if ! aidememo_pyo3_python_is_supported "$override"; then
            echo "AIDEMEMO_PYO3_PYTHON/PYO3_PYTHON must be Python 3.9-3.15 for aidememo-python/PyO3 0.29 (got $(aidememo_pyo3_python_version "$override"))" >&2
            return 1
        fi
        command -v "$override"
        return
    fi

    for candidate in python3.13 python3.14 python3.15 python3.12 python3.11 python3.10 python3.9 python3; do
        if command -v "$candidate" >/dev/null 2>&1 && aidememo_pyo3_python_is_supported "$candidate"; then
            command -v "$candidate"
            return
        fi
    done

    echo "Could not find a PyO3-compatible Python (need 3.9-3.15). Set AIDEMEMO_PYO3_PYTHON=/path/to/python3.13." >&2
    return 1
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    aidememo_resolve_pyo3_python
fi
