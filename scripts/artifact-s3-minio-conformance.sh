#!/usr/bin/env bash
set -euo pipefail

for command in minio mc curl cargo; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "required command is not installed: $command" >&2
    exit 1
  fi
done

api_port="${AIDEMEMO_MINIO_API_PORT:-19000}"
console_port="${AIDEMEMO_MINIO_CONSOLE_PORT:-19001}"
case "$api_port:$console_port" in
  *[!0-9:]*|:*|*:)
    echo "MinIO ports must be positive integers" >&2
    exit 1
    ;;
esac
api_port_number=$((10#$api_port))
console_port_number=$((10#$console_port))
if ((api_port_number < 1 || api_port_number > 65535 || console_port_number < 1 || console_port_number > 65535)); then
  echo "MinIO ports must be between 1 and 65535" >&2
  exit 1
fi

runtime_base="${TMPDIR:-/tmp}"
runtime_dir="$(mktemp -d "${runtime_base%/}/aidememo-minio.XXXXXX")"
minio_pid=""
log_path="$runtime_dir/minio.log"
access_key="aidememo-conformance"
secret_key="aidememo-conformance-secret-key"
bucket="aidememo-conformance"
endpoint="http://127.0.0.1:$api_port"

cleanup() {
  status=$?
  trap - EXIT INT TERM
  if [[ -n "$minio_pid" ]]; then
    kill "$minio_pid" >/dev/null 2>&1 || true
    wait "$minio_pid" >/dev/null 2>&1 || true
  fi
  if [[ "$status" -ne 0 && -f "$log_path" ]]; then
    tail -n 80 "$log_path" >&2 || true
  fi
  case "$runtime_dir" in
    "${runtime_base%/}"/aidememo-minio.*)
      rm -r -- "$runtime_dir"
      ;;
    *)
      echo "refusing to remove unexpected temporary path: $runtime_dir" >&2
      status=1
      ;;
  esac
  exit "$status"
}
trap cleanup EXIT INT TERM

MINIO_ROOT_USER="$access_key" \
MINIO_ROOT_PASSWORD="$secret_key" \
minio server "$runtime_dir/data" \
  --address "127.0.0.1:$api_port" \
  --console-address "127.0.0.1:$console_port" \
  >"$log_path" 2>&1 &
minio_pid=$!

ready=0
for _ in $(seq 1 60); do
  if curl --silent --fail --max-time 1 "$endpoint/minio/health/live" >/dev/null; then
    ready=1
    break
  fi
  if ! kill -0 "$minio_pid" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
if [[ "$ready" -ne 1 ]]; then
  echo "MinIO did not become ready at $endpoint" >&2
  exit 1
fi

MC_CONFIG_DIR="$runtime_dir/mc" mc alias set aidememo-conformance \
  "$endpoint" "$access_key" "$secret_key" >/dev/null
MC_CONFIG_DIR="$runtime_dir/mc" mc mb --ignore-existing \
  "aidememo-conformance/$bucket" >/dev/null

AWS_ACCESS_KEY_ID="$access_key" \
AWS_SECRET_ACCESS_KEY="$secret_key" \
AIDEMEMO_S3_TEST_BUCKET="$bucket" \
AIDEMEMO_S3_TEST_PREFIX="aidememo/conformance" \
AIDEMEMO_S3_TEST_REGION="us-east-1" \
AIDEMEMO_S3_TEST_ENDPOINT="$endpoint" \
AIDEMEMO_S3_TEST_FORCE_PATH_STYLE="true" \
cargo test -p aidememo-artifacts --features s3 \
  --test s3_live_conformance s3_provider_presigned_lifecycle_conforms \
  -- --ignored --exact
