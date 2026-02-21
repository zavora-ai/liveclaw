import 'package:flutter/material.dart';
import 'package:google_fonts/google_fonts.dart';

import 'liveclaw_home_screen.dart';

class LiveclawApp extends StatelessWidget {
  const LiveclawApp({super.key});

  @override
  Widget build(BuildContext context) {
    const baseSurface = Color(0xFF0C1418);
    const accent = Color(0xFF32E0C4);
    const alert = Color(0xFFF66B3D);
    const frost = Color(0xFFE8F5EF);

    return MaterialApp(
      title: 'Liveclaw Resonance',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: accent,
          brightness: Brightness.dark,
          primary: accent,
          secondary: alert,
          surface: baseSurface,
        ),
        scaffoldBackgroundColor: baseSurface,
        useMaterial3: true,
        textTheme: TextTheme(
          headlineLarge: GoogleFonts.sora(
            fontWeight: FontWeight.w700,
            letterSpacing: -0.4,
          ),
          titleLarge: GoogleFonts.sora(
            fontWeight: FontWeight.w600,
          ),
          bodyLarge: GoogleFonts.spaceGrotesk(
            color: frost,
          ),
          bodyMedium: GoogleFonts.spaceGrotesk(
            color: frost.withOpacity(0.8),
          ),
          labelLarge: GoogleFonts.spaceGrotesk(
            fontWeight: FontWeight.w600,
          ),
        ),
      ),
      home: const LiveclawHomeScreen(),
    );
  }
}
