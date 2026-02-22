import 'package:flutter/material.dart';

enum ConnectionStage {
  disconnected,
  connecting,
  connected,
  authenticated,
  sessionReady,
  streaming,
  error,
}

enum UserRole {
  readOnly,
  supervised,
  full,
}

extension UserRoleX on UserRole {
  String get wireValue => switch (this) {
        UserRole.readOnly => 'readonly',
        UserRole.supervised => 'supervised',
        UserRole.full => 'full',
      };

  String get label => switch (this) {
        UserRole.readOnly => 'Read Only',
        UserRole.supervised => 'Supervised',
        UserRole.full => 'Full',
      };
}

@immutable
class TranscriptEntry {
  const TranscriptEntry({
    required this.speaker,
    required this.text,
    required this.timestamp,
    required this.isFinal,
  });

  final String speaker;
  final String text;
  final DateTime timestamp;
  final bool isFinal;
}

@immutable
class ToolActivityEntry {
  const ToolActivityEntry({
    required this.toolName,
    required this.summary,
    required this.timestamp,
  });

  final String toolName;
  final String summary;
  final DateTime timestamp;
}

@immutable
class VoiceSessionState {
  const VoiceSessionState({
    required this.stage,
    required this.gatewayUrl,
    required this.role,
    required this.deviceAuthEnabled,
    required this.statusMessage,
    required this.paired,
    required this.biometricUnlocked,
    required this.recording,
    required this.micLevel,
    required this.protocolVersion,
    required this.runtimeKind,
    required this.providerKind,
    required this.publicBindAllowed,
    required this.transcript,
    required this.toolActivity,
    this.principalId,
    this.sessionId,
    this.errorMessage,
  });

  factory VoiceSessionState.initial() => const VoiceSessionState(
        stage: ConnectionStage.disconnected,
        gatewayUrl: 'ws://127.0.0.1:8420/ws',
        role: UserRole.supervised,
        deviceAuthEnabled: false,
        statusMessage: 'Ready. Connect to your LiveClaw gateway.',
        paired: false,
        biometricUnlocked: false,
        recording: false,
        micLevel: 0,
        protocolVersion: '',
        runtimeKind: '',
        providerKind: '',
        publicBindAllowed: false,
        transcript: <TranscriptEntry>[],
        toolActivity: <ToolActivityEntry>[],
      );

  final ConnectionStage stage;
  final String gatewayUrl;
  final UserRole role;
  final bool deviceAuthEnabled;
  final String statusMessage;
  final bool paired;
  final bool biometricUnlocked;
  final bool recording;
  final double micLevel;
  final String protocolVersion;
  final String runtimeKind;
  final String providerKind;
  final bool publicBindAllowed;
  final List<TranscriptEntry> transcript;
  final List<ToolActivityEntry> toolActivity;
  final String? principalId;
  final String? sessionId;
  final String? errorMessage;

  bool get canPair => stage == ConnectionStage.connected && !paired;

  VoiceSessionState copyWith({
    ConnectionStage? stage,
    String? gatewayUrl,
    UserRole? role,
    bool? deviceAuthEnabled,
    String? statusMessage,
    bool? paired,
    bool? biometricUnlocked,
    bool? recording,
    double? micLevel,
    String? protocolVersion,
    String? runtimeKind,
    String? providerKind,
    bool? publicBindAllowed,
    List<TranscriptEntry>? transcript,
    List<ToolActivityEntry>? toolActivity,
    String? principalId,
    String? sessionId,
    String? errorMessage,
    bool clearPrincipalId = false,
    bool clearSessionId = false,
    bool clearErrorMessage = false,
  }) {
    return VoiceSessionState(
      stage: stage ?? this.stage,
      gatewayUrl: gatewayUrl ?? this.gatewayUrl,
      role: role ?? this.role,
      deviceAuthEnabled: deviceAuthEnabled ?? this.deviceAuthEnabled,
      statusMessage: statusMessage ?? this.statusMessage,
      paired: paired ?? this.paired,
      biometricUnlocked: biometricUnlocked ?? this.biometricUnlocked,
      recording: recording ?? this.recording,
      micLevel: micLevel ?? this.micLevel,
      protocolVersion: protocolVersion ?? this.protocolVersion,
      runtimeKind: runtimeKind ?? this.runtimeKind,
      providerKind: providerKind ?? this.providerKind,
      publicBindAllowed: publicBindAllowed ?? this.publicBindAllowed,
      transcript: transcript ?? this.transcript,
      toolActivity: toolActivity ?? this.toolActivity,
      principalId: clearPrincipalId ? null : (principalId ?? this.principalId),
      sessionId: clearSessionId ? null : (sessionId ?? this.sessionId),
      errorMessage:
          clearErrorMessage ? null : (errorMessage ?? this.errorMessage),
    );
  }
}
