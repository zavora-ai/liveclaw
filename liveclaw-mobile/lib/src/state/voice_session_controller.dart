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

final tokenVaultProvider =
    Provider<SecureTokenVault>((ref) => SecureTokenVault());

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
    NotifierProvider<VoiceSessionController, VoiceSessionState>(
  VoiceSessionController.new,
);

class VoiceSessionController extends Notifier<VoiceSessionState> {
  late LiveclawGatewayClient _gateway;
  late SecureTokenVault _tokenVault;
  late BiometricGate _biometricGate;
  late AudioCaptureService _audioCapture;
  late AudioPlaybackService _audioPlayback;

  StreamSubscription<GatewayEvent>? _gatewayEventsSub;
  StreamSubscription<Uint8List>? _micStreamSub;

  @override
  VoiceSessionState build() {
    _gateway = ref.read(gatewayClientProvider);
    _tokenVault = ref.read(tokenVaultProvider);
    _biometricGate = ref.read(biometricGateProvider);
    _audioCapture = ref.read(audioCaptureServiceProvider);
    _audioPlayback = ref.read(audioPlaybackServiceProvider);
    _gatewayEventsSub = _gateway.events.listen(_handleGatewayEvent);
    ref.onDispose(() {
      unawaited(_micStreamSub?.cancel());
      unawaited(_gatewayEventsSub?.cancel());
      unawaited(_audioCapture.stopCapture());
    });
    return VoiceSessionState.initial();
  }

  void setGatewayUrl(String url) {
    state = state.copyWith(
      gatewayUrl: url.trim(),
      clearErrorMessage: true,
      statusMessage: 'Gateway URL updated.',
    );
  }

  void setRole(UserRole role) {
    state = state.copyWith(
      role: role,
      clearErrorMessage: true,
      statusMessage: 'Role set to ${role.label}.',
    );
  }

  void setDeviceAuthEnabled(bool enabled) {
    state = state.copyWith(
      deviceAuthEnabled: enabled,
      biometricUnlocked: enabled ? state.biometricUnlocked : false,
      clearErrorMessage: true,
      statusMessage: enabled
          ? 'Device auth enabled. Connect will require Face ID/passcode.'
          : 'Device auth disabled. URL + pair code will be used.',
    );
  }

  Future<bool> secureConnect() async {
    if (state.stage == ConnectionStage.connecting) {
      return false;
    }

    state = state.copyWith(
      stage: ConnectionStage.connecting,
      clearErrorMessage: true,
      recording: false,
      micLevel: 0,
      statusMessage: state.deviceAuthEnabled
          ? 'Verifying device authentication...'
          : 'Connecting to gateway...',
    );

    if (state.deviceAuthEnabled) {
      final unlocked = await _biometricGate.unlock();
      if (!unlocked) {
        state = state.copyWith(
          stage: ConnectionStage.disconnected,
          biometricUnlocked: false,
          errorMessage: 'Device auth was cancelled',
          recording: false,
          micLevel: 0,
          statusMessage: 'Connection cancelled: device auth was not completed.',
        );
        return false;
      }
      state = state.copyWith(
        biometricUnlocked: true,
        statusMessage: 'Device auth passed. Connecting to gateway...',
      );
    } else {
      state = state.copyWith(biometricUnlocked: false);
    }

    try {
      await _gateway.connect(state.gatewayUrl);
      state = state.copyWith(
        stage: ConnectionStage.connected,
        clearErrorMessage: true,
        statusMessage: 'Connected. Checking gateway health...',
      );

      _gateway.requestDiagnostics();
      _gateway.requestGatewayHealth();
      _gateway.ping();

      final savedToken = await _tokenVault.readPairingToken();
      if (savedToken != null && savedToken.isNotEmpty) {
        state = state.copyWith(
          paired: true,
          statusMessage: 'Saved token found. Authenticating...',
        );
        _gateway.authenticate(savedToken);
      } else {
        state = state.copyWith(
          statusMessage: 'Connected. Enter your 6-digit pairing code.',
        );
      }
      return true;
    } catch (error) {
      _pushError('Failed to connect: $error');
      return false;
    }
  }

  Future<void> pairWithCode(String code) async {
    if (code.trim().length != 6) {
      _pushError('Pairing code should be exactly 6 digits');
      return;
    }

    if (state.stage == ConnectionStage.connecting) {
      state = state.copyWith(
        statusMessage: 'Connection in progress. Please wait...',
      );
      return;
    }

    if (state.stage == ConnectionStage.disconnected ||
        state.stage == ConnectionStage.error) {
      state = state.copyWith(
        statusMessage: 'Connecting before pairing...',
        clearErrorMessage: true,
      );
      final connected = await secureConnect();
      if (!connected) {
        return;
      }
    }

    if (state.stage == ConnectionStage.disconnected ||
        state.stage == ConnectionStage.connecting ||
        state.stage == ConnectionStage.error) {
      _pushError('Connect to gateway before pairing');
      return;
    }

    state = state.copyWith(
      statusMessage: 'Submitting pairing code...',
      clearErrorMessage: true,
    );
    _gateway.pair(code.trim());
  }

  void createSession() {
    state = state.copyWith(
      statusMessage: 'Creating session...',
      clearErrorMessage: true,
    );
    _gateway.createSession(
      role: state.role.wireValue,
      enableGraph: state.role == UserRole.supervised,
      instructions:
          'You are LiveClaw mobile mode. Keep audio replies short, action-oriented, and confirm high-risk tools before execution.',
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
      final capture = await _audioCapture.startCapture();
      final captureSampleRate = capture.sampleRate;
      final captureChannels = capture.numChannels;

      state = state.copyWith(
        statusMessage: capture.statusHint ??
            (captureSampleRate == 24000 && captureChannels == 1
                ? 'Listening at 24kHz... release to send audio.'
                : 'Listening at ${captureSampleRate}Hz/${captureChannels}ch and adapting to 24kHz mono...'),
      );

      if (capture.mode == AudioCaptureMode.stream) {
        final stream = capture.stream;
        if (stream == null) {
          throw StateError('Microphone stream started without stream payload.');
        }
        _micStreamSub = stream.listen(
          (chunk) {
            final normalizedChunk = _normalizeCapturedChunk(
              chunk,
              captureSampleRate,
              captureChannels,
            );
            if (normalizedChunk.isEmpty) {
              return;
            }
            _gateway.sendAudioChunk(
              sessionId: sessionId,
              audioBase64: base64Encode(normalizedChunk),
            );
            state = state.copyWith(
              micLevel: _estimateMicLevel(normalizedChunk),
              recording: true,
              stage: ConnectionStage.streaming,
            );
          },
          onError: (Object error) {
            _pushError('Mic stream failed: $error');
          },
          cancelOnError: false,
        );
      }

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

    final captured = await _audioCapture.stopCapture();
    final bufferedBytes = captured?.pcm16Bytes;
    if (captured?.mode == AudioCaptureMode.bufferedFile &&
        (bufferedBytes == null || bufferedBytes.isEmpty)) {
      _pushError('Could not capture microphone audio. Try again.');
      return;
    }
    if (captured != null && bufferedBytes != null) {
      final normalized = _normalizeCapturedChunk(
        bufferedBytes,
        captured.sampleRate,
        captured.numChannels,
      );
      if (normalized.isEmpty) {
        _pushError('Captured audio was empty. Try holding to speak again.');
        return;
      }
      _sendBufferedAudio(
        sessionId: sessionId,
        pcm16Bytes: normalized,
      );
    }
    _gateway.commitAudio(sessionId);
    _gateway.createResponse(sessionId);

    state = state.copyWith(
      recording: false,
      micLevel: 0,
      stage: ConnectionStage.sessionReady,
      statusMessage: captured?.mode == AudioCaptureMode.bufferedFile
          ? 'Buffered audio sent. Waiting for LiveClaw response...'
          : 'Audio sent. Waiting for LiveClaw response...',
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
          statusMessage: 'Pairing accepted. Authenticating...',
        );
        _gateway.authenticate(token);
      case AuthenticatedEvent(:final principalId):
        state = state.copyWith(
          stage: ConnectionStage.authenticated,
          principalId: principalId,
          clearErrorMessage: true,
          statusMessage:
              'Authenticated as $principalId. You can create a session now.',
        );
      case PairFailureEvent(:final reason):
        _pushError('Pair failed: $reason');
      case SessionCreatedEvent(:final sessionId):
        state = state.copyWith(
          stage: ConnectionStage.sessionReady,
          sessionId: sessionId,
          clearErrorMessage: true,
          statusMessage: 'Session created. Hold the orb to speak.',
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
          statusMessage: 'Session terminated.',
        );
      case TranscriptUpdateEvent(:final text, :final isFinal):
        if (text.trim().isEmpty) {
          return;
        }
        _upsertTranscriptUpdate(text: text, isFinal: isFinal);
      case AudioOutputEvent(:final audioBase64):
        unawaited(_playAudioOutput(audioBase64));
      case ProtocolAckEvent(:final type, :final sessionId):
        _handleProtocolAck(type: type, sessionId: sessionId);
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
          statusMessage: 'Gateway online. Active sessions: $activeSessions',
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

  void _upsertTranscriptUpdate({
    required String text,
    required bool isFinal,
  }) {
    final updated = List<TranscriptEntry>.from(state.transcript);
    final now = DateTime.now();
    final liveIndex = updated.lastIndexWhere(
      (entry) => entry.speaker == 'live' && !entry.isFinal,
    );

    if (isFinal) {
      if (liveIndex >= 0) {
        updated[liveIndex] = TranscriptEntry(
          speaker: 'agent',
          text: _mergeTranscriptDelta(updated[liveIndex].text, text),
          timestamp: now,
          isFinal: true,
        );
      } else {
        updated.add(
          TranscriptEntry(
            speaker: 'agent',
            text: text,
            timestamp: now,
            isFinal: true,
          ),
        );
      }
    } else {
      if (liveIndex >= 0) {
        final previous = updated[liveIndex];
        updated[liveIndex] = TranscriptEntry(
          speaker: 'live',
          text: _mergeTranscriptDelta(previous.text, text),
          timestamp: now,
          isFinal: false,
        );
      } else {
        updated.add(
          TranscriptEntry(
            speaker: 'live',
            text: text,
            timestamp: now,
            isFinal: false,
          ),
        );
      }
    }

    state = state.copyWith(transcript: updated);
  }

  String _mergeTranscriptDelta(String previous, String next) {
    final previousTrimmed = previous.trim();
    final nextTrimmed = next.trim();
    if (previousTrimmed.isEmpty) {
      return nextTrimmed;
    }
    if (nextTrimmed.isEmpty) {
      return previousTrimmed;
    }
    if (nextTrimmed.startsWith(previousTrimmed)) {
      return nextTrimmed;
    }
    if (previousTrimmed.endsWith(nextTrimmed)) {
      return previousTrimmed;
    }
    return '$previousTrimmed $nextTrimmed';
  }

  Future<void> _playAudioOutput(String audioBase64) async {
    if (audioBase64.trim().isEmpty) {
      return;
    }
    try {
      await _audioPlayback.playPcm16FromBase64(audioBase64);
    } catch (error) {
      state = state.copyWith(
        toolActivity: <ToolActivityEntry>[
          ToolActivityEntry(
            toolName: 'audio',
            summary: 'Playback failed: $error',
            timestamp: DateTime.now(),
          ),
          ...state.toolActivity,
        ],
      );
    }
  }

  void _handleProtocolAck({
    required String type,
    required String? sessionId,
  }) {
    final activeSessionId = state.sessionId;
    if (sessionId != null &&
        sessionId.isNotEmpty &&
        activeSessionId != null &&
        activeSessionId != sessionId) {
      return;
    }

    switch (type) {
      case 'AudioCommitted':
        state = state.copyWith(
          statusMessage: 'Audio committed. Waiting for response...',
        );
      case 'ResponseCreateAccepted':
        state = state.copyWith(
          statusMessage: 'Response generation started...',
        );
      case 'ResponseInterruptAccepted':
        state = state.copyWith(
          statusMessage: 'Response interrupted.',
        );
      case 'PromptAccepted':
        state = state.copyWith(
          statusMessage: 'Prompt accepted by gateway.',
        );
      default:
        break;
    }
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

  Uint8List _normalizeCapturedChunk(
    Uint8List bytes,
    int inputSampleRate,
    int inputChannels,
  ) {
    final mono = _downmixToMonoPcm16(bytes, inputChannels);
    if (mono.isEmpty) {
      return Uint8List(0);
    }

    final resampled = inputSampleRate == 24000
        ? mono
        : _resampleMonoPcm16(mono, inputSampleRate, 24000);

    return _monoPcm16ToBytes(resampled);
  }

  Int16List _downmixToMonoPcm16(Uint8List bytes, int inputChannels) {
    if (inputChannels <= 0) {
      return Int16List(0);
    }

    final totalSamples = bytes.length ~/ 2;
    final frameCount = totalSamples ~/ inputChannels;
    if (frameCount <= 0) {
      return Int16List(0);
    }

    final output = Int16List(frameCount);
    final inputView = ByteData.sublistView(bytes);

    for (var frame = 0; frame < frameCount; frame++) {
      var sum = 0;
      final baseSampleIndex = frame * inputChannels;
      for (var ch = 0; ch < inputChannels; ch++) {
        final sample = inputView.getInt16(
          (baseSampleIndex + ch) * 2,
          Endian.little,
        );
        sum += sample;
      }
      final mixed = (sum / inputChannels).round().clamp(-32768, 32767);
      output[frame] = mixed;
    }

    return output;
  }

  Int16List _resampleMonoPcm16(
    Int16List input,
    int inputSampleRate,
    int targetSampleRate,
  ) {
    if (input.isEmpty || inputSampleRate <= 0 || targetSampleRate <= 0) {
      return Int16List(0);
    }
    if (inputSampleRate == targetSampleRate) {
      return input;
    }

    final outputLength =
        ((input.length * targetSampleRate) / inputSampleRate).floor();
    if (outputLength <= 0) {
      return Int16List(0);
    }

    final output = Int16List(outputLength);
    for (var i = 0; i < outputLength; i++) {
      final sourceIndex =
          ((i * inputSampleRate) / targetSampleRate).floor().clamp(
                0,
                input.length - 1,
              );
      output[i] = input[sourceIndex];
    }

    return output;
  }

  Uint8List _monoPcm16ToBytes(Int16List samples) {
    final bytes = Uint8List(samples.length * 2);
    final view = ByteData.sublistView(bytes);
    for (var i = 0; i < samples.length; i++) {
      view.setInt16(i * 2, samples[i], Endian.little);
    }
    return bytes;
  }

  void _sendBufferedAudio({
    required String sessionId,
    required Uint8List pcm16Bytes,
  }) {
    const chunkSize = 3840; // 80ms at 24kHz mono PCM16.
    for (var offset = 0; offset < pcm16Bytes.length; offset += chunkSize) {
      final end = offset + chunkSize < pcm16Bytes.length
          ? offset + chunkSize
          : pcm16Bytes.length;
      final chunk = Uint8List.sublistView(pcm16Bytes, offset, end);
      _gateway.sendAudioChunk(
        sessionId: sessionId,
        audioBase64: base64Encode(chunk),
      );
    }
  }

  void _pushError(String message) {
    state = state.copyWith(
      stage: ConnectionStage.error,
      errorMessage: message,
      recording: false,
      micLevel: 0,
      statusMessage: 'Failed: $message',
    );
  }
}
