import 'dart:io';
import 'dart:typed_data';

import 'package:record/record.dart';

enum AudioCaptureMode { stream, bufferedFile }

class AudioCaptureSession {
  const AudioCaptureSession({
    required this.mode,
    required this.stream,
    required this.sampleRate,
    required this.numChannels,
    this.statusHint,
  });

  final AudioCaptureMode mode;
  final Stream<Uint8List>? stream;
  final int sampleRate;
  final int numChannels;
  final String? statusHint;
}

class AudioCaptureStopResult {
  const AudioCaptureStopResult({
    required this.mode,
    required this.sampleRate,
    required this.numChannels,
    this.pcm16Bytes,
  });

  final AudioCaptureMode mode;
  final int sampleRate;
  final int numChannels;
  final Uint8List? pcm16Bytes;
}

class AudioCaptureService {
  AudioCaptureService({AudioRecorder? recorder})
      : _recorder = recorder ?? AudioRecorder();

  final AudioRecorder _recorder;
  AudioCaptureMode? _activeMode;
  int _activeSampleRate = 24000;
  int _activeNumChannels = 1;
  String? _bufferFilePath;

  Future<AudioCaptureSession> startCapture() async {
    final hasPermission = await _recorder.hasPermission();
    if (!hasPermission) {
      throw StateError('Microphone permission not granted');
    }

    if (Platform.isIOS) {
      await _configureIosStreamingSession();
    }

    const candidateConfigs = <({int sampleRate, int numChannels})>[
      (sampleRate: 24000, numChannels: 1),
      (sampleRate: 48000, numChannels: 1),
      (sampleRate: 44100, numChannels: 1),
      (sampleRate: 16000, numChannels: 1),
      (sampleRate: 48000, numChannels: 2),
      (sampleRate: 44100, numChannels: 2),
    ];
    final errors = <String>[];

    for (final candidate in candidateConfigs) {
      try {
        final stream = await _recorder.startStream(
          RecordConfig(
            encoder: AudioEncoder.pcm16bits,
            sampleRate: candidate.sampleRate,
            numChannels: candidate.numChannels,
            bitRate: candidate.sampleRate * 16 * candidate.numChannels,
            iosConfig: const IosRecordConfig(
              categoryOptions: [
                IosAudioCategoryOption.allowBluetooth,
              ],
            ),
          ),
        );
        _activeMode = AudioCaptureMode.stream;
        _activeSampleRate = candidate.sampleRate;
        _activeNumChannels = candidate.numChannels;
        return AudioCaptureSession(
          mode: AudioCaptureMode.stream,
          stream: stream,
          sampleRate: candidate.sampleRate,
          numChannels: candidate.numChannels,
          statusHint: candidate.sampleRate == 24000 &&
                  candidate.numChannels == 1
              ? null
              : 'Microphone started at ${candidate.sampleRate}Hz/${candidate.numChannels}ch and will be adapted to 24kHz mono.',
        );
      } catch (error) {
        errors.add(
          '${candidate.sampleRate}Hz/${candidate.numChannels}ch: $error',
        );
      }
    }

    if (Platform.isIOS) {
      final fallback = await _startIosBufferedCapture(errors: errors);
      if (fallback != null) {
        return fallback;
      }
      await _restoreIosAudioSessionDefaults();
    }

    throw StateError(
      'Failed to start microphone capture in supported formats. ${errors.join(' | ')}. '
      'If using iOS Simulator, enable microphone input in Simulator settings '
      '(I/O or Features > Audio Input), or test on a physical iPhone.',
    );
  }

  Future<AudioCaptureStopResult?> stopCapture() async {
    if (await _recorder.isRecording()) {
      final stopPath = await _recorder.stop();
      if (_activeMode == AudioCaptureMode.bufferedFile) {
        final fallbackPath = _bufferFilePath;
        final path = stopPath ?? fallbackPath;
        if (path == null || path.isEmpty) {
          return const AudioCaptureStopResult(
            mode: AudioCaptureMode.bufferedFile,
            sampleRate: 24000,
            numChannels: 1,
          );
        }
        final bytes = await File(path).readAsBytes();
        final wav = _extractWavData(bytes);
        await _deleteIfExists(path);
        await _restoreIosAudioSessionDefaults();

        return AudioCaptureStopResult(
          mode: AudioCaptureMode.bufferedFile,
          sampleRate: wav.sampleRate,
          numChannels: wav.numChannels,
          pcm16Bytes: wav.pcm16Bytes,
        );
      }
    }
    if (Platform.isIOS) {
      await _restoreIosAudioSessionDefaults();
    }
    _activeMode = null;
    _bufferFilePath = null;
    return null;
  }

  Future<void> dispose() async {
    final path = _bufferFilePath;
    if (path != null && path.isNotEmpty) {
      await _deleteIfExists(path);
    }
    await _recorder.dispose();
  }

  Future<void> _configureIosStreamingSession() async {
    final ios = _recorder.ios;
    if (ios == null) {
      return;
    }

    await ios.manageAudioSession(false);
    await ios.setAudioSessionCategory(
      category: IosAudioCategory.record,
      options: const [
        IosAudioCategoryOptions.allowBluetooth,
      ],
    );
    await ios.setAudioSessionActive(true);
  }

  Future<void> _restoreIosAudioSessionDefaults() async {
    if (!Platform.isIOS) {
      return;
    }
    final ios = _recorder.ios;
    if (ios == null) {
      return;
    }
    try {
      await ios.setAudioSessionActive(false);
    } catch (_) {}
    try {
      await ios.manageAudioSession(true);
    } catch (_) {}
    _activeMode = null;
    _bufferFilePath = null;
  }

  Future<AudioCaptureSession?> _startIosBufferedCapture({
    required List<String> errors,
  }) async {
    const candidateFileConfigs = <({int sampleRate, int numChannels})>[
      (sampleRate: 48000, numChannels: 1),
      (sampleRate: 44100, numChannels: 1),
      (sampleRate: 24000, numChannels: 1),
      (sampleRate: 16000, numChannels: 1),
    ];

    for (final candidate in candidateFileConfigs) {
      try {
        final path =
            '${Directory.systemTemp.path}/liveclaw_capture_${DateTime.now().microsecondsSinceEpoch}.wav';
        await _recorder.start(
          RecordConfig(
            encoder: AudioEncoder.wav,
            sampleRate: candidate.sampleRate,
            numChannels: candidate.numChannels,
            iosConfig: const IosRecordConfig(
              categoryOptions: [
                IosAudioCategoryOption.allowBluetooth,
              ],
            ),
          ),
          path: path,
        );
        _activeMode = AudioCaptureMode.bufferedFile;
        _activeSampleRate = candidate.sampleRate;
        _activeNumChannels = candidate.numChannels;
        _bufferFilePath = path;
        return const AudioCaptureSession(
          mode: AudioCaptureMode.bufferedFile,
          stream: null,
          sampleRate: 24000,
          numChannels: 1,
          statusHint:
              'Microphone in compatibility mode (buffered capture). Hold to speak, release to send.',
        );
      } catch (error) {
        errors.add(
          'wav ${candidate.sampleRate}Hz/${candidate.numChannels}ch: $error',
        );
      }
    }
    return null;
  }

  Future<void> _deleteIfExists(String path) async {
    final file = File(path);
    if (await file.exists()) {
      await file.delete();
    }
  }

  _WavCapture _extractWavData(Uint8List bytes) {
    if (bytes.length < 44) {
      throw StateError('Buffered capture produced an invalid WAV payload.');
    }
    final view = ByteData.sublistView(bytes);
    if (_readAscii(bytes, 0, 4) != 'RIFF' ||
        _readAscii(bytes, 8, 4) != 'WAVE') {
      throw StateError('Buffered capture did not produce WAV data.');
    }

    var offset = 12;
    int sampleRate = _activeSampleRate;
    int numChannels = _activeNumChannels;
    int? dataOffset;
    int? dataLength;

    while (offset + 8 <= bytes.length) {
      final chunkId = _readAscii(bytes, offset, 4);
      final chunkSize = view.getUint32(offset + 4, Endian.little);
      final payloadStart = offset + 8;
      if (payloadStart + chunkSize > bytes.length) {
        break;
      }

      if (chunkId == 'fmt ' && chunkSize >= 16) {
        numChannels = view.getUint16(payloadStart + 2, Endian.little);
        sampleRate = view.getUint32(payloadStart + 4, Endian.little);
      } else if (chunkId == 'data') {
        dataOffset = payloadStart;
        dataLength = chunkSize;
        break;
      }

      offset = payloadStart + chunkSize + (chunkSize.isOdd ? 1 : 0);
    }

    if (dataOffset == null || dataLength == null) {
      throw StateError('Buffered capture WAV missing data chunk.');
    }

    return _WavCapture(
      sampleRate: sampleRate,
      numChannels: numChannels,
      pcm16Bytes:
          Uint8List.sublistView(bytes, dataOffset, dataOffset + dataLength),
    );
  }

  String _readAscii(Uint8List bytes, int offset, int length) {
    return String.fromCharCodes(bytes.sublist(offset, offset + length));
  }
}

class _WavCapture {
  const _WavCapture({
    required this.sampleRate,
    required this.numChannels,
    required this.pcm16Bytes,
  });

  final int sampleRate;
  final int numChannels;
  final Uint8List pcm16Bytes;
}
