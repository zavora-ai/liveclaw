# Liveclaw Mobile (Flutter)

Secure, audio-first mobile experience for Liveclaw. This app is designed as a **voice command deck** instead of a chat UI.

## What is unique here

- **Trust Ring UX**: live security posture in one view (pairing, biometric unlock, runtime profile, provider, public-bind risk).
- **Resonance Orb**: hold-to-speak interaction with live amplitude pulse tuned for barge-in workflows.
- **Action Lane**: dedicated stream of tool executions + gateway health/diagnostic events, separate from transcript.
- **Memory Pulse**: one-tap prompt pattern that generates operator-ready summaries for next spoken action.

## Security posture

- Pairing token stored in platform secure storage (`flutter_secure_storage`).
- Session unlock gated by biometric/passcode (`local_auth`).
- Explicit role selection (`readonly` / `supervised` / `full`) before session creation.
- Gateway diagnostics surfaced in-app for runtime/provider/public-bind verification.

## Protocol support

Implemented around `liveclaw-gateway` WebSocket protocol messages:

- Outbound: `Pair`, `Authenticate`, `CreateSession`, `SessionAudio`, `SessionAudioCommit`, `SessionResponseCreate`, `SessionResponseInterrupt`, `SessionPrompt`, `GetDiagnostics`, `GetGatewayHealth`, `Ping`
- Inbound: `PairSuccess`, `Authenticated`, `SessionCreated`, `SessionTerminated`, `TranscriptUpdate`, `AudioOutput`, `SessionToolResult`, `Diagnostics`, `GatewayHealth`, `Error`, `Pong`

## Run locally

1. Install Flutter SDK (3.4+).
2. From this folder:

```bash
flutter create .
flutter pub get
flutter run
```

Default gateway URL is `ws://127.0.0.1:8080/ws`.

## Notes

- Mobile app sends microphone PCM16 mono 24k chunks and expects `AudioOutput` payloads as base64 PCM16 mono 24k.
- Audio playback wraps PCM into WAV in-memory before playback for device compatibility.
- If your gateway runs on another host/device, update the WebSocket URL in-app.
- Ensure microphone/network permissions are enabled in generated platform projects:
  - Android: `RECORD_AUDIO`, `INTERNET`
  - iOS: `NSMicrophoneUsageDescription`, network access as required
