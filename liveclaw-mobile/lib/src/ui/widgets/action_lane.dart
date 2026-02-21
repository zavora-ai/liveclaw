import 'package:flutter/material.dart';
import 'package:intl/intl.dart';

import '../../models/session_models.dart';

class ActionLane extends StatelessWidget {
  const ActionLane({
    super.key,
    required this.entries,
  });

  final List<ToolActivityEntry> entries;

  @override
  Widget build(BuildContext context) {
    if (entries.isEmpty) {
      return Container(
        padding: const EdgeInsets.all(14),
        decoration: _panelDecoration(),
        child: Text(
          'Action lane tracks tool calls and security/health responses.',
          style: Theme.of(context).textTheme.bodyMedium,
        ),
      );
    }

    final visible = entries.take(8).toList();

    return SizedBox(
      height: 122,
      child: ListView.separated(
        scrollDirection: Axis.horizontal,
        itemCount: visible.length,
        separatorBuilder: (_, __) => const SizedBox(width: 10),
        itemBuilder: (context, index) {
          final entry = visible[index];
          return Container(
            width: 236,
            padding: const EdgeInsets.all(12),
            decoration: _panelDecoration(),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: <Widget>[
                Text(
                  entry.toolName,
                  maxLines: 1,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.titleSmall?.copyWith(
                        color: const Color(0xFF9CF2E5),
                      ),
                ),
                const SizedBox(height: 6),
                Text(
                  entry.summary,
                  maxLines: 2,
                  overflow: TextOverflow.ellipsis,
                  style: Theme.of(context).textTheme.bodySmall,
                ),
                const Spacer(),
                Text(
                  DateFormat.Hms().format(entry.timestamp),
                  style: Theme.of(context).textTheme.labelSmall?.copyWith(
                        color: Colors.white60,
                      ),
                ),
              ],
            ),
          );
        },
      ),
    );
  }

  BoxDecoration _panelDecoration() {
    return BoxDecoration(
      color: const Color(0x40132128),
      borderRadius: BorderRadius.circular(16),
      border: Border.all(color: Colors.white12),
    );
  }
}
