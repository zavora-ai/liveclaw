# LiveClaw Release Runbook

Date: 2026-02-21
Scope: M6 release-candidate operator runbook

## Preconditions

1. `scripts/run_quality_gate.sh` passes.
2. `scripts/demo/run_all.sh` passes.
3. `scripts/demo/m1_voice_e2e_live.sh` passes against the target provider profile.
4. `scripts/demo/m6_release_flow.sh` passes.

## Startup

1. Start gateway/app:
   ```bash
   cargo run -p liveclaw-app -- liveclaw.toml
   ```
2. Capture pairing code from startup logs.
3. Connect the reusable client:
   ```bash
   scripts/ws_client.sh
   ```

## Operational Readiness Checks

1. Pair and authenticate from the WS client.
2. Create a session and run a prompt/tool call.
3. Validate gateway health:
   - WS: `GetGatewayHealth`
   - Confirm: `active_sessions`, `active_ws_bindings`, `active_channel_jobs`.
4. Validate release-flow protocol coverage:
   ```bash
   scripts/demo/m6_release_flow.sh
   ```

## Channel and Scheduler Checks

1. Validate HTTP ingress path (`/channels/webhook`, `/channels/slack/events`, `/channels/telegram/update`) with bearer token.
2. Validate outbound poll path (`/channels/outbound/poll`).
3. Validate scheduler lifecycle:
   - Create: `/channels/jobs/create`
   - List: `/channels/jobs/list`
   - Cancel: `/channels/jobs/cancel`
4. Validate supervised webhook action:
   - `/channels/webhook/supervised-action`
   - Confirm response type `WebhookSupervisedActionResult`.

## Evidence to Store

1. Quality gate output.
2. Demo script outputs (`run_all`, `m1_voice_e2e_live`, `m6_release_flow`).
3. Gateway health snapshot and scheduler job lifecycle logs.
4. Commit SHA and deployment timestamp.
