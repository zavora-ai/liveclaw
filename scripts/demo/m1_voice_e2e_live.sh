#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M1 Voice E2E Live"

require_cmd() {
  local cmd="$1"
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "FAIL: required command not found: ${cmd}" >&2
    exit 1
  fi
}

wait_for_pattern() {
  local file="$1"
  local pattern="$2"
  local timeout_secs="$3"
  local from_line="${4:-1}"
  local deadline=$((SECONDS + timeout_secs))
  while ((SECONDS < deadline)); do
    if [[ -f "${file}" ]] && grep -Fq "${pattern}" <(tail -n +"${from_line}" "${file}"); then
      return 0
    fi
    sleep 1
  done
  return 1
}

wait_for_gateway_health() {
  local health_url="$1"
  local timeout_secs="$2"
  local deadline=$((SECONDS + timeout_secs))
  while ((SECONDS < deadline)); do
    if curl -fsS "${health_url}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

extract_json_field() {
  local json_line="$1"
  local field="$2"
  printf "%s\n" "${json_line}" | sed -n "s/.*\"${field}\":\"\\([^\"]*\\)\".*/\\1/p"
}

latest_line_number() {
  local file="$1"
  if [[ ! -f "${file}" ]]; then
    echo 0
    return
  fi
  wc -l <"${file}" | tr -d " "
}

count_matching_lines() {
  local file="$1"
  local pattern="$2"
  local from_line="${3:-1}"
  if [[ ! -f "${file}" ]]; then
    echo 0
    return
  fi
  tail -n +"${from_line}" "${file}" | grep -Fc "${pattern}" || true
}

ws_to_health_url() {
  local ws_url="$1"
  local scheme="http"
  if [[ "${ws_url}" == wss://* ]]; then
    scheme="https"
  fi
  local without_scheme="${ws_url#ws://}"
  without_scheme="${without_scheme#wss://}"
  local host_port="${without_scheme%%/*}"
  printf "%s://%s/health\n" "${scheme}" "${host_port}"
}

fail_with_context() {
  local message="$1"
  echo "FAIL: ${message}" >&2
  if [[ -f "${WS_OUT}" ]]; then
    echo "---- ws.out (tail) ----" >&2
    tail -n 120 "${WS_OUT}" | sed -E 's/"audio":"[^"]+"/"audio":"<omitted>"/g' >&2 || true
  fi
  if [[ -f "${WS_ERR}" ]]; then
    echo "---- ws.err (tail) ----" >&2
    tail -n 80 "${WS_ERR}" >&2 || true
  fi
  if [[ -f "${APP_LOG}" ]]; then
    echo "---- app.log (tail) ----" >&2
    tail -n 120 "${APP_LOG}" >&2 || true
  fi
  exit 1
}

require_cmd cargo
require_cmd websocat
require_cmd curl
require_cmd base64

if [[ -z "${LIVECLAW_API_KEY:-}" && -z "${OPENAI_API_KEY:-}" ]]; then
  echo "FAIL: LIVECLAW_API_KEY or OPENAI_API_KEY must be set for live voice E2E." >&2
  exit 1
fi

TMP_DIR="$(mktemp -d)"
APP_LOG="${TMP_DIR}/liveclaw-app.log"
WS_IN="${TMP_DIR}/ws.in"
WS_OUT="${TMP_DIR}/ws.out"
WS_ERR="${TMP_DIR}/ws.err"
PCM_FILE="${TMP_DIR}/input.pcm"

APP_PID=""
WS_PID=""

cleanup() {
  if [[ -n "${WS_PID}" ]] && kill -0 "${WS_PID}" >/dev/null 2>&1; then
    kill "${WS_PID}" >/dev/null 2>&1 || true
    wait "${WS_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${APP_PID}" ]] && kill -0 "${APP_PID}" >/dev/null 2>&1; then
    kill "${APP_PID}" >/dev/null 2>&1 || true
    wait "${APP_PID}" >/dev/null 2>&1 || true
  fi
  exec 3>&- || true
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

if [[ -n "${LIVECLAW_E2E_AUDIO_FILE:-}" ]]; then
  cp "${LIVECLAW_E2E_AUDIO_FILE}" "${PCM_FILE}"
else
  "${ROOT_DIR}/scripts/generate_e2e_pcm.sh" \
    "${PCM_FILE}" \
    "${LIVECLAW_E2E_PROMPT:-Hello. Please respond with a short OK.}"
fi

if [[ ! -s "${PCM_FILE}" ]]; then
  echo "FAIL: PCM input file is empty" >&2
  exit 1
fi

APP_CONFIG="${LIVECLAW_E2E_CONFIG:-dev/liveclaw.dev.toml}"
WS_URL="${LIVECLAW_E2E_WS_URL:-ws://127.0.0.1:8420/ws}"
CHUNK_BYTES="${LIVECLAW_E2E_CHUNK_BYTES:-9600}"
CHUNK_DELAY_SECS="${LIVECLAW_E2E_CHUNK_DELAY_SECS:-0.08}"
OUTPUT_TIMEOUT_SECS="${LIVECLAW_E2E_OUTPUT_TIMEOUT_SECS:-180}"
INPUT_SAMPLE_RATE="${LIVECLAW_E2E_SAMPLE_RATE:-24000}"
TRAILING_SILENCE_MS="${LIVECLAW_E2E_TRAILING_SILENCE_MS:-1200}"
FORCE_RESPONSE="${LIVECLAW_E2E_FORCE_RESPONSE:-1}"
USE_EXISTING_GATEWAY="${LIVECLAW_E2E_USE_EXISTING_GATEWAY:-auto}"
AUTH_TOKEN="${LIVECLAW_E2E_TOKEN:-}"
PAIR_CODE_OVERRIDE="${LIVECLAW_E2E_PAIR_CODE:-}"
HEALTH_URL="$(ws_to_health_url "${WS_URL}")"
SESSION_INSTRUCTIONS="${LIVECLAW_E2E_SESSION_INSTRUCTIONS:-Reply with one short acknowledgment only.}"

gateway_running=0
if curl -fsS "${HEALTH_URL}" >/dev/null 2>&1; then
  gateway_running=1
fi

case "${USE_EXISTING_GATEWAY}" in
  auto)
    if [[ "${gateway_running}" == "1" ]]; then
      echo "Reusing existing gateway at ${WS_URL}"
    else
      (
        cd "${ROOT_DIR}"
        RUST_LOG="${RUST_LOG:-info}" cargo run -p liveclaw-app -- "${APP_CONFIG}" >"${APP_LOG}" 2>&1
      ) &
      APP_PID="$!"
      if ! wait_for_gateway_health "${HEALTH_URL}" 60; then
        fail_with_context "gateway did not start within timeout"
      fi
    fi
    ;;
  always)
    if [[ "${gateway_running}" != "1" ]]; then
      fail_with_context "LIVECLAW_E2E_USE_EXISTING_GATEWAY=always but no gateway is reachable at ${HEALTH_URL}"
    fi
    echo "Using existing gateway at ${WS_URL}"
    ;;
  never)
    (
      cd "${ROOT_DIR}"
      RUST_LOG="${RUST_LOG:-info}" cargo run -p liveclaw-app -- "${APP_CONFIG}" >"${APP_LOG}" 2>&1
    ) &
    APP_PID="$!"
    if ! wait_for_gateway_health "${HEALTH_URL}" 60; then
      fail_with_context "gateway did not start within timeout"
    fi
    ;;
  *)
    fail_with_context "invalid LIVECLAW_E2E_USE_EXISTING_GATEWAY='${USE_EXISTING_GATEWAY}' (expected auto|always|never)"
    ;;
esac

if ! curl -fsS "${HEALTH_URL}" >/dev/null 2>&1; then
  fail_with_context "gateway health endpoint is not reachable at ${HEALTH_URL}"
fi

mkfifo "${WS_IN}"
(cat "${WS_IN}" | websocat -t "${WS_URL}" >"${WS_OUT}" 2>"${WS_ERR}") &
WS_PID="$!"
exec 3>"${WS_IN}"

send_json() {
  local json="$1"
  printf "%s\n" "${json}" >&3
}

send_and_expect() {
  local payload="$1"
  local pattern="$2"
  local timeout_secs="$3"
  local start_line
  start_line=$(( $(latest_line_number "${WS_OUT}") + 1 ))
  send_json "${payload}"
  if ! wait_for_pattern "${WS_OUT}" "${pattern}" "${timeout_secs}" "${start_line}"; then
    fail_with_context "did not receive expected response pattern '${pattern}' within ${timeout_secs}s"
  fi
}

# Check health first; if pairing is required, authenticate via pairing code from app logs.
send_and_expect '{"type":"GetGatewayHealth"}' '"type":"GatewayHealth"' 20

health_line="$(grep '"type":"GatewayHealth"' "${WS_OUT}" | tail -n 1 || true)"
if [[ "${health_line}" == *'"require_pairing":true'* ]]; then
  token="${AUTH_TOKEN}"
  if [[ -z "${token}" ]]; then
    pair_code="${PAIR_CODE_OVERRIDE}"
    if [[ -z "${pair_code}" ]] && [[ -n "${APP_PID}" ]]; then
      if ! wait_for_pattern "${APP_LOG}" "Pairing code:" 20; then
        fail_with_context "pairing required but pairing code was not found in app logs"
      fi
      pair_code="$(grep 'Pairing code:' "${APP_LOG}" | tail -n 1 | sed -n 's/.*Pairing code: \([0-9][0-9][0-9][0-9][0-9][0-9]\).*/\1/p')"
    fi

    if [[ -z "${pair_code}" ]]; then
      fail_with_context "pairing is required. Provide LIVECLAW_E2E_TOKEN or LIVECLAW_E2E_PAIR_CODE when using an existing gateway."
    fi

    send_and_expect "{\"type\":\"Pair\",\"code\":\"${pair_code}\"}" '"type":"PairSuccess"' 20
    pair_line="$(grep '"type":"PairSuccess"' "${WS_OUT}" | tail -n 1 || true)"
    token="$(extract_json_field "${pair_line}" "token")"
    if [[ -z "${token}" ]]; then
      fail_with_context "failed to parse token from PairSuccess response"
    fi
  fi

  send_and_expect "{\"type\":\"Authenticate\",\"token\":\"${token}\"}" '"type":"Authenticated"' 20
fi

escaped_instructions="${SESSION_INSTRUCTIONS//\\/\\\\}"
escaped_instructions="${escaped_instructions//\"/\\\"}"
create_session_payload="$(printf '{"type":"CreateSession","config":{"role":"full","instructions":"%s"}}' "${escaped_instructions}")"
send_and_expect "${create_session_payload}" '"type":"SessionCreated"' 20

created_line="$(grep '"type":"SessionCreated"' "${WS_OUT}" | tail -n 1 || true)"
session_id="$(extract_json_field "${created_line}" "session_id")"
if [[ -z "${session_id}" ]]; then
  fail_with_context "failed to parse session_id from SessionCreated response"
fi

audio_accepted_pattern="\"type\":\"AudioAccepted\",\"session_id\":\"${session_id}\""
transcript_pattern="\"type\":\"TranscriptUpdate\",\"session_id\":\"${session_id}\""
audio_output_pattern="\"type\":\"AudioOutput\",\"session_id\":\"${session_id}\""
terminated_pattern="\"type\":\"SessionTerminated\",\"session_id\":\"${session_id}\""

stream_start_line=$(( $(latest_line_number "${WS_OUT}") + 1 ))
split -b "${CHUNK_BYTES}" -d -a 5 "${PCM_FILE}" "${TMP_DIR}/chunk_"
for chunk_file in "${TMP_DIR}"/chunk_*; do
  [[ -f "${chunk_file}" ]] || continue
  chunk_b64="$(base64 < "${chunk_file}" | tr -d '\n')"
  if [[ -z "${chunk_b64}" ]]; then
    continue
  fi
  send_json "{\"type\":\"SessionAudio\",\"session_id\":\"${session_id}\",\"audio\":\"${chunk_b64}\"}"
  sleep "${CHUNK_DELAY_SECS}"
done

if [[ "${TRAILING_SILENCE_MS}" =~ ^[0-9]+$ ]] && [[ "${INPUT_SAMPLE_RATE}" =~ ^[0-9]+$ ]]; then
  trailing_silence_bytes=$((INPUT_SAMPLE_RATE * TRAILING_SILENCE_MS / 1000 * 2))
  remaining_silence="${trailing_silence_bytes}"
  while ((remaining_silence > 0)); do
    chunk_len="${CHUNK_BYTES}"
    if ((remaining_silence < CHUNK_BYTES)); then
      chunk_len="${remaining_silence}"
    fi
    silence_b64="$(head -c "${chunk_len}" /dev/zero | base64 | tr -d '\n')"
    send_json "{\"type\":\"SessionAudio\",\"session_id\":\"${session_id}\",\"audio\":\"${silence_b64}\"}"
    remaining_silence=$((remaining_silence - chunk_len))
    sleep "${CHUNK_DELAY_SECS}"
  done
fi

if ! wait_for_pattern "${WS_OUT}" "${audio_accepted_pattern}" 20 "${stream_start_line}"; then
  fail_with_context "audio was sent but AudioAccepted was not observed for session ${session_id}"
fi

if [[ "${FORCE_RESPONSE}" != "0" ]]; then
  send_and_expect "{\"type\":\"SessionAudioCommit\",\"session_id\":\"${session_id}\"}" "\"type\":\"AudioCommitted\",\"session_id\":\"${session_id}\"" 20
  send_and_expect "{\"type\":\"SessionResponseCreate\",\"session_id\":\"${session_id}\"}" "\"type\":\"ResponseCreateAccepted\",\"session_id\":\"${session_id}\"" 20
fi

if ! wait_for_pattern "${WS_OUT}" "${transcript_pattern}" "${OUTPUT_TIMEOUT_SECS}" "${stream_start_line}"; then
  fail_with_context "transcript output not observed within ${OUTPUT_TIMEOUT_SECS}s for session ${session_id}"
fi

if ! wait_for_pattern "${WS_OUT}" "${audio_output_pattern}" "${OUTPUT_TIMEOUT_SECS}" "${stream_start_line}"; then
  fail_with_context "audio output not observed within ${OUTPUT_TIMEOUT_SECS}s for session ${session_id}"
fi

send_and_expect "{\"type\":\"TerminateSession\",\"session_id\":\"${session_id}\"}" "${terminated_pattern}" 20

transcript_count="$(count_matching_lines "${WS_OUT}" "${transcript_pattern}" "${stream_start_line}")"
audio_count="$(count_matching_lines "${WS_OUT}" "${audio_output_pattern}" "${stream_start_line}")"
demo_pass "Live voice roundtrip passed (session_id=${session_id}, transcripts=${transcript_count}, audio_events=${audio_count})"
