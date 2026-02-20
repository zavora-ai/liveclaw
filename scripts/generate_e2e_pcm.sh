#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 ]]; then
  echo "Usage: $0 <output_pcm_file> [prompt]" >&2
  exit 1
fi

out_file="$1"
prompt="${2:-Hello. Please respond with OK.}"
tts_model="${LIVECLAW_E2E_TTS_MODEL:-gpt-4o-mini-tts}"
tts_voice="${LIVECLAW_E2E_TTS_VOICE:-alloy}"
api_key="${LIVECLAW_API_KEY:-${OPENAI_API_KEY:-}}"

if [[ -z "${api_key}" ]]; then
  echo "FAIL: LIVECLAW_API_KEY or OPENAI_API_KEY must be set to generate test audio." >&2
  exit 1
fi

mkdir -p "$(dirname "${out_file}")"

escaped_prompt="${prompt//\\/\\\\}"
escaped_prompt="${escaped_prompt//\"/\\\"}"

payload="$(printf '{"model":"%s","voice":"%s","format":"pcm","input":"%s"}' \
  "${tts_model}" \
  "${tts_voice}" \
  "${escaped_prompt}")"

curl -sS -X POST "https://api.openai.com/v1/audio/speech" \
  -H "Authorization: Bearer ${api_key}" \
  -H "Content-Type: application/json" \
  -d "${payload}" \
  --output "${out_file}"

if [[ ! -s "${out_file}" ]]; then
  echo "FAIL: Generated audio file is empty: ${out_file}" >&2
  exit 1
fi

echo "Generated PCM audio clip: ${out_file}"
