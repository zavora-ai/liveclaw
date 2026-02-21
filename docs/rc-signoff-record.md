# LiveClaw RC Smoke Sign-off Record

Date (UTC): 2026-02-21T14:11:27Z
Commit: 977e7c74ced4f2c558d54b9556b6dcedf40e584c-dirty

## Command Results

| Check | Status | Evidence |
|---|---|---|
| Quality gate | PASS | scripts/run_quality_gate.sh; log: output/rc-signoff/20260221T141127Z/quality_gate.log |
| Demo harness | PASS | scripts/demo/run_all.sh; log: output/rc-signoff/20260221T141127Z/demo_run_all.log |
| M6 release flow | PASS | scripts/demo/m6_release_flow.sh included in run-all log: output/rc-signoff/20260221T141127Z/demo_run_all.log |
| M1 live provider voice | PREVIOUSLY_VALIDATED | scripts/demo/m1_voice_e2e_live.sh; Not rerun in this sign-off execution; provider-backed evidence is tracked in docs/closeout-plan.md baseline snapshot. |

## RC Hardening Artifacts

1. Operational limits: docs/rc-operational-limits.md
2. Runbook: docs/release-runbook.md
3. Deployment guide: docs/deployment-guide.md
4. Rollback playbook: docs/rollback-playbook.md

## Notes

1. Set LIVECLAW_RC_RUN_LIVE_M1=1 before running this script to include a live provider-backed M1 pass in the current record.
2. The optional live M1 log path is output/rc-signoff/20260221T141127Z/m1_voice_e2e_live.log.
