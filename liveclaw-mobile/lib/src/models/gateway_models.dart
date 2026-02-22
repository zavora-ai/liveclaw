import 'dart:convert';

sealed class GatewayEvent {
  const GatewayEvent();

  static GatewayEvent fromJson(Map<String, dynamic> json) {
    final type = json['type'] as String?;
    switch (type) {
      case 'PairSuccess':
        return PairSuccessEvent(token: json['token'] as String? ?? '');
      case 'Authenticated':
        return AuthenticatedEvent(
          principalId: json['principal_id'] as String? ?? 'unknown',
        );
      case 'PairFailure':
        return PairFailureEvent(
            reason: json['reason'] as String? ?? 'Pairing failed');
      case 'SessionCreated':
        return SessionCreatedEvent(
            sessionId: json['session_id'] as String? ?? '');
      case 'SessionTerminated':
        return SessionTerminatedEvent(
            sessionId: json['session_id'] as String? ?? '');
      case 'TranscriptUpdate':
        return TranscriptUpdateEvent(
          sessionId: json['session_id'] as String? ?? '',
          text: json['text'] as String? ?? '',
          isFinal: json['is_final'] as bool? ?? false,
        );
      case 'AudioOutput':
        final audioPayload = json['audio'] ??
            json['audio_base64'] ??
            _asMap(json['data'])['audio'] ??
            _asMap(json['data'])['audio_base64'] ??
            json['chunk'];
        return AudioOutputEvent(
          sessionId: json['session_id'] as String? ?? '',
          audioBase64: audioPayload as String? ?? '',
        );
      case 'AudioAccepted':
      case 'AudioCommitted':
      case 'ResponseCreateAccepted':
      case 'ResponseInterruptAccepted':
      case 'PromptAccepted':
        return ProtocolAckEvent(
          type: type!,
          sessionId: json['session_id'] as String?,
        );
      case 'SessionToolResult':
        final rawResult = json['result'];
        final rawGraph = json['graph'];
        return SessionToolResultEvent(
          sessionId: json['session_id'] as String? ?? '',
          toolName: json['tool_name'] as String? ?? 'unknown_tool',
          result: _asMap(rawResult, fallbackKey: 'value'),
          graph: _asMap(rawGraph),
        );
      case 'Diagnostics':
        final data = _asMap(json['data']);
        return DiagnosticsEvent(
          protocolVersion: data['protocol_version'] as String? ?? '',
          runtimeKind: data['runtime_kind'] as String? ?? '',
          providerKind: data['provider_kind'] as String? ?? '',
          securityAllowPublicBind:
              data['security_allow_public_bind'] as bool? ?? false,
          activeSessions: data['active_sessions'] as int? ?? 0,
        );
      case 'GatewayHealth':
        final data = _asMap(json['data']);
        return GatewayHealthEvent(
          uptimeSeconds: data['uptime_seconds'] as int? ?? 0,
          requirePairing: data['require_pairing'] as bool? ?? true,
          activeSessions: data['active_sessions'] as int? ?? 0,
        );
      case 'Error':
        return ErrorEvent(
          code: json['code'] as String? ?? 'gateway_error',
          message: json['message'] as String? ?? 'Gateway error',
        );
      case 'Pong':
        return const PongEvent();
      default:
        return UnknownEvent(type: type ?? 'Unknown', payload: json);
    }
  }

  static Map<String, dynamic> _asMap(dynamic value,
      {String fallbackKey = 'data'}) {
    if (value is Map<String, dynamic>) {
      return value;
    }
    if (value is Map) {
      return Map<String, dynamic>.from(value);
    }
    if (value == null) {
      return <String, dynamic>{};
    }
    return <String, dynamic>{fallbackKey: value};
  }
}

class PairSuccessEvent extends GatewayEvent {
  const PairSuccessEvent({required this.token});
  final String token;
}

class AuthenticatedEvent extends GatewayEvent {
  const AuthenticatedEvent({required this.principalId});
  final String principalId;
}

class PairFailureEvent extends GatewayEvent {
  const PairFailureEvent({required this.reason});
  final String reason;
}

class SessionCreatedEvent extends GatewayEvent {
  const SessionCreatedEvent({required this.sessionId});
  final String sessionId;
}

class SessionTerminatedEvent extends GatewayEvent {
  const SessionTerminatedEvent({required this.sessionId});
  final String sessionId;
}

class TranscriptUpdateEvent extends GatewayEvent {
  const TranscriptUpdateEvent({
    required this.sessionId,
    required this.text,
    required this.isFinal,
  });

  final String sessionId;
  final String text;
  final bool isFinal;
}

class AudioOutputEvent extends GatewayEvent {
  const AudioOutputEvent({required this.sessionId, required this.audioBase64});

  final String sessionId;
  final String audioBase64;
}

class ProtocolAckEvent extends GatewayEvent {
  const ProtocolAckEvent({required this.type, required this.sessionId});

  final String type;
  final String? sessionId;
}

class SessionToolResultEvent extends GatewayEvent {
  const SessionToolResultEvent({
    required this.sessionId,
    required this.toolName,
    required this.result,
    required this.graph,
  });

  final String sessionId;
  final String toolName;
  final Map<String, dynamic> result;
  final Map<String, dynamic> graph;

  String prettyResult() => const JsonEncoder.withIndent('  ').convert(result);
}

class DiagnosticsEvent extends GatewayEvent {
  const DiagnosticsEvent({
    required this.protocolVersion,
    required this.runtimeKind,
    required this.providerKind,
    required this.securityAllowPublicBind,
    required this.activeSessions,
  });

  final String protocolVersion;
  final String runtimeKind;
  final String providerKind;
  final bool securityAllowPublicBind;
  final int activeSessions;
}

class GatewayHealthEvent extends GatewayEvent {
  const GatewayHealthEvent({
    required this.uptimeSeconds,
    required this.requirePairing,
    required this.activeSessions,
  });

  final int uptimeSeconds;
  final bool requirePairing;
  final int activeSessions;
}

class ErrorEvent extends GatewayEvent {
  const ErrorEvent({required this.code, required this.message});

  final String code;
  final String message;
}

class PongEvent extends GatewayEvent {
  const PongEvent();
}

class UnknownEvent extends GatewayEvent {
  const UnknownEvent({required this.type, required this.payload});

  final String type;
  final Map<String, dynamic> payload;
}
