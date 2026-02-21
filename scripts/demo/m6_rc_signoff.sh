#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${SCRIPT_DIR}/common.sh"

demo_header "M6 RC Smoke Sign-off"

utc_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
utc_display="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
commit_sha="$(git -C "${ROOT_DIR}" rev-parse HEAD)"
if [[ -n "$(git -C "${ROOT_DIR}" status --porcelain --untracked-files=no)" ]]; then
  commit_sha="${commit_sha}-dirty"
fi

signoff_dir="${ROOT_DIR}/output/rc-signoff/${utc_stamp}"
mkdir -p "${signoff_dir}"

quality_log="${signoff_dir}/quality_gate.log"
run_all_log="${signoff_dir}/demo_run_all.log"
live_m1_log="${signoff_dir}/m1_voice_e2e_live.log"

(
  cd "${ROOT_DIR}"
  scripts/run_quality_gate.sh | tee "${quality_log}"
)

(
  cd "${ROOT_DIR}"
  LIVECLAW_SKIP_GATE=1 scripts/demo/run_all.sh | tee "${run_all_log}"
)

live_m1_status="PREVIOUSLY_VALIDATED"
live_m1_note="Not rerun in this sign-off execution; provider-backed evidence is tracked in docs/closeout-plan.md baseline snapshot."
if [[ "${LIVECLAW_RC_RUN_LIVE_M1:-0}" == "1" ]]; then
  (
    cd "${ROOT_DIR}"
    scripts/demo/m1_voice_e2e_live.sh | tee "${live_m1_log}"
  )
  live_m1_status="PASS"
  live_m1_note="Executed in this sign-off run."
fi

quality_rel="${quality_log#${ROOT_DIR}/}"
run_all_rel="${run_all_log#${ROOT_DIR}/}"
live_m1_rel="${live_m1_log#${ROOT_DIR}/}"

cat > "${ROOT_DIR}/docs/rc-signoff-record.md" <<EOF
# LiveClaw RC Smoke Sign-off Record

Date (UTC): ${utc_display}
Commit: ${commit_sha}

## Command Results

| Check | Status | Evidence |
|---|---|---|
| Quality gate | PASS | scripts/run_quality_gate.sh; log: ${quality_rel} |
| Demo harness | PASS | scripts/demo/run_all.sh; log: ${run_all_rel} |
| M6 release flow | PASS | scripts/demo/m6_release_flow.sh included in run-all log: ${run_all_rel} |
| M1 live provider voice | ${live_m1_status} | scripts/demo/m1_voice_e2e_live.sh; ${live_m1_note} |

## RC Hardening Artifacts

1. Operational limits: docs/rc-operational-limits.md
2. Runbook: docs/release-runbook.md
3. Deployment guide: docs/deployment-guide.md
4. Rollback playbook: docs/rollback-playbook.md

## Notes

1. Set LIVECLAW_RC_RUN_LIVE_M1=1 before running this script to include a live provider-backed M1 pass in the current record.
2. The optional live M1 log path is ${live_m1_rel}.
EOF

demo_pass "RC smoke sign-off record written to docs/rc-signoff-record.md"
