import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import 'package:liveclaw_mobile/src/ui/liveclaw_app.dart';

void main() {
  testWidgets('renders liveclaw shell', (WidgetTester tester) async {
    await tester.pumpWidget(const ProviderScope(child: LiveclawApp()));
    expect(find.text('Liveclaw Resonance'), findsOneWidget);
  });
}
