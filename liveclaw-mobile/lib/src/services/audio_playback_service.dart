import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:audioplayers/audioplayers.dart';

import 'pcm_wav.dart';

class AudioPlaybackService {
  AudioPlaybackService({AudioPlayer? player})
      : _player = player ?? AudioPlayer() {
    unawaited(_player.setReleaseMode(ReleaseMode.stop));
  }

  final AudioPlayer _player;
  final Queue<_QueuedAudioChunk> _queue = ListQueue<_QueuedAudioChunk>();
  bool _draining = false;
  bool _disposed = false;
  int _chunkCounter = 0;

  Future<void> playPcm16FromBase64(String base64PcmAudio) async {
    final normalized = _normalizeBase64Payload(base64PcmAudio);
    if (normalized.isEmpty || _disposed) {
      return;
    }

    final decoded = base64Decode(normalized);
    if (decoded.isEmpty) {
      return;
    }

    final audioBytes = Uint8List.fromList(decoded);
    final wavBytes = _isWav(audioBytes)
        ? audioBytes
        : PcmWav.wrapPcm16Mono(
            pcmBytes: audioBytes,
            sampleRate: 24000,
          );
    final chunkPath = await _writeChunkFile(wavBytes);

    _queue.add(
      _QueuedAudioChunk(
        path: chunkPath,
        approxDurationMs: _estimateChunkDurationMs(audioBytes),
      ),
    );
    await _drainQueue();
  }

  Future<void> _drainQueue() async {
    if (_draining || _disposed) {
      return;
    }
    _draining = true;

    try {
      while (_queue.isNotEmpty && !_disposed) {
        final next = _queue.removeFirst();
        try {
          await _player.play(
            DeviceFileSource(next.path, mimeType: 'audio/wav'),
            ctx: _playbackContext,
          );
          try {
            await _player.onPlayerComplete.first.timeout(
              Duration(milliseconds: next.approxDurationMs + 300),
            );
          } on TimeoutException {
            await Future<void>.delayed(
              Duration(milliseconds: next.approxDurationMs),
            );
          }
        } finally {
          await _deleteIfExists(next.path);
        }
      }
    } finally {
      _draining = false;
    }
  }

  String _normalizeBase64Payload(String payload) {
    final trimmed = payload.trim();
    if (trimmed.isEmpty) {
      return '';
    }
    if (!trimmed.startsWith('data:')) {
      return trimmed;
    }
    final commaIndex = trimmed.indexOf(',');
    if (commaIndex < 0 || commaIndex + 1 >= trimmed.length) {
      return '';
    }
    return trimmed.substring(commaIndex + 1);
  }

  bool _isWav(Uint8List bytes) {
    if (bytes.length < 12) {
      return false;
    }
    final riff = String.fromCharCodes(bytes.sublist(0, 4));
    final wave = String.fromCharCodes(bytes.sublist(8, 12));
    return riff == 'RIFF' && wave == 'WAVE';
  }

  Future<String> _writeChunkFile(Uint8List wavBytes) async {
    final fileName =
        'liveclaw_out_${DateTime.now().microsecondsSinceEpoch}_${_chunkCounter++}.wav';
    final path = '${Directory.systemTemp.path}/$fileName';
    await File(path).writeAsBytes(wavBytes, flush: true);
    return path;
  }

  Future<void> _deleteIfExists(String path) async {
    final file = File(path);
    if (await file.exists()) {
      await file.delete();
    }
  }

  int _estimateChunkDurationMs(Uint8List pcmBytes) {
    final samples = pcmBytes.length ~/ 2;
    if (samples <= 0) {
      return 80;
    }
    final millis = ((samples / 24000.0) * 1000.0).round();
    return millis.clamp(40, 1600);
  }

  Future<void> dispose() async {
    _disposed = true;
    while (_queue.isNotEmpty) {
      final pending = _queue.removeFirst();
      await _deleteIfExists(pending.path);
    }
    await _player.dispose();
  }
}

class _QueuedAudioChunk {
  const _QueuedAudioChunk({
    required this.path,
    required this.approxDurationMs,
  });

  final String path;
  final int approxDurationMs;
}

final AudioContext _playbackContext = AudioContext(
  iOS: AudioContextIOS(
    category: AVAudioSessionCategory.playback,
  ),
);
