import 'dart:async';
import 'dart:convert';

import 'package:web_socket_channel/io.dart';

import '../models/gateway_models.dart';

class LiveclawGatewayClient {
  IOWebSocketChannel? _channel;
  StreamSubscription<dynamic>? _channelSub;
  final StreamController<GatewayEvent> _eventsController =
      StreamController<GatewayEvent>.broadcast();

  Stream<GatewayEvent> get events => _eventsController.stream;

  bool get isConnected => _channel != null;

  Future<void> connect(String gatewayUrl) async {
    await disconnect();

    final uri = Uri.parse(gatewayUrl);
    final channel = IOWebSocketChannel.connect(
      uri,
      pingInterval: const Duration(seconds: 20),
    );

    _channel = channel;
    _channelSub = channel.stream.listen(
      (dynamic raw) => _onData(raw),
      onError: (Object error, StackTrace stackTrace) {
        _eventsController.add(
          ErrorEvent(
            code: 'socket_error',
            message: error.toString(),
          ),
        );
      },
      onDone: () {
        _eventsController.add(
          const ErrorEvent(
            code: 'socket_closed',
            message: 'Gateway connection closed',
          ),
        );
      },
      cancelOnError: false,
    );
  }

  Future<void> disconnect() async {
    await _channelSub?.cancel();
    _channelSub = null;
    await _channel?.sink.close();
    _channel = null;
  }

  Future<void> dispose() async {
    await disconnect();
    await _eventsController.close();
  }

  void pair(String code) {
    _send(<String, dynamic>{
      'type': 'Pair',
      'code': code.trim(),
    });
  }

  void authenticate(String token) {
    _send(<String, dynamic>{
      'type': 'Authenticate',
      'token': token,
    });
  }

  void createSession({
    required String role,
    bool enableGraph = true,
    String? model,
    String? voice,
    String? instructions,
  }) {
    _send(<String, dynamic>{
      'type': 'CreateSession',
      'config': <String, dynamic>{
        'role': role,
        'enable_graph': enableGraph,
        if (model != null && model.isNotEmpty) 'model': model,
        if (voice != null && voice.isNotEmpty) 'voice': voice,
        if (instructions != null && instructions.isNotEmpty)
          'instructions': instructions,
      },
    });
  }

  void terminateSession(String sessionId) {
    _send(<String, dynamic>{
      'type': 'TerminateSession',
      'session_id': sessionId,
    });
  }

  void sendAudioChunk(
      {required String sessionId, required String audioBase64}) {
    _send(<String, dynamic>{
      'type': 'SessionAudio',
      'session_id': sessionId,
      'audio': audioBase64,
    });
  }

  void commitAudio(String sessionId) {
    _send(<String, dynamic>{
      'type': 'SessionAudioCommit',
      'session_id': sessionId,
    });
  }

  void createResponse(String sessionId) {
    _send(<String, dynamic>{
      'type': 'SessionResponseCreate',
      'session_id': sessionId,
    });
  }

  void interruptResponse(String sessionId) {
    _send(<String, dynamic>{
      'type': 'SessionResponseInterrupt',
      'session_id': sessionId,
    });
  }

  void prompt({required String sessionId, required String prompt}) {
    _send(<String, dynamic>{
      'type': 'SessionPrompt',
      'session_id': sessionId,
      'prompt': prompt,
      'create_response': true,
    });
  }

  void requestDiagnostics() {
    _send(const <String, dynamic>{'type': 'GetDiagnostics'});
  }

  void requestGatewayHealth() {
    _send(const <String, dynamic>{'type': 'GetGatewayHealth'});
  }

  void ping() {
    _send(const <String, dynamic>{'type': 'Ping'});
  }

  void _send(Map<String, dynamic> payload) {
    final channel = _channel;
    if (channel == null) {
      _eventsController.add(
        const ErrorEvent(
          code: 'not_connected',
          message: 'Connect before sending gateway messages',
        ),
      );
      return;
    }

    channel.sink.add(jsonEncode(payload));
  }

  void _onData(dynamic raw) {
    try {
      final decoded = jsonDecode(raw as String) as Map<String, dynamic>;
      _eventsController.add(GatewayEvent.fromJson(decoded));
    } catch (error) {
      _eventsController.add(
        ErrorEvent(
          code: 'decode_error',
          message: 'Failed to decode gateway event: $error',
        ),
      );
    }
  }
}
