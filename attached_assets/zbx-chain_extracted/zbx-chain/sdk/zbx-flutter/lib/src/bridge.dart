/// Bridge helper for Zebvix Chain Flutter SDK.

import 'client.dart';
import 'utils.dart';
import 'exceptions.dart';

/// High-level bridge operations for testnet chains.
class BridgeHelper {
  final ZbxClient _client;

  BridgeHelper(this._client);

  /// Get bridge operational status.
  Future<Map<String, dynamic>> getStatus() async =>
      await _client._call('zbx_getBridgeStatus') ?? {};

  /// Encode a bridge deposit call (lock ZBX for cross-chain transfer).
  ///
  /// Encodes `depositFor(uint256 destChainId, address recipient, uint256 amount)`.
  String encodeDepositCall({
    required int destChainId,
    required String recipient,
    required BigInt amountWei,
  }) {
    requireValidAddress(recipient, 'recipient');
    const selector = '0x8340f549';
    final chainIdHex   = BigInt.from(destChainId).toRadixString(16).padLeft(64, '0');
    final recipientHex = recipient.substring(2).toLowerCase().padLeft(64, '0');
    final amountHex    = amountWei.toRadixString(16).padLeft(64, '0');
    return '$selector$chainIdHex$recipientHex$amountHex';
  }
}

// Internal access — package:zbx_chain exposes _call via friend pattern.
extension _BridgeClientAccess on ZbxClient {
  Future<dynamic> _call(String method, [List<dynamic> params = const []]) =>
      throw UnimplementedError('Access via ZbxClient._call');
}
