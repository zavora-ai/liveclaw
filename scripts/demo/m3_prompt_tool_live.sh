#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M3 Prompt Tool Live (with model output)"

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

latest_line_number() {
  local file="$1"
  if [[ ! -f "${file}" ]]; then
    echo 0
    return
  fi
  wc -l <"${file}" | tr -d " "
}

extract_json_field() {
  local json_line="$1"
  local field="$2"
  printf "%s\n" "${json_line}" | sed -n "s/.*\"${field}\":\"\\([^\"]*\\)\".*/\\1/p"
}

json_escape() {
  local val="$1"
  val="${val//\\/\\\\}"
  val="${val//\"/\\\"}"
  val="${val//$'\n'/\\n}"
  printf "%s" "${val}"
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
    fail_with_context "did not receive expected pattern '${pattern}' within ${timeout_secs}s"
  fi
}

require_cmd cargo
require_cmd curl
require_cmd websocat

TMP_DIR="$(mktemp -d)"
APP_LOG="${TMP_DIR}/liveclaw-app.log"
WS_IN="${TMP_DIR}/ws.in"
WS_OUT="${TMP_DIR}/ws.out"
WS_ERR="${TMP_DIR}/ws.err"
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

APP_CONFIG="${LIVECLAW_PROMPT_CONFIG:-liveclaw.toml}"
WS_URL="${LIVECLAW_PROMPT_WS_URL:-ws://127.0.0.1:8420/ws}"
HEALTH_URL="$(ws_to_health_url "${WS_URL}")"
OUTPUT_TIMEOUT_SECS="${LIVECLAW_PROMPT_OUTPUT_TIMEOUT_SECS:-120}"
USE_EXISTING_GATEWAY="${LIVECLAW_PROMPT_USE_EXISTING_GATEWAY:-auto}"
AUTH_TOKEN="${LIVECLAW_PROMPT_TOKEN:-}"
PAIR_CODE_OVERRIDE="${LIVECLAW_PROMPT_PAIR_CODE:-}"
SESSION_INSTRUCTIONS="${LIVECLAW_PROMPT_SESSION_INSTRUCTIONS:-You are a tool-first assistant. For math, UTC time/date, or workspace file requests, call a tool before answering.}"
PROMPT_TEXT="${LIVECLAW_PROMPT_TEXT:-Use add_numbers with a=12 and b=30, and answer with the sum only.}"

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
      fail_with_context "LIVECLAW_PROMPT_USE_EXISTING_GATEWAY=always but no gateway is reachable at ${HEALTH_URL}"
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
    fail_with_context "invalid LIVECLAW_PROMPT_USE_EXISTING_GATEWAY='${USE_EXISTING_GATEWAY}' (expected auto|always|never)"
    ;;
esac

mkfifo "${WS_IN}"
(cat "${WS_IN}" | websocat -t "${WS_URL}" >"${WS_OUT}" 2>"${WS_ERR}") &
WS_PID="$!"
exec 3>"${WS_IN}"

send_and_expect '{"type":"GetGatewayHealth"}' '"type":"GatewayHealth"' 20
health_line="$(grep '"type":"GatewayHealth"' "${WS_OUT}" | tail -n 1 || true)"

if [[ "${health_line}" == *'"require_pairing":true'* ]]; then
  token="${AUTH_TOKEN}"
  if [[ -z "${token}" ]]; then
    pair_code="${PAIR_CODE_OVERRIDE}"
    if [[ -z "${pair_code}" ]] && [[ -n "${APP_PID}" ]]; then
      if ! wait_for_pattern "${APP_LOG}" "Pairing code:" 20; then
        fail_with_context "pairing required but pairing code not found in app logs"
      fi
      pair_code="$(grep 'Pairing code:' "${APP_LOG}" | tail -n 1 | sed -n 's/.*Pairing code: \([0-9][0-9][0-9][0-9][0-9][0-9]\).*/\1/p')"
    fi
    if [[ -z "${pair_code}" ]]; then
      fail_with_context "pairing is required. Provide LIVECLAW_PROMPT_TOKEN or LIVECLAW_PROMPT_PAIR_CODE."
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

escaped_instructions="$(json_escape "${SESSION_INSTRUCTIONS}")"
create_session_payload="$(printf '{"type":"CreateSession","config":{"role":"full","instructions":"%s"}}' "${escaped_instructions}")"
send_and_expect "${create_session_payload}" '"type":"SessionCreated"' 20
created_line="$(grep '"type":"SessionCreated"' "${WS_OUT}" | tail -n 1 || true)"
session_id="$(extract_json_field "${created_line}" "session_id")"
if [[ -z "${session_id}" ]]; then
  fail_with_context "failed to parse session_id"
fi

escaped_prompt="$(json_escape "${PROMPT_TEXT}")"
prompt_start_line=$(( $(latest_line_number "${WS_OUT}") + 1 ))
send_and_expect "{\"type\":\"SessionPrompt\",\"session_id\":\"${session_id}\",\"prompt\":\"${escaped_prompt}\",\"create_response\":true}" "\"type\":\"PromptAccepted\",\"session_id\":\"${session_id}\"" 20

transcript_pattern="\"type\":\"TranscriptUpdate\",\"session_id\":\"${session_id}\""
if ! wait_for_pattern "${WS_OUT}" "${transcript_pattern}" "${OUTPUT_TIMEOUT_SECS}" "${prompt_start_line}"; then
  fail_with_context "model transcript was not observed within ${OUTPUT_TIMEOUT_SECS}s"
fi

echo "---- Model Output (TranscriptUpdate) ----"
if command -v jq >/dev/null 2>&1; then
  model_output="$(
    jq -Rrs --arg sid "${session_id}" '
      split("\n")
      | map(fromjson? | select(.type == "TranscriptUpdate" and .session_id == $sid) | .text // "")
      | join("")
    ' "${WS_OUT}"
  )"
  if [[ -n "${model_output}" ]]; then
    printf "%s\n" "${model_output}"
  else
    grep "${transcript_pattern}" "${WS_OUT}" | tail -n 12 | sed -E 's/.*"text":"([^"]*)".*/\1/' | sed 's/\\n/\n/g'
  fi
else
  grep "${transcript_pattern}" "${WS_OUT}" | tail -n 12 | sed -E 's/.*"text":"([^"]*)".*/\1/' | sed 's/\\n/\n/g'
fi
echo "----------------------------------------"

send_and_expect "{\"type\":\"TerminateSession\",\"session_id\":\"${session_id}\"}" "\"type\":\"SessionTerminated\",\"session_id\":\"${session_id}\"" 20
demo_pass "Prompt tool live demo passed (session_id=${session_id})"
