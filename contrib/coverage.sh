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
#   CARGO_TOOLCHAIN=nightly COVERAGE_DOCTESTS=1 ./contrib/coverage.sh

OUTPUT_DIR="${OUTPUT_DIR:-target/coverage}"
LCOV_PATH="${LCOV_PATH:-${OUTPUT_DIR}/lcov.info}"
SUMMARY_PATH="${SUMMARY_PATH:-${OUTPUT_DIR}/summary.json}"
HTML="${HTML:-0}"
COVERAGE_CLEAN="${COVERAGE_CLEAN:-1}"
CARGO_TOOLCHAIN="${CARGO_TOOLCHAIN:-}"
COVERAGE_DOCTESTS="${COVERAGE_DOCTESTS:-0}"

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

cargo_llvm_cov=(cargo)
if [[ -n "${CARGO_TOOLCHAIN}" ]]; then
  cargo_llvm_cov+=("+${CARGO_TOOLCHAIN}")
fi
cargo_llvm_cov+=(llvm-cov)

coverage_args=(
  --workspace
  --exclude coincube-fuzz
  --lib
  --bins
  --tests
  --ignore-filename-regex '/(target|fuzz)/'
)

report_args=(
  --ignore-filename-regex '/(target|fuzz)/'
)

if [[ "${COVERAGE_DOCTESTS}" == "1" ]]; then
  coverage_args+=(--doctests)
  report_args+=(--doctests)
fi

coverage_command=(
  "${cargo_llvm_cov[@]}"
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
  "${cargo_llvm_cov[@]}" clean --workspace
fi
coverage_command+=(
  --lcov
  --output-path "${LCOV_PATH}"
)
set +e
"${coverage_command[@]}" "$@"
run_status=$?

"${cargo_llvm_cov[@]}" report \
  "${report_args[@]}" \
  --summary-only \
  --json \
  --output-path "${SUMMARY_PATH}"
summary_status=$?

html_status=0
if [[ "${HTML}" == "1" ]]; then
  "${cargo_llvm_cov[@]}" report \
    "${report_args[@]}" \
    --html \
    --output-dir "${OUTPUT_DIR}/html"
  html_status=$?
fi
set -e

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
  exit "${run_status}"
fi
if [[ "${summary_status}" -ne 0 ]]; then
  echo "Coverage summary generation failed with exit status ${summary_status}." >&2
  exit "${summary_status}"
fi
if [[ "${html_status}" -ne 0 ]]; then
  echo "Coverage HTML generation failed with exit status ${html_status}." >&2
  exit "${html_status}"
fi
