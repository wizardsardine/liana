#!/usr/bin/env bash
set -euo pipefail

# Rust coverage for the standalone Spark bridge workspace. The bridge is
# intentionally excluded from the root workspace so its Breez Spark dependency
# graph can stay isolated from the GUI's Breez Liquid dependency graph.
#
# Common local usage:
#   ./contrib/coverage-bridge.sh
#   HTML=1 ./contrib/coverage-bridge.sh
#   FAIL_UNDER_LINES=10 ./contrib/coverage-bridge.sh

BRIDGE_MANIFEST="${BRIDGE_MANIFEST:-coincube-spark-bridge/Cargo.toml}"
OUTPUT_DIR="${OUTPUT_DIR:-target/coverage}"
LCOV_PATH="${LCOV_PATH:-${OUTPUT_DIR}/bridge-lcov.info}"
SUMMARY_PATH="${SUMMARY_PATH:-${OUTPUT_DIR}/bridge-summary.json}"
HTML="${HTML:-0}"
COVERAGE_CLEAN="${COVERAGE_CLEAN:-1}"
CARGO_TOOLCHAIN="${CARGO_TOOLCHAIN:-}"
COVERAGE_TARGET_LINES="${COVERAGE_TARGET_LINES:-}"

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

test_args=(
  --manifest-path "${BRIDGE_MANIFEST}"
  --workspace
  --no-report
)

report_args=(
  --manifest-path "${BRIDGE_MANIFEST}"
  --ignore-filename-regex '/target/'
)

if [[ -n "${FAIL_UNDER_LINES:-}" ]]; then
  report_args+=(--fail-under-lines "${FAIL_UNDER_LINES}")
fi
if [[ -n "${FAIL_UNDER_FUNCTIONS:-}" ]]; then
  report_args+=(--fail-under-functions "${FAIL_UNDER_FUNCTIONS}")
fi
if [[ -n "${FAIL_UNDER_REGIONS:-}" ]]; then
  report_args+=(--fail-under-regions "${FAIL_UNDER_REGIONS}")
fi

if [[ "${COVERAGE_CLEAN}" == "1" ]]; then
  "${cargo_llvm_cov[@]}" clean --manifest-path "${BRIDGE_MANIFEST}" --workspace
fi

set +e
"${cargo_llvm_cov[@]}" "${test_args[@]}" "$@"
run_status=$?

"${cargo_llvm_cov[@]}" report \
  "${report_args[@]}" \
  --lcov \
  --output-path "${LCOV_PATH}"
lcov_status=$?

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
    --output-dir "${OUTPUT_DIR}/bridge-html"
  html_status=$?
fi
set -e

cat <<EOF
Bridge coverage artifacts written:
  LCOV:    ${LCOV_PATH}
  Summary: ${SUMMARY_PATH}
EOF

if [[ -n "${COVERAGE_TARGET_LINES}" ]]; then
  echo "  Target:  ${COVERAGE_TARGET_LINES}% line coverage"
fi

if [[ "${HTML}" == "1" ]]; then
  echo "  HTML:    ${OUTPUT_DIR}/bridge-html/index.html"
fi

if [[ "${run_status}" -ne 0 ]]; then
  echo "Bridge coverage test run failed with exit status ${run_status}." >&2
  exit "${run_status}"
fi
if [[ "${lcov_status}" -ne 0 ]]; then
  echo "Bridge LCOV generation failed with exit status ${lcov_status}." >&2
  exit "${lcov_status}"
fi
if [[ "${summary_status}" -ne 0 ]]; then
  echo "Bridge coverage summary generation failed with exit status ${summary_status}." >&2
  exit "${summary_status}"
fi
if [[ "${html_status}" -ne 0 ]]; then
  echo "Bridge coverage HTML generation failed with exit status ${html_status}." >&2
  exit "${html_status}"
fi
