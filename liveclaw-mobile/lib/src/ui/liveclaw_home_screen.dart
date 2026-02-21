import 'package:flutter/material.dart';
import 'package:flutter_riverpod/flutter_riverpod.dart';

import '../models/session_models.dart';
import '../state/voice_session_controller.dart';
import 'widgets/action_lane.dart';
import 'widgets/resonance_orb.dart';
import 'widgets/transcript_timeline.dart';

class LiveclawHomeScreen extends ConsumerStatefulWidget {
  const LiveclawHomeScreen({super.key});

  @override
  ConsumerState<LiveclawHomeScreen> createState() => _LiveclawHomeScreenState();
}

class _LiveclawHomeScreenState extends ConsumerState<LiveclawHomeScreen> {
  late final TextEditingController _gatewayController;
  final TextEditingController _pairCodeController = TextEditingController();
  final TextEditingController _promptController = TextEditingController(
    text: 'Summarize this voice turn and list the next best spoken step.',
  );

  @override
  void initState() {
    super.initState();
    final initial = ref.read(voiceSessionControllerProvider);
    _gatewayController = TextEditingController(text: initial.gatewayUrl);
  }

  @override
  void dispose() {
    _gatewayController.dispose();
    _pairCodeController.dispose();
    _promptController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final state = ref.watch(voiceSessionControllerProvider);
    final controller = ref.read(voiceSessionControllerProvider.notifier);

    final canSpeak =
        state.sessionId != null && (state.stage == ConnectionStage.sessionReady || state.stage == ConnectionStage.streaming);

    return Scaffold(
      body: Stack(
        children: <Widget>[
          const _Backdrop(),
          SafeArea(
            child: SingleChildScrollView(
              padding: const EdgeInsets.fromLTRB(16, 14, 16, 28),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: <Widget>[
                  _Header(stage: state.stage),
                  const SizedBox(height: 12),
                  _securityDeck(context, state, controller),
                  const SizedBox(height: 14),
                  Center(
                    child: ResonanceOrb(
                      enabled: canSpeak,
                      recording: state.recording,
                      micLevel: state.micLevel,
                      onPressStart: () => controller.startPushToTalk(),
                      onPressEnd: () => controller.stopPushToTalk(),
                    ),
                  ),
                  const SizedBox(height: 8),
                  Text(
                    state.sessionId == null
                        ? 'No active session'
                        : 'Session ${state.sessionId}',
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.bodySmall?.copyWith(
                          color: Colors.white70,
                        ),
                  ),
                  const SizedBox(height: 12),
                  _sessionControls(context, state, controller),
                  const SizedBox(height: 14),
                  Text('Action Lane', style: Theme.of(context).textTheme.titleMedium),
                  const SizedBox(height: 8),
                  ActionLane(entries: state.toolActivity),
                  const SizedBox(height: 14),
                  Text('Transcript', style: Theme.of(context).textTheme.titleMedium),
                  const SizedBox(height: 8),
                  TranscriptTimeline(items: state.transcript),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _securityDeck(
    BuildContext context,
    VoiceSessionState state,
    VoiceSessionController controller,
  ) {
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: const Color(0x4D0A2028),
        borderRadius: BorderRadius.circular(18),
        border: Border.all(color: Colors.white10),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('Trust Ring', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 10),
          TextField(
            controller: _gatewayController,
            decoration: const InputDecoration(
              labelText: 'Gateway WebSocket URL',
              hintText: 'ws://127.0.0.1:8080/ws',
              prefixIcon: Icon(Icons.router_rounded),
            ),
            onChanged: controller.setGatewayUrl,
          ),
          const SizedBox(height: 10),
          Row(
            children: <Widget>[
              Expanded(
                child: TextField(
                  controller: _pairCodeController,
                  keyboardType: TextInputType.number,
                  maxLength: 6,
                  decoration: const InputDecoration(
                    labelText: 'Pairing Code',
                    counterText: '',
                    prefixIcon: Icon(Icons.key_rounded),
                  ),
                ),
              ),
              const SizedBox(width: 10),
              FilledButton.icon(
                onPressed: () => controller.pairWithCode(_pairCodeController.text),
                icon: const Icon(Icons.link_rounded),
                label: const Text('Pair'),
              ),
            ],
          ),
          const SizedBox(height: 8),
          FilledButton.tonalIcon(
            onPressed: () {
              controller.setGatewayUrl(_gatewayController.text);
              controller.secureConnect();
            },
            icon: const Icon(Icons.verified_user_rounded),
            label: const Text('Secure Connect'),
          ),
          const SizedBox(height: 12),
          SegmentedButton<UserRole>(
            showSelectedIcon: false,
            segments: const <ButtonSegment<UserRole>>[
              ButtonSegment<UserRole>(
                value: UserRole.readOnly,
                label: Text('ReadOnly'),
              ),
              ButtonSegment<UserRole>(
                value: UserRole.supervised,
                label: Text('Supervised'),
              ),
              ButtonSegment<UserRole>(
                value: UserRole.full,
                label: Text('Full'),
              ),
            ],
            selected: <UserRole>{state.role},
            onSelectionChanged: (selection) {
              if (selection.isEmpty) {
                return;
              }
              controller.setRole(selection.first);
            },
          ),
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              _badge('Paired', state.paired ? 'YES' : 'NO', state.paired),
              _badge('Biometric', state.biometricUnlocked ? 'UNLOCKED' : 'LOCKED', state.biometricUnlocked),
              _badge('Protocol', state.protocolVersion.isEmpty ? '--' : state.protocolVersion, true),
              _badge('Runtime', state.runtimeKind.isEmpty ? '--' : state.runtimeKind, true),
              _badge('Provider', state.providerKind.isEmpty ? '--' : state.providerKind, true),
              _badge('Public Bind', state.publicBindAllowed ? 'ON' : 'OFF', !state.publicBindAllowed),
            ],
          ),
          if (state.principalId != null) ...<Widget>[
            const SizedBox(height: 8),
            Text(
              'Principal: ${state.principalId}',
              style: Theme.of(context).textTheme.bodySmall,
            ),
          ],
          if (state.errorMessage != null) ...<Widget>[
            const SizedBox(height: 8),
            Text(
              state.errorMessage!,
              style: Theme.of(context).textTheme.bodySmall?.copyWith(
                    color: Theme.of(context).colorScheme.secondary,
                  ),
            ),
          ],
        ],
      ),
    );
  }

  Widget _sessionControls(
    BuildContext context,
    VoiceSessionState state,
    VoiceSessionController controller,
  ) {
    return Container(
      padding: const EdgeInsets.all(14),
      decoration: BoxDecoration(
        color: const Color(0x40132128),
        borderRadius: BorderRadius.circular(16),
        border: Border.all(color: Colors.white12),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: <Widget>[
          Text('Operator Moves', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 10),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: <Widget>[
              FilledButton.icon(
                onPressed: (state.sessionId == null && state.stage == ConnectionStage.authenticated)
                    ? controller.createSession
                    : null,
                icon: const Icon(Icons.play_circle_fill_rounded),
                label: const Text('Create Session'),
              ),
              FilledButton.tonalIcon(
                onPressed: state.sessionId == null ? null : controller.createMemoryPulse,
                icon: const Icon(Icons.auto_awesome_rounded),
                label: const Text('Memory Pulse'),
              ),
              OutlinedButton.icon(
                onPressed: state.sessionId == null ? null : controller.interruptResponse,
                icon: const Icon(Icons.pause_circle_filled_rounded),
                label: const Text('Interrupt'),
              ),
              OutlinedButton.icon(
                onPressed: state.sessionId == null ? null : controller.terminateSession,
                icon: const Icon(Icons.stop_circle_outlined),
                label: const Text('Terminate'),
              ),
            ],
          ),
          const SizedBox(height: 10),
          TextField(
            controller: _promptController,
            maxLines: 2,
            decoration: InputDecoration(
              labelText: 'Quick prompt lane',
              suffixIcon: IconButton(
                onPressed: state.sessionId == null
                    ? null
                    : () => controller.sendPrompt(_promptController.text),
                icon: const Icon(Icons.send_rounded),
              ),
            ),
          ),
        ],
      ),
    );
  }

  Widget _badge(String label, String value, bool positive) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(999),
        color: positive ? const Color(0x2732E0C4) : const Color(0x27F66B3D),
        border: Border.all(
          color: positive ? const Color(0x9932E0C4) : const Color(0x99F66B3D),
        ),
      ),
      child: RichText(
        text: TextSpan(
          style: Theme.of(context).textTheme.labelSmall,
          children: <TextSpan>[
            TextSpan(text: '$label ', style: const TextStyle(color: Colors.white70)),
            TextSpan(text: value, style: const TextStyle(color: Colors.white)),
          ],
        ),
      ),
    );
  }
}

class _Header extends StatelessWidget {
  const _Header({required this.stage});

  final ConnectionStage stage;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: <Widget>[
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: <Widget>[
              Text('Liveclaw Resonance', style: Theme.of(context).textTheme.headlineSmall),
              const SizedBox(height: 4),
              Text(
                'Audio-first command deck for secure realtime sessions.',
                style: Theme.of(context).textTheme.bodyMedium,
              ),
            ],
          ),
        ),
        const SizedBox(width: 10),
        Container(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
          decoration: BoxDecoration(
            borderRadius: BorderRadius.circular(999),
            color: const Color(0x40295166),
            border: Border.all(color: Colors.white12),
          ),
          child: Text(_stageLabel(stage)),
        ),
      ],
    );
  }

  static String _stageLabel(ConnectionStage stage) {
    return switch (stage) {
      ConnectionStage.disconnected => 'Offline',
      ConnectionStage.connecting => 'Connecting',
      ConnectionStage.connected => 'Connected',
      ConnectionStage.authenticated => 'Authenticated',
      ConnectionStage.sessionReady => 'Session Ready',
      ConnectionStage.streaming => 'Live Streaming',
      ConnectionStage.error => 'Attention',
    };
  }
}

class _Backdrop extends StatelessWidget {
  const _Backdrop();

  @override
  Widget build(BuildContext context) {
    return DecoratedBox(
      decoration: const BoxDecoration(
        gradient: LinearGradient(
          begin: Alignment.topLeft,
          end: Alignment.bottomRight,
          colors: <Color>[Color(0xFF07141A), Color(0xFF102631), Color(0xFF051015)],
          stops: <double>[0.0, 0.62, 1.0],
        ),
      ),
      child: Stack(
        children: <Widget>[
          Positioned(
            top: -40,
            right: -20,
            child: _GlowBlob(color: const Color(0x332AE6D6), size: 180),
          ),
          Positioned(
            top: 290,
            left: -50,
            child: _GlowBlob(color: const Color(0x33F4A261), size: 220),
          ),
          Positioned(
            bottom: -70,
            right: -40,
            child: _GlowBlob(color: const Color(0x331EA1D2), size: 240),
          ),
        ],
      ),
    );
  }
}

class _GlowBlob extends StatelessWidget {
  const _GlowBlob({required this.color, required this.size});

  final Color color;
  final double size;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: color,
        boxShadow: <BoxShadow>[
          BoxShadow(color: color, blurRadius: 70, spreadRadius: 10),
        ],
      ),
    );
  }
}
