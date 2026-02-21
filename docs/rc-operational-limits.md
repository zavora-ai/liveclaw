# LiveClaw RC Operational Limits

Date: 2026-02-21
Scope: Release-candidate limits backed by automated evidence

## Session Control Plane Limits

| Limit | Value | Evidence |
|---|---|---|
| Concurrent `CreateSession` burst (minimum verified) | 128 requests in one burst | `liveclaw-gateway/src/server.rs` `test_concurrent_create_session_burst_updates_health_and_ownership` |
| Active session ownership map after burst | 128 entries | same test asserts `active_sessions == 128` |
| Active WS bindings after burst | 128 standard + 128 priority | same test asserts `active_ws_bindings == 128` and `active_priority_bindings == 128` |

## How To Re-verify

1. Run focused limit evidence:
   ```bash
   cargo test -p liveclaw-gateway --lib server::tests::test_concurrent_create_session_burst_updates_health_and_ownership -- --exact
   ```
2. Run M6 RC flow (includes the same burst check):
   ```bash
   scripts/demo/m6_release_flow.sh
   ```

## Operator Policy

1. Treat 128 concurrent new-session requests as the current RC minimum validated burst.
2. Keep production autoscaling and ingress throttling configured so startup bursts do not exceed this value until a higher validated limit is published.
