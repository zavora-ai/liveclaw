#!/usr/bin/env bash
set -euo pipefail

check_var() {
  local name="$1"
  if [[ -n "${!name:-}" ]]; then
    echo "${name}=set"
  else
    echo "${name}=missing"
  fi
}

echo "Provider environment matrix"
check_var "LIVECLAW_API_KEY"
check_var "OPENAI_API_KEY"
check_var "GOOGLE_API_KEY"
check_var "ANTHROPIC_API_KEY"
check_var "DEEPSEEK_API_KEY"
check_var "GROQ_API_KEY"
