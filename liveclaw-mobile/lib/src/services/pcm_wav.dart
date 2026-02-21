import 'dart:typed_data';

class PcmWav {
  const PcmWav._();

  static Uint8List wrapPcm16Mono({
    required Uint8List pcmBytes,
    required int sampleRate,
    int channels = 1,
  }) {
    final byteRate = sampleRate * channels * 16 ~/ 8;
    final blockAlign = channels * 16 ~/ 8;
    final totalLength = 44 + pcmBytes.length;

    final out = Uint8List(totalLength);
    final view = ByteData.view(out.buffer);

    _writeAscii(out, 0, 'RIFF');
    view.setUint32(4, totalLength - 8, Endian.little);
    _writeAscii(out, 8, 'WAVE');
    _writeAscii(out, 12, 'fmt ');
    view.setUint32(16, 16, Endian.little);
    view.setUint16(20, 1, Endian.little);
    view.setUint16(22, channels, Endian.little);
    view.setUint32(24, sampleRate, Endian.little);
    view.setUint32(28, byteRate, Endian.little);
    view.setUint16(32, blockAlign, Endian.little);
    view.setUint16(34, 16, Endian.little);
    _writeAscii(out, 36, 'data');
    view.setUint32(40, pcmBytes.length, Endian.little);
    out.setRange(44, totalLength, pcmBytes);

    return out;
  }

  static void _writeAscii(Uint8List bytes, int offset, String value) {
    for (var i = 0; i < value.length; i++) {
      bytes[offset + i] = value.codeUnitAt(i);
    }
  }
}
