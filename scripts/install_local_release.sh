#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PREFIX="${HOME}/.local/bin"
TARGET_NAME="liveclaw-app"
SKIP_BUILD=0
FORCE=0

print_usage() {
  cat <<'USAGE'
Usage:
  scripts/install_local_release.sh [options]

Options:
  --prefix PATH        Install directory (default: ~/.local/bin)
  --name NAME          Installed binary name (default: liveclaw-app)
  --skip-build         Do not run cargo build --release
  --force              Overwrite existing installed binary
  -h, --help           Show this help

Examples:
  ./scripts/install_local_release.sh
  ./scripts/install_local_release.sh --prefix /usr/local/bin --name liveclaw
USAGE
}

require_value() {
  local flag="$1"
  local value="${2:-}"
  if [[ -z "${value}" ]]; then
    echo "Missing value for ${flag}" >&2
    exit 1
  fi
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      require_value "$1" "${2:-}"
      PREFIX="$2"
      shift 2
      ;;
    --name)
      require_value "$1" "${2:-}"
      TARGET_NAME="$2"
      shift 2
      ;;
    --skip-build)
      SKIP_BUILD=1
      shift
      ;;
    --force)
      FORCE=1
      shift
      ;;
    -h|--help)
      print_usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      echo "" >&2
      print_usage >&2
      exit 1
      ;;
  esac
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required but was not found in PATH." >&2
  exit 1
fi

cd "${PROJECT_ROOT}"

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
  echo "Building liveclaw-app release binary..."
  cargo build --release -p liveclaw-app
fi

SOURCE_BIN="${PROJECT_ROOT}/target/release/liveclaw-app"
if [[ ! -x "${SOURCE_BIN}" ]]; then
  echo "Release binary not found at ${SOURCE_BIN}" >&2
  echo "Run without --skip-build or build it manually first." >&2
  exit 1
fi

mkdir -p "${PREFIX}"
DEST_BIN="${PREFIX%/}/${TARGET_NAME}"

if [[ -e "${DEST_BIN}" && "${FORCE}" -ne 1 ]]; then
  echo "Destination already exists: ${DEST_BIN}" >&2
  echo "Re-run with --force to overwrite." >&2
  exit 1
fi

install -m 0755 "${SOURCE_BIN}" "${DEST_BIN}"

echo ""
echo "Installed: ${DEST_BIN}"
echo "Verify:"
echo "  ${DEST_BIN} help"
echo ""
echo "If this directory is not in PATH, add it:"
echo "  export PATH=\"${PREFIX}:\$PATH\""
