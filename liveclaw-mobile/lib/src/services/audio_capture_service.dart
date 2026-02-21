import 'dart:typed_data';

import 'package:record/record.dart';

class AudioCaptureService {
  AudioCaptureService({AudioRecorder? recorder}) : _recorder = recorder ?? AudioRecorder();

  final AudioRecorder _recorder;

  Future<Stream<Uint8List>> startCapture() async {
    final hasPermission = await _recorder.hasPermission();
    if (!hasPermission) {
      throw StateError('Microphone permission not granted');
    }

    return _recorder.startStream(
      const RecordConfig(
        encoder: AudioEncoder.pcm16bits,
        sampleRate: 24000,
        numChannels: 1,
        bitRate: 384000,
      ),
    );
  }

  Future<void> stopCapture() async {
    if (await _recorder.isRecording()) {
      await _recorder.stop();
    }
  }

  Future<void> dispose() => _recorder.dispose();
}
