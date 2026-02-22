import 'package:flutter_secure_storage/flutter_secure_storage.dart';

class SecureTokenVault {
  SecureTokenVault({FlutterSecureStorage? storage})
      : _storage = storage ??
            const FlutterSecureStorage(
              iOptions: IOSOptions(
                  accessibility:
                      KeychainAccessibility.first_unlock_this_device),
            );

  static const _pairingTokenKey = 'liveclaw_pairing_token';

  final FlutterSecureStorage _storage;

  Future<void> savePairingToken(String token) async {
    await _storage.write(key: _pairingTokenKey, value: token);
  }

  Future<String?> readPairingToken() {
    return _storage.read(key: _pairingTokenKey);
  }

  Future<void> clearPairingToken() async {
    await _storage.delete(key: _pairingTokenKey);
  }
}
