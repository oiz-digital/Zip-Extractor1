/// Staking helper for Zebvix Chain Flutter SDK.

import 'client.dart';
import 'constants.dart';
import 'utils.dart';

/// High-level staking operations.
class StakingHelper {
  final ZbxClient _client;

  StakingHelper(this._client);

  /// Get staking info for a delegator address.
  Future<Map<String, dynamic>> getInfo(String address) =>
      _client.getStakingInfo(address);

  /// Get all active validators.
  Future<List<Map<String, dynamic>>> getValidators() =>
      _client.getValidators();

  /// Encode a `stake(address validator, uint256 amount)` call for submission.
  String encodeStakeCall(String validatorAddress, BigInt amountWei) {
    requireValidAddress(validatorAddress, 'validatorAddress');
    // Function selector: keccak256("stake(address,uint256)")[0:4]
    const selector = '0xa694fc3a';
    final addrPadded = validatorAddress
        .substring(2)
        .toLowerCase()
        .padLeft(64, '0');
    final amountHex = amountWei.toRadixString(16).padLeft(64, '0');
    return '$selector$addrPadded$amountHex';
  }

  /// Encode an `unstake(address validator, uint256 amount)` call.
  String encodeUnstakeCall(String validatorAddress, BigInt amountWei) {
    requireValidAddress(validatorAddress, 'validatorAddress');
    const selector = '0x2e17de78';
    final addrPadded = validatorAddress
        .substring(2)
        .toLowerCase()
        .padLeft(64, '0');
    final amountHex = amountWei.toRadixString(16).padLeft(64, '0');
    return '$selector$addrPadded$amountHex';
  }

  /// Encode a `claimRewards(address validator)` call.
  String encodeClaimRewardsCall(String validatorAddress) {
    requireValidAddress(validatorAddress, 'validatorAddress');
    const selector = '0xef5cfb8c';
    final addrPadded = validatorAddress
        .substring(2)
        .toLowerCase()
        .padLeft(64, '0');
    return '$selector$addrPadded';
  }
}
