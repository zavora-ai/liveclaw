import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../models/gateway_models.dart';
import '../models/session_models.dart';
import '../services/audio_capture_service.dart';
import '../services/audio_playback_service.dart';
import '../services/biometric_gate.dart';
import '../services/liveclaw_gateway_client.dart';
import '../services/secure_token_vault.dart';

final gatewayClientProvider = Provider<LiveclawGatewayClient>((ref) {
  final client = LiveclawGatewayClient();
  ref.onDispose(() {
    unawaited(client.dispose());
  });
  return client;
});

final tokenVaultProvider = Provider<SecureTokenVault>((ref) => SecureTokenVault());

final biometricGateProvider = Provider<BiometricGate>((ref) => BiometricGate());

final audioCaptureServiceProvider = Provider<AudioCaptureService>((ref) {
  final service = AudioCaptureService();
  ref.onDispose(() {
    unawaited(service.dispose());
  });
  return service;
});

final audioPlaybackServiceProvider = Provider<AudioPlaybackService>((ref) {
  final service = AudioPlaybackService();
  ref.onDispose(() {
    unawaited(service.dispose());
  });
  return service;
});

final voiceSessionControllerProvider =
    StateNotifierProvider<VoiceSessionController, VoiceSessionState>((ref) {
  return VoiceSessionController(
    gateway: ref.watch(gatewayClientProvider),
    tokenVault: ref.watch(tokenVaultProvider),
    biometricGate: ref.watch(biometricGateProvider),
    audioCapture: ref.watch(audioCaptureServiceProvider),
    audioPlayback: ref.watch(audioPlaybackServiceProvider),
  );
});

class VoiceSessionController extends StateNotifier<VoiceSessionState> {
  VoiceSessionController({
    required LiveclawGatewayClient gateway,
    required SecureTokenVault tokenVault,
    required BiometricGate biometricGate,
    required AudioCaptureService audioCapture,
    required AudioPlaybackService audioPlayback,
  })  : _gateway = gateway,
        _tokenVault = tokenVault,
        _biometricGate = biometricGate,
        _audioCapture = audioCapture,
        _audioPlayback = audioPlayback,
        super(VoiceSessionState.initial()) {
    _gatewayEventsSub = _gateway.events.listen(_handleGatewayEvent);
  }

  final LiveclawGatewayClient _gateway;
  final SecureTokenVault _tokenVault;
  final BiometricGate _biometricGate;
  final AudioCaptureService _audioCapture;
  final AudioPlaybackService _audioPlayback;

  StreamSubscription<GatewayEvent>? _gatewayEventsSub;
  StreamSubscription<Uint8List>? _micStreamSub;

  void setGatewayUrl(String url) {
    state = state.copyWith(gatewayUrl: url.trim(), clearErrorMessage: true);
  }

  void setRole(UserRole role) {
    state = state.copyWith(role: role, clearErrorMessage: true);
  }

  Future<void> secureConnect() async {
    state = state.copyWith(
      stage: ConnectionStage.connecting,
      clearErrorMessage: true,
      clearPrincipalId: true,
      clearSessionId: true,
      paired: false,
      recording: false,
      micLevel: 0,
      transcript: const <TranscriptEntry>[],
      toolActivity: const <ToolActivityEntry>[],
    );

    final unlocked = await _biometricGate.unlock();
    if (!unlocked) {
      _pushError('Biometric/passcode unlock was cancelled');
      return;
    }

    try {
      await _gateway.connect(state.gatewayUrl);
      state = state.copyWith(
        stage: ConnectionStage.connected,
        biometricUnlocked: true,
        clearErrorMessage: true,
      );

      _gateway.requestDiagnostics();
      _gateway.requestGatewayHealth();
      _gateway.ping();

      final savedToken = await _tokenVault.readPairingToken();
      if (savedToken != null && savedToken.isNotEmpty) {
        state = state.copyWith(paired: true);
        _gateway.authenticate(savedToken);
      }
    } catch (error) {
      _pushError('Failed to connect: $error');
    }
  }

  Future<void> pairWithCode(String code) async {
    if (code.trim().length != 6) {
      _pushError('Pairing code should be 6 digits');
      return;
    }

    if (state.stage == ConnectionStage.disconnected || state.stage == ConnectionStage.error) {
      await secureConnect();
    }

    _gateway.pair(code.trim());
  }

  void createSession() {
    _gateway.createSession(
      role: state.role.wireValue,
      enableGraph: state.role == UserRole.supervised,
      instructions:
          'You are Liveclaw mobile mode. Keep audio replies short, action-oriented, and confirm high-risk tools before execution.',
    );
  }

  Future<void> startPushToTalk() async {
    if (state.recording) {
      return;
    }

    final sessionId = state.sessionId;
    if (sessionId == null || sessionId.isEmpty) {
      _pushError('Create a session first');
      return;
    }

    try {
      final stream = await _audioCapture.startCapture();
      _micStreamSub = stream.listen(
        (chunk) {
          _gateway.sendAudioChunk(
            sessionId: sessionId,
            audioBase64: base64Encode(chunk),
          );
          state = state.copyWith(
            micLevel: _estimateMicLevel(chunk),
            recording: true,
            stage: ConnectionStage.streaming,
          );
        },
        onError: (Object error) {
          _pushError('Mic stream failed: $error');
        },
        cancelOnError: false,
      );

      state = state.copyWith(
        recording: true,
        stage: ConnectionStage.streaming,
        clearErrorMessage: true,
      );
    } catch (error) {
      _pushError('Could not start microphone capture: $error');
    }
  }

  Future<void> stopPushToTalk() async {
    if (!state.recording) {
      return;
    }

    final sessionId = state.sessionId;
    if (sessionId == null || sessionId.isEmpty) {
      return;
    }

    await _micStreamSub?.cancel();
    _micStreamSub = null;

    await _audioCapture.stopCapture();
    _gateway.commitAudio(sessionId);
    _gateway.createResponse(sessionId);

    state = state.copyWith(
      recording: false,
      micLevel: 0,
      stage: ConnectionStage.sessionReady,
    );
  }

  void interruptResponse() {
    final sessionId = state.sessionId;
    if (sessionId == null || sessionId.isEmpty) {
      return;
    }
    _gateway.interruptResponse(sessionId);
  }

  void sendPrompt(String prompt) {
    final sessionId = state.sessionId;
    if (sessionId == null || sessionId.isEmpty) {
      _pushError('Create a session before sending prompts');
      return;
    }

    _gateway.prompt(sessionId: sessionId, prompt: prompt);
  }

  void createMemoryPulse() {
    sendPrompt(
      'Generate a live operator brief with 3 sections: immediate action, risk watch, and next spoken question.',
    );
  }

  void terminateSession() {
    final sessionId = state.sessionId;
    if (sessionId == null || sessionId.isEmpty) {
      return;
    }
    _gateway.terminateSession(sessionId);
  }

  void _handleGatewayEvent(GatewayEvent event) {
    switch (event) {
      case PairSuccessEvent(:final token):
        unawaited(_tokenVault.savePairingToken(token));
        state = state.copyWith(
          paired: true,
          clearErrorMessage: true,
        );
        _gateway.authenticate(token);
      case AuthenticatedEvent(:final principalId):
        state = state.copyWith(
          stage: ConnectionStage.authenticated,
          principalId: principalId,
          clearErrorMessage: true,
        );
      case PairFailureEvent(:final reason):
        _pushError('Pair failed: $reason');
      case SessionCreatedEvent(:final sessionId):
        state = state.copyWith(
          stage: ConnectionStage.sessionReady,
          sessionId: sessionId,
          clearErrorMessage: true,
          transcript: <TranscriptEntry>[
            ...state.transcript,
            TranscriptEntry(
              speaker: 'system',
              text: 'Session ready: $sessionId',
              timestamp: DateTime.now(),
              isFinal: true,
            ),
          ],
        );
      case SessionTerminatedEvent():
        state = state.copyWith(
          stage: ConnectionStage.authenticated,
          recording: false,
          micLevel: 0,
          clearSessionId: true,
        );
      case TranscriptUpdateEvent(:final text, :final isFinal):
        if (text.trim().isEmpty) {
          return;
        }
        state = state.copyWith(
          transcript: <TranscriptEntry>[
            ...state.transcript,
            TranscriptEntry(
              speaker: isFinal ? 'agent' : 'live',
              text: text,
              timestamp: DateTime.now(),
              isFinal: isFinal,
            ),
          ],
        );
      case AudioOutputEvent(:final audioBase64):
        unawaited(_audioPlayback.playPcm16FromBase64(audioBase64));
      case SessionToolResultEvent(:final toolName, :final result):
        state = state.copyWith(
          toolActivity: <ToolActivityEntry>[
            ToolActivityEntry(
              toolName: toolName,
              summary: _summarizeToolResult(result),
              timestamp: DateTime.now(),
            ),
            ...state.toolActivity,
          ],
        );
      case DiagnosticsEvent(
          :final protocolVersion,
          :final runtimeKind,
          :final providerKind,
          :final securityAllowPublicBind
        ):
        state = state.copyWith(
          protocolVersion: protocolVersion,
          runtimeKind: runtimeKind,
          providerKind: providerKind,
          publicBindAllowed: securityAllowPublicBind,
        );
      case GatewayHealthEvent(:final activeSessions):
        state = state.copyWith(
          toolActivity: <ToolActivityEntry>[
            ToolActivityEntry(
              toolName: 'health',
              summary: 'Gateway active sessions: $activeSessions',
              timestamp: DateTime.now(),
            ),
            ...state.toolActivity,
          ],
        );
      case ErrorEvent(:final message):
        _pushError(message);
      case UnknownEvent(:final type):
        state = state.copyWith(
          toolActivity: <ToolActivityEntry>[
            ToolActivityEntry(
              toolName: 'event',
              summary: 'Received unmodeled event: $type',
              timestamp: DateTime.now(),
            ),
            ...state.toolActivity,
          ],
        );
      case PongEvent():
        break;
    }
  }

  String _summarizeToolResult(Map<String, dynamic> result) {
    if (result.isEmpty) {
      return 'Tool completed with no payload';
    }

    if (result.containsKey('status')) {
      return 'status=${result['status']}';
    }

    final compact = jsonEncode(result);
    if (compact.length > 120) {
      return '${compact.substring(0, 117)}...';
    }
    return compact;
  }

  double _estimateMicLevel(Uint8List bytes) {
    if (bytes.length < 2) {
      return 0;
    }

    final samples = ByteData.sublistView(bytes);
    var sum = 0.0;
    for (var i = 0; i <= bytes.length - 2; i += 2) {
      final sample = samples.getInt16(i, Endian.little).abs() / 32768.0;
      sum += sample;
    }

    final mean = sum / (bytes.length / 2);
    return mean.clamp(0, 1);
  }

  void _pushError(String message) {
    state = state.copyWith(
      stage: ConnectionStage.error,
      errorMessage: message,
      recording: false,
      micLevel: 0,
    );
  }

  @override
  void dispose() {
    unawaited(_micStreamSub?.cancel());
    unawaited(_gatewayEventsSub?.cancel());
    unawaited(_audioCapture.stopCapture());
    super.dispose();
  }
}
