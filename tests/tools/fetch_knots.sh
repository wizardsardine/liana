#!/usr/bin/env bash
# Fetch a Bitcoin Knots release for this host, verify it against the
# PGP-signed SHA256SUMS with tests/tools/knots_verify (same trust anchor as the
# desktop installer), and print the path of the extracted `bitcoind`.
#
#   tests/tools/fetch_knots.sh <version> [cache-dir]
#   e.g. tests/tools/fetch_knots.sh 29.5.1.knots20260508
#
# Nothing is extracted unless verification succeeds. The cache dir defaults to
# tests/tools/knots/ (gitignored) and keeps the archive, the manifest and its
# signature next to the extraction.
#
# Reuse is never taken on trust. There is no "verified" marker: on every run
# the cached archive is verified again with the *current* knots_verify (so a
# change to the verifier or to the vendored signing key is applied to archives
# downloaded under the old one), and the extracted `bitcoind` is reused only if
# its SHA-256 equals the digest of the archive's own `bin/bitcoind` member,
# recomputed by streaming it out of the just-verified archive. A cached file
# that no longer verifies is discarded and fetched once more; if that copy does
# not verify either, the script fails and prints no path.
#
# KNOTS_TEST_BASE_URL (a file:// release directory) and KNOTS_TEST_VERIFY_PATH
# (a stand-in verifier) exist for tests/test_btcb2_tools.py and are honoured
# only together with KNOTS_TEST_MODE=1; any of them set otherwise makes the
# script exit before downloading or verifying anything, so an inherited
# variable can neither redirect the download nor name a verifier. In test
# mode the stand-in verifier is mandatory: the real one is never searched for,
# so a test cannot silently run against a repo-local build.
#
# What that does and does not guarantee: in real runs the verifier is no
# longer selectable by a dedicated variable, but it is *located* through the
# Cargo target dir — "${CARGO_TARGET_DIR:-<this dir>/knots_verify/target}/
# release/knots_verify" first, the repo-local build second — and the caller
# controls CARGO_TARGET_DIR. A stale or foreign knots_verify there is used.
# (A relative CARGO_TARGET_DIR is resolved against the caller's cwd, before
# the script changes directory.)
# The script therefore prints the verifier it resolved on stderr on every run,
# so the CI log and a local run both show which binary vouched for the
# archives. The trust anchor inside the verifier (the vendored Knots signing
# key and its pinned fingerprint) is compiled into that binary.
set -euo pipefail

version="${1:?usage: fetch_knots.sh <version> [cache-dir]}"
tools_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cache_dir="${2:-$tools_dir/knots}"
major="${version%%.*}"

test_mode="${KNOTS_TEST_MODE:-0}"
if [ -n "${KNOTS_TEST_BASE_URL:-}${KNOTS_TEST_VERIFY_PATH:-}" ] && [ "$test_mode" != "1" ]; then
  echo "KNOTS_TEST_* overrides are only accepted with KNOTS_TEST_MODE=1; refusing to fetch or verify with an overridden release directory or verifier" >&2
  exit 2
fi
if [ "$test_mode" = "1" ]; then
  echo "TEST MODE: release directory/verifier overridden; this is not a verified bitcoinknots.org release" >&2
  if [ -z "${KNOTS_TEST_BASE_URL:-}" ] || [ -z "${KNOTS_TEST_VERIFY_PATH:-}" ]; then
    echo "KNOTS_TEST_MODE=1 requires both KNOTS_TEST_BASE_URL and KNOTS_TEST_VERIFY_PATH" >&2
    exit 2
  fi
fi

# The cache dir is used from inside itself later (`cd "$dest"`), so it must be
# absolute whatever the caller passed; the printed path must work from any cwd.
mkdir -p "$cache_dir"
cache_dir="$(cd "$cache_dir" && pwd -P)"

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)   suffix="arm64-apple-darwin" ;;
  Darwin-x86_64)  suffix="x86_64-apple-darwin" ;;
  Linux-x86_64)   suffix="x86_64-linux-gnu" ;;
  Linux-aarch64)  suffix="aarch64-linux-gnu" ;;
  *) echo "unsupported host $(uname -s)-$(uname -m)" >&2; exit 2 ;;
esac

base_url="https://bitcoinknots.org/files/${major}.x/${version}"
[ "$test_mode" = "1" ] && base_url="$KNOTS_TEST_BASE_URL"
archive="bitcoin-${version}-${suffix}.tar.gz"
member="bitcoin-${version}/bin/bitcoind"
dest="$cache_dir/$version"
bitcoind="$dest/$member"

verify_bin=""
if [ "$test_mode" = "1" ]; then
  verify_bin="$KNOTS_TEST_VERIFY_PATH"
  if [ ! -x "$verify_bin" ]; then
    echo "KNOTS_TEST_VERIFY_PATH is not an executable: $verify_bin" >&2
    exit 2
  fi
else
  for candidate in \
      "${CARGO_TARGET_DIR:-$tools_dir/knots_verify/target}/release/knots_verify" \
      "$tools_dir/knots_verify/target/release/knots_verify"; do
    if [ -x "$candidate" ]; then verify_bin="$candidate"; break; fi
  done
  if [ -z "$verify_bin" ]; then
    echo "knots_verify not built: (cd tests/tools/knots_verify && cargo build --release)" >&2
    exit 2
  fi
  # The candidate was tested against the caller's cwd, but it is invoked after
  # `cd "$dest"` below: a relative CARGO_TARGET_DIR must be made absolute here
  # or the verifier it selected is not found at verification time.
  case "$verify_bin" in
    /*) ;;
    *) verify_bin="$(cd "$(dirname "$verify_bin")" && pwd -P)/$(basename "$verify_bin")" ;;
  esac
fi
echo "verifier: $verify_bin" >&2

sha256_of() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
  else shasum -a 256 "$1" | cut -d' ' -f1; fi
}
sha256_stdin() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | cut -d' ' -f1
  else shasum -a 256 | cut -d' ' -f1; fi
}

download_missing() {
  for f in "$archive" SHA256SUMS SHA256SUMS.asc; do
    # Download to a temporary name and rename on success: a transfer that fails
    # after its retries must not leave a partial file that a later run accepts.
    if [ ! -f "$f" ]; then
      rm -f "$f.part"
      curl -fsSL --retry 3 -o "$f.part" "$base_url/$f"
      mv "$f.part" "$f"
    fi
  done
}

mkdir -p "$dest"
cd "$dest"
download_missing

# Verify with the current verifier on every run, cached or fresh. A cached set
# that fails is thrown away and fetched once more; a fresh set that fails is
# final.
if ! "$verify_bin" "$archive" SHA256SUMS SHA256SUMS.asc >&2; then
  echo "cached release files for $version no longer verify; fetching them again" >&2
  rm -f "$archive" SHA256SUMS SHA256SUMS.asc "$archive.part"
  rm -rf "bitcoin-$version"
  download_missing
  "$verify_bin" "$archive" SHA256SUMS SHA256SUMS.asc >&2
fi

# Reuse the extraction only if its bitcoind is byte-identical to the member of
# the archive that just verified; otherwise extract again from that archive.
expected="$(tar -xzOf "$archive" "$member" | sha256_stdin)"
if [ ! -x "$bitcoind" ] || [ "$(sha256_of "$bitcoind")" != "$expected" ]; then
  rm -rf "bitcoin-$version"
  tar -xzf "$archive"
  [ -x "$bitcoind" ] || { echo "no $member in $archive" >&2; exit 1; }
  [ "$(sha256_of "$bitcoind")" = "$expected" ] || { echo "extracted bitcoind does not match the archive member" >&2; exit 1; }
fi
echo "$bitcoind"
