/// Zebvix Chain JSON-RPC client for Flutter/Dart.

import 'dart:convert';
import 'package:http/http.dart' as http;
import 'exceptions.dart';
import 'models.dart';
import 'utils.dart';

/// Async JSON-RPC 2.0 client for Zebvix Chain.
///
/// ```dart
/// final client = ZbxClient(rpcUrl: 'https://testnet-rpc.zebvix.com');
/// final blockNum = await client.getBlockNumber();
/// ```
class ZbxClient {
  final String rpcUrl;
  final Duration timeout;
  int _id = 0;
  int? _chainId;

  ZbxClient({
    required this.rpcUrl,
    this.timeout = const Duration(seconds: 30),
  }) {
    if (rpcUrl.isEmpty) throw ZbxException('rpcUrl must not be empty');
  }

  /// The chain ID of the connected network (fetched lazily).
  int? get chainId => _chainId;

  // ── Internal RPC call ─────────────────────────────────────────────────────

  Future<dynamic> _call(String method, [List<dynamic> params = const []]) async {
    _id++;
    final payload = jsonEncode({
      'jsonrpc': '2.0',
      'method': method,
      'params': params,
      'id': _id,
    });

    late http.Response response;
    try {
      response = await http.post(
        Uri.parse(rpcUrl),
        headers: {'Content-Type': 'application/json'},
        body: payload,
      ).timeout(timeout);
    } catch (e) {
      throw ZbxException('HTTP error: $e');
    }

    final data = jsonDecode(response.body) as Map<String, dynamic>;
    if (data.containsKey('error')) {
      final err = data['error'] as Map<String, dynamic>;
      throw RpcException(
        err['code'] as int? ?? -1,
        err['message'] as String? ?? 'unknown',
      );
    }
    return data['result'];
  }

  // ── eth_* methods ─────────────────────────────────────────────────────────

  /// Get the chain ID (eth_chainId).
  Future<int> getChainId() async {
    final result = await _call('eth_chainId') as String;
    _chainId = hexToInt(result);
    return _chainId!;
  }

  /// Get the latest block number (eth_blockNumber).
  Future<int> getBlockNumber() async {
    final result = await _call('eth_blockNumber') as String;
    return hexToInt(result);
  }

  /// Get ZBX balance of [address] in wei (eth_getBalance).
  Future<BigInt> getBalance(String address, [String block = 'latest']) async {
    requireValidAddress(address);
    final result = await _call('eth_getBalance', [address, block]) as String;
    return hexToBigInt(result);
  }

  /// Get the transaction count (nonce) for [address].
  Future<int> getNonce(String address, [String block = 'latest']) async {
    requireValidAddress(address);
    final result = await _call('eth_getTransactionCount', [address, block]) as String;
    return hexToInt(result);
  }

  /// Get the current gas price in wei.
  Future<BigInt> getGasPrice() async {
    final result = await _call('eth_gasPrice') as String;
    return hexToBigInt(result);
  }

  /// Get a block by number or tag (eth_getBlockByNumber).
  Future<Block?> getBlock(dynamic blockNumberOrTag, {bool fullTransactions = false}) async {
    final tag = blockNumberOrTag is int
        ? '0x${blockNumberOrTag.toRadixString(16)}'
        : blockNumberOrTag as String;
    final result = await _call('eth_getBlockByNumber', [tag, fullTransactions]);
    if (result == null) return null;
    return Block.fromJson(result as Map<String, dynamic>);
  }

  /// Get the latest block.
  Future<Block?> getLatestBlock({bool fullTransactions = false}) =>
      getBlock('latest', fullTransactions: fullTransactions);

  /// Get a transaction by hash.
  Future<Transaction?> getTransaction(String txHash) async {
    requireValidHash(txHash, 'txHash');
    final result = await _call('eth_getTransactionByHash', [txHash]);
    if (result == null) return null;
    return Transaction.fromJson(result as Map<String, dynamic>);
  }

  /// Get a transaction receipt by hash.
  Future<Map<String, dynamic>?> getReceipt(String txHash) async {
    requireValidHash(txHash, 'txHash');
    final result = await _call('eth_getTransactionReceipt', [txHash]);
    if (result == null) return null;
    return result as Map<String, dynamic>;
  }

  /// Broadcast a signed raw transaction.
  Future<String> sendRawTransaction(String rawTx) async {
    if (!rawTx.startsWith('0x')) {
      throw ZbxException('rawTx must be 0x-prefixed RLP hex');
    }
    return await _call('eth_sendRawTransaction', [rawTx]) as String;
  }

  /// Execute a read-only contract call.
  Future<String> call(String to, String data, [String block = 'latest']) async {
    requireValidAddress(to, 'to');
    return await _call('eth_call', [{'to': to, 'data': data}, block]) as String;
  }

  /// Estimate gas for a transaction.
  Future<int> estimateGas({
    required String to,
    String data = '0x',
    String? from,
    BigInt? value,
  }) async {
    requireValidAddress(to, 'to');
    final params = <String, dynamic>{'to': to, 'data': data};
    if (from != null) params['from'] = from;
    if (value != null) params['value'] = '0x${value.toRadixString(16)}';
    final result = await _call('eth_estimateGas', [params]) as String;
    return hexToInt(result);
  }

  // ── zbx_* methods ─────────────────────────────────────────────────────────

  /// Get the active validator set.
  Future<List<Map<String, dynamic>>> getValidators() async {
    final result = await _call('zbx_getValidators');
    if (result == null) return [];
    return (result as List<dynamic>).cast<Map<String, dynamic>>();
  }

  /// Get the current epoch number.
  Future<int> getEpoch() async {
    final result = await _call('zbx_getEpoch') as String;
    return hexToInt(result);
  }

  /// Get staking information for a delegator.
  Future<Map<String, dynamic>> getStakingInfo(String address) async {
    requireValidAddress(address);
    final result = await _call('zbx_getStakingInfo', [address]);
    return (result ?? {}) as Map<String, dynamic>;
  }

  /// Get oracle price for a trading pair (e.g. 'ZBX/USD').
  Future<BigInt?> getOraclePrice(String pair) async {
    final result = await _call('zbx_getOraclePrice', [pair]);
    if (result == null) return null;
    if (result is String) return hexToBigInt(result);
    return BigInt.from(result as int);
  }
}
