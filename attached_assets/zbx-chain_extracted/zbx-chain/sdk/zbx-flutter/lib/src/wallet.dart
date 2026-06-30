/// Zebvix Chain wallet for Flutter — key generation, import, and signing.

import 'dart:math';
import 'dart:typed_data';
import 'package:hex/hex.dart';
import 'exceptions.dart';
import 'constants.dart';

/// An secp256k1 wallet for Zebvix Chain.
///
/// ```dart
/// // Generate a new wallet
/// final wallet = Wallet.generate();
/// print(wallet.address);
///
/// // Import from private key
/// final wallet = Wallet.fromPrivateKey('0xdeadbeef...');
/// ```
class Wallet {
  final Uint8List _privateKey;
  final int chainId;
  String? _address;

  Wallet._(this._privateKey, {this.chainId = chainIdTestnet}) {
    if (_privateKey.length != 32) {
      throw WalletException(
        'Private key must be 32 bytes, got ${_privateKey.length}',
      );
    }
  }

  // ── Factory methods ───────────────────────────────────────────────────────

  /// Generate a new cryptographically secure random wallet.
  factory Wallet.generate({int chainId = chainIdTestnet}) {
    final rng = Random.secure();
    final key = Uint8List.fromList(
      List.generate(32, (_) => rng.nextInt(256)),
    );
    return Wallet._(key, chainId: chainId);
  }

  /// Import from a hex private key (with or without 0x prefix).
  factory Wallet.fromPrivateKey(String keyHex, {int chainId = chainIdTestnet}) {
    final cleaned = keyHex.startsWith('0x') ? keyHex.substring(2) : keyHex;
    late Uint8List bytes;
    try {
      bytes = Uint8List.fromList(HEX.decode(cleaned));
    } catch (e) {
      throw WalletException('Invalid hex private key: $e');
    }
    return Wallet._(bytes, chainId: chainId);
  }

  // ── Properties ────────────────────────────────────────────────────────────

  /// Hex-encoded private key (without 0x prefix).
  String get privateKeyHex => HEX.encode(_privateKey);

  /// 0x-prefixed private key hex.
  String get privateKey0x => '0x${HEX.encode(_privateKey)}';

  /// EIP-55 checksummed Ethereum-compatible address.
  ///
  /// Note: full address derivation (secp256k1 pubkey → keccak256 → checksum)
  /// requires a native crypto library. This stub returns a deterministic
  /// placeholder derived from the key bytes.
  String get address {
    _address ??= _deriveAddress();
    return _address!;
  }

  // ── Signing (stubs — wire to native secp256k1) ────────────────────────────

  /// Sign an EIP-1559 or legacy transaction dict.
  ///
  /// Production: use flutter_libsecp256k1 or web3dart.
  Map<String, dynamic> signTransaction(Map<String, dynamic> tx) {
    // Stub: production implementation signs with secp256k1 + EIP-155.
    throw WalletException(
      'signTransaction requires web3dart package: flutter pub add web3dart',
    );
  }

  /// Sign a personal message (EIP-191).
  ///
  /// Production: use web3dart SignedMessage.
  String signMessage(String message) {
    throw WalletException(
      'signMessage requires web3dart package: flutter pub add web3dart',
    );
  }

  // ── Internal ──────────────────────────────────────────────────────────────

  String _deriveAddress() {
    // Deterministic placeholder — real derivation needs secp256k1 pubkey → keccak256.
    final prefix = HEX.encode(_privateKey.sublist(12, 20));
    return '0x${prefix.padLeft(40, '0')}';
  }

  @override
  String toString() => 'Wallet(address=$address, chainId=$chainId)';
}
