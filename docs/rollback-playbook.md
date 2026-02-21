# LiveClaw Rollback Playbook

Date: 2026-02-21
Scope: RC rollback and recovery

## Rollback Triggers

1. Gateway cannot pass `GetGatewayHealth` checks after deploy.
2. Session creation/control path regressions (pair/auth/create/terminate).
3. Channel ingress/scheduler path regressions (`m6_release_flow` failure).
4. Provider/runtime instability with no short-term remediation.

## Immediate Containment

1. Stop accepting new ingress traffic.
2. Cancel active scheduled channel jobs.
3. Preserve logs and recent demo/health outputs for incident analysis.

## Rollback Steps

1. Stop current process.
2. Deploy previously known-good binary/config pair.
3. Restart service with previous release artifacts.
4. Re-run smoke checks:
   ```bash
   scripts/demo/m6_release_flow.sh
   ```
5. Confirm WS client pair/auth/session flow is restored.

## Data and State Notes

1. Preserve memory/artifact files before restart when file-backed storage is enabled.
2. If config changed workspace or storage paths, restore previous path values before restart.
3. Do not delete audit logs during rollback.

## Recovery Exit Criteria

1. Quality/smoke checks pass on rolled-back version.
2. Health metrics stabilize (`active_sessions`, error rate, scheduler count).
3. Incident note is recorded with:
   - rollback start/end time
   - restored commit/tag
   - root-cause follow-up owner
