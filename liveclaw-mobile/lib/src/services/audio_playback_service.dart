import 'dart:convert';
import 'dart:typed_data';

import 'package:audioplayers/audioplayers.dart';

import 'pcm_wav.dart';

class AudioPlaybackService {
  AudioPlaybackService({AudioPlayer? player}) : _player = player ?? AudioPlayer();

  final AudioPlayer _player;

  Future<void> playPcm16FromBase64(String base64PcmAudio) async {
    if (base64PcmAudio.isEmpty) {
      return;
    }

    final pcm = base64Decode(base64PcmAudio);
    final wav = PcmWav.wrapPcm16Mono(pcmBytes: Uint8List.fromList(pcm), sampleRate: 24000);

    await _player.stop();
    await _player.play(BytesSource(wav));
  }

  Future<void> dispose() => _player.dispose();
}
