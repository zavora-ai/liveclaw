#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

CONFIG_PATH="liveclaw.toml"
RUN_SETUP=1
RUN_DOCTOR=1
RUN_QUALITY_GATE=0
ONBOARD_ARGS=()

print_usage() {
  cat <<'USAGE'
Usage:
  scripts/bootstrap_local.sh [options]

Options:
  --config PATH                Onboarding config path (default: liveclaw.toml)
  --api-key KEY                API key passed to onboarding
  --model MODEL                Model id passed to onboarding
  --provider-profile PROFILE   Provider profile (legacy|openai|openai_compatible)
  --base-url WS_URL            Realtime provider WebSocket URL
  --gateway-host HOST          Gateway host
  --gateway-port PORT          Gateway port
  --require-pairing BOOL       Pairing requirement (true/false)
  --no-pairing                 Disable pairing
  --force                      Overwrite config if it already exists
  --install-service            Install service after onboarding
  --start-service              Install and start service after onboarding
  --skip-setup                 Skip dev/setup.sh
  --skip-doctor                Skip post-onboarding doctor check
  --quality-gate               Run scripts/run_quality_gate.sh after onboarding
  -h, --help                   Show this help

Notes:
  - This script runs onboarding in --non-interactive mode.
  - Provide LIVECLAW_API_KEY (or --api-key) before running.
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
    --config)
      require_value "$1" "${2:-}"
      CONFIG_PATH="$2"
      shift 2
      ;;
    --api-key|--model|--provider-profile|--base-url|--gateway-host|--gateway-port|--require-pairing)
      require_value "$1" "${2:-}"
      ONBOARD_ARGS+=("$1" "$2")
      shift 2
      ;;
    --no-pairing|--force|--install-service|--start-service)
      ONBOARD_ARGS+=("$1")
      shift
      ;;
    --skip-setup)
      RUN_SETUP=0
      shift
      ;;
    --skip-doctor)
      RUN_DOCTOR=0
      shift
      ;;
    --quality-gate)
      RUN_QUALITY_GATE=1
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

cd "${PROJECT_ROOT}"

if [[ "${RUN_SETUP}" -eq 1 ]]; then
  "${PROJECT_ROOT}/dev/setup.sh"
fi

ONBOARD_CMD=(
  cargo run -p liveclaw-app -- onboard "${CONFIG_PATH}" --non-interactive
)
ONBOARD_CMD+=("${ONBOARD_ARGS[@]}")

echo "Running onboarding command:"
printf '  %q' "${ONBOARD_CMD[@]}"
echo
"${ONBOARD_CMD[@]}"

if [[ "${RUN_DOCTOR}" -eq 1 ]]; then
  echo "Running doctor check..."
  cargo run -p liveclaw-app -- doctor "${CONFIG_PATH}"
fi

if [[ "${RUN_QUALITY_GATE}" -eq 1 ]]; then
  echo "Running quality gate..."
  "${PROJECT_ROOT}/scripts/run_quality_gate.sh"
fi

echo ""
echo "Bootstrap complete."
echo "Next step:"
echo "  cargo run -p liveclaw-app -- ${CONFIG_PATH}"
