import 'package:flutter/material.dart';
import 'package:intl/intl.dart';

import '../../models/session_models.dart';

class TranscriptTimeline extends StatelessWidget {
  const TranscriptTimeline({
    super.key,
    required this.items,
  });

  final List<TranscriptEntry> items;

  @override
  Widget build(BuildContext context) {
    final visible = items.reversed.take(14).toList();

    if (visible.isEmpty) {
      return Container(
        padding: const EdgeInsets.all(18),
        decoration: _panelDecoration(),
        child: const Text('Transcript lane is empty. Start speaking to populate it.'),
      );
    }

    return Container(
      padding: const EdgeInsets.all(14),
      decoration: _panelDecoration(),
      child: Column(
        children: <Widget>[
          for (final item in visible)
            Align(
              alignment: item.speaker == 'agent' || item.speaker == 'system'
                  ? Alignment.centerLeft
                  : Alignment.centerRight,
              child: Container(
                constraints: const BoxConstraints(maxWidth: 360),
                margin: const EdgeInsets.symmetric(vertical: 5),
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
                decoration: BoxDecoration(
                  color: _bubbleColor(item.speaker),
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(color: Colors.white12),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: <Widget>[
                    Text(
                      item.speaker.toUpperCase(),
                      style: Theme.of(context).textTheme.labelSmall?.copyWith(
                            color: Colors.white70,
                            letterSpacing: 0.9,
                          ),
                    ),
                    const SizedBox(height: 4),
                    Text(item.text),
                    const SizedBox(height: 4),
                    Text(
                      DateFormat.Hm().format(item.timestamp),
                      style: Theme.of(context).textTheme.labelSmall?.copyWith(
                            color: Colors.white60,
                          ),
                    ),
                  ],
                ),
              ),
            ),
        ],
      ),
    );
  }

  Color _bubbleColor(String speaker) {
    if (speaker == 'system') {
      return const Color(0x401EA1D2);
    }
    if (speaker == 'agent') {
      return const Color(0x402A9D8F);
    }
    return const Color(0x40F4A261);
  }

  BoxDecoration _panelDecoration() {
    return BoxDecoration(
      color: const Color(0x40132128),
      borderRadius: BorderRadius.circular(16),
      border: Border.all(color: Colors.white12),
    );
  }
}
