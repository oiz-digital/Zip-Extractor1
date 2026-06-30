/// Utility functions for Zebvix Flutter SDK.

import 'constants.dart';
import 'exceptions.dart';

final _hexAddressRe = RegExp(r'^0x[0-9a-fA-F]{40}$');
final _hexHashRe    = RegExp(r'^0x[0-9a-fA-F]{64}$');

/// Returns true if [addr] is a valid 20-byte hex address.
bool isValidAddress(String addr) => _hexAddressRe.hasMatch(addr);

/// Returns true if [hash] is a valid 32-byte hex hash.
bool isValidHash(String hash) => _hexHashRe.hasMatch(hash);

/// Convert ZBX amount to wei.
///
/// ```dart
/// toWei(1.0)  // 1000000000000000000
/// toWei(0.5)  // 500000000000000000
/// ```
BigInt toWei(double zbx) {
  final scaled = (zbx * 1e18).toInt();
  return BigInt.from(scaled);
}

/// Convert wei to ZBX.
///
/// ```dart
/// fromWei(BigInt.from(10).pow(18))  // 1.0
/// ```
double fromWei(BigInt wei) {
  return wei / weiPerZbx;
}

/// Convert a 0x-prefixed hex string to BigInt.
BigInt hexToBigInt(String hex) {
  final cleaned = hex.startsWith('0x') ? hex.substring(2) : hex;
  if (cleaned.isEmpty) return BigInt.zero;
  return BigInt.parse(cleaned, radix: 16);
}

/// Convert a 0x-prefixed hex string to int.
int hexToInt(String hex) => hexToBigInt(hex).toInt();

/// Convert int to 0x-prefixed hex string.
String intToHex(int n) => '0x${n.toRadixString(16)}';

/// Validate address or throw [ValidationException].
void requireValidAddress(String addr, [String name = 'address']) {
  if (!isValidAddress(addr)) {
    throw ValidationException(
      "invalid $name: '$addr' must be 0x + 20 hex bytes",
    );
  }
}

/// Validate hash or throw [ValidationException].
void requireValidHash(String hash, [String name = 'hash']) {
  if (!isValidHash(hash)) {
    throw ValidationException(
      "invalid $name: '$hash' must be 0x + 32 hex bytes",
    );
  }
}
