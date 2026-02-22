import 'dart:math' as math;

import 'package:flutter/material.dart';

class ResonanceOrb extends StatelessWidget {
  const ResonanceOrb({
    super.key,
    required this.enabled,
    required this.recording,
    required this.micLevel,
    required this.onPressStart,
    required this.onPressEnd,
  });

  final bool enabled;
  final bool recording;
  final double micLevel;
  final VoidCallback onPressStart;
  final VoidCallback onPressEnd;

  @override
  Widget build(BuildContext context) {
    final colorScheme = Theme.of(context).colorScheme;
    final size = recording ? 236.0 : 214.0;

    return GestureDetector(
      onLongPressStart: enabled ? (_) => onPressStart() : null,
      onLongPressEnd: enabled ? (_) => onPressEnd() : null,
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 220),
        width: size,
        height: size,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          gradient: RadialGradient(
            colors: <Color>[
              colorScheme.primary.withValues(alpha: 0.9),
              colorScheme.primary.withValues(alpha: 0.22),
              const Color(0xFF061A1F),
            ],
            stops: const <double>[0.0, 0.42, 1.0],
          ),
          boxShadow: <BoxShadow>[
            BoxShadow(
              color: colorScheme.primary
                  .withValues(alpha: 0.42 + (micLevel * 0.18)),
              blurRadius: 24 + (micLevel * 36),
              spreadRadius: 3 + (micLevel * 7),
            ),
            const BoxShadow(
              color: Color(0x44000000),
              blurRadius: 16,
              offset: Offset(0, 10),
            ),
          ],
        ),
        child: CustomPaint(
          painter: _ResonanceRingsPainter(
            pulse: micLevel,
            active: recording,
            color: colorScheme.secondary,
          ),
          child: Center(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: <Widget>[
                Icon(
                  recording
                      ? Icons.radio_button_checked_rounded
                      : Icons.mic_rounded,
                  color: Colors.white,
                  size: 44,
                ),
                const SizedBox(height: 6),
                Text(
                  recording ? 'Listening...' : 'Hold to speak',
                  style: Theme.of(context).textTheme.labelLarge?.copyWith(
                        color: Colors.white,
                      ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

class _ResonanceRingsPainter extends CustomPainter {
  const _ResonanceRingsPainter({
    required this.pulse,
    required this.active,
    required this.color,
  });

  final double pulse;
  final bool active;
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final center = Offset(size.width / 2, size.height / 2);
    final baseRadius = math.min(size.width, size.height) / 2;

    for (var i = 1; i <= 3; i++) {
      final ringProgress = i / 3;
      final dynamicRadius = baseRadius * (0.36 + (ringProgress * 0.46)) +
          (active ? pulse * (8 + (i * 3)) : 0);
      final paint = Paint()
        ..color = color.withValues(
            alpha: active ? (0.24 - (ringProgress * 0.06)) : 0.08)
        ..style = PaintingStyle.stroke
        ..strokeWidth = active ? 2.2 : 1.1;

      canvas.drawCircle(center, dynamicRadius, paint);
    }
  }

  @override
  bool shouldRepaint(covariant _ResonanceRingsPainter oldDelegate) {
    return oldDelegate.pulse != pulse ||
        oldDelegate.active != active ||
        oldDelegate.color != color;
  }
}
