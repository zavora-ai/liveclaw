# LiveClaw Deployment Guide

Date: 2026-02-21
Scope: Release-candidate deployment procedure

## 1) Build and Gate

1. Validate toolchain and dependencies.
2. Run:
   ```bash
   scripts/run_quality_gate.sh
   scripts/demo/run_all.sh
   ```
3. Build production binary:
   ```bash
   cargo build --release -p liveclaw-app
   ```

## 2) Configuration

1. Copy baseline config (`liveclaw.toml`) and set environment-specific values:
   - `gateway.host` and `gateway.port`
   - `provider.profile` and provider credentials
   - `security.workspace_root`
   - `runtime.kind` (`native` or `docker`)
2. For public binds, explicitly configure the security override guardrails.

## 3) Deploy

1. Install binary and config to target host.
2. Start service (example):
   ```bash
   ./target/release/liveclaw-app /etc/liveclaw/liveclaw.toml
   ```
3. For `runtime.kind=docker`, ensure the worker image is available and the host can start containers.

## 4) Post-Deploy Verification

1. Connect with WS client (`scripts/ws_client.sh`).
2. Pair, authenticate, create session, and validate:
   - `GetDiagnostics`
   - `GetGatewayHealth`
3. Execute:
   ```bash
   scripts/demo/m6_release_flow.sh
   ```
4. Record commit SHA and verification timestamp in release notes.

## 5) Failure Path

1. If any readiness check fails, stop release promotion.
2. Follow `docs/rollback-playbook.md`.
