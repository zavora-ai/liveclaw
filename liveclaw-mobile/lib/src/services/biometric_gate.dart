import 'package:local_auth/local_auth.dart';

class BiometricGate {
  BiometricGate({LocalAuthentication? auth}) : _auth = auth ?? LocalAuthentication();

  final LocalAuthentication _auth;

  Future<bool> unlock() async {
    final canCheck = await _auth.canCheckBiometrics;
    final isSupported = await _auth.isDeviceSupported();
    if (!canCheck && !isSupported) {
      return true;
    }

    try {
      return await _auth.authenticate(
        localizedReason: 'Unlock Liveclaw session controls',
        options: const AuthenticationOptions(
          biometricOnly: false,
          stickyAuth: true,
          useErrorDialogs: true,
        ),
      );
    } catch (_) {
      return false;
    }
  }
}
