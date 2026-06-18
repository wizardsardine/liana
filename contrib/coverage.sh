#!/usr/bin/env bash
set -euo pipefail

# Rust coverage for the main workspace. This intentionally excludes fuzz
# targets: libFuzzer binaries do not behave like normal test binaries under
# `-- --list` or coverage collection.
#
# Common local usage:
#   ./contrib/coverage.sh
#   HTML=1 ./contrib/coverage.sh
#   FAIL_UNDER_LINES=45 ./contrib/coverage.sh

OUTPUT_DIR="${OUTPUT_DIR:-target/coverage}"
LCOV_PATH="${LCOV_PATH:-${OUTPUT_DIR}/lcov.info}"
SUMMARY_PATH="${SUMMARY_PATH:-${OUTPUT_DIR}/summary.json}"
HTML="${HTML:-0}"
COVERAGE_CLEAN="${COVERAGE_CLEAN:-1}"

# The GUI embeds this value at compile time. Unit and integration tests do not
# contact Breez unless explicitly configured, so mirror CI's harmless default.
export BREEZ_API_KEY="${BREEZ_API_KEY:-DUMMY_BREEZ_API_KEY}"

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  cat >&2 <<'EOF'
error: cargo-llvm-cov is required.

Install it with:
  cargo install cargo-llvm-cov
EOF
  exit 1
fi

mkdir -p "${OUTPUT_DIR}"

coverage_args=(
  --workspace
  --exclude coincube-fuzz
  --lib
  --bins
  --tests
  --ignore-filename-regex '/(target|fuzz)/'
)

coverage_command=(
  cargo llvm-cov
  "${coverage_args[@]}"
)

if [[ "${COVERAGE_CLEAN}" != "1" ]]; then
  coverage_command+=(--no-clean)
fi

if [[ -n "${FAIL_UNDER_LINES:-}" ]]; then
  coverage_command+=(--fail-under-lines "${FAIL_UNDER_LINES}")
fi
if [[ -n "${FAIL_UNDER_FUNCTIONS:-}" ]]; then
  coverage_command+=(--fail-under-functions "${FAIL_UNDER_FUNCTIONS}")
fi
if [[ -n "${FAIL_UNDER_REGIONS:-}" ]]; then
  coverage_command+=(--fail-under-regions "${FAIL_UNDER_REGIONS}")
fi

if [[ "${COVERAGE_CLEAN}" == "1" ]]; then
  cargo llvm-cov clean --workspace
fi
coverage_command+=(
  --lcov
  --output-path "${LCOV_PATH}"
)
set +e
"${coverage_command[@]}" "$@"
run_status=$?
set -e

cargo llvm-cov report \
  --ignore-filename-regex '/(target|fuzz)/' \
  --summary-only \
  --json \
  --output-path "${SUMMARY_PATH}"

if [[ "${HTML}" == "1" ]]; then
  cargo llvm-cov report \
    --ignore-filename-regex '/(target|fuzz)/' \
    --html \
    --output-dir "${OUTPUT_DIR}/html"
fi

cat <<EOF
Coverage artifacts written:
  LCOV:    ${LCOV_PATH}
  Summary: ${SUMMARY_PATH}
EOF

if [[ "${HTML}" == "1" ]]; then
  echo "  HTML:    ${OUTPUT_DIR}/html/index.html"
fi

if [[ "${run_status}" -ne 0 ]]; then
  echo "Coverage test run failed with exit status ${run_status}." >&2
fi
exit "${run_status}"
