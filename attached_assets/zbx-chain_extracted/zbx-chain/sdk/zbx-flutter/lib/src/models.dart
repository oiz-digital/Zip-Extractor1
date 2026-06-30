/// Zebvix Chain data models.

/// Block header model.
class Block {
  final int number;
  final String hash;
  final String parentHash;
  final String stateRoot;
  final int timestamp;
  final int gasLimit;
  final int gasUsed;
  final String? baseFee;
  final String? proposer;
  final List<String> transactions;

  const Block({
    required this.number,
    required this.hash,
    required this.parentHash,
    required this.stateRoot,
    required this.timestamp,
    required this.gasLimit,
    required this.gasUsed,
    this.baseFee,
    this.proposer,
    this.transactions = const [],
  });

  factory Block.fromJson(Map<String, dynamic> json) => Block(
    number:       _hexToInt(json['number'] as String? ?? '0x0'),
    hash:         json['hash'] as String? ?? '0x',
    parentHash:   json['parentHash'] as String? ?? '0x',
    stateRoot:    json['stateRoot'] as String? ?? '0x',
    timestamp:    _hexToInt(json['timestamp'] as String? ?? '0x0'),
    gasLimit:     _hexToInt(json['gasLimit'] as String? ?? '0x0'),
    gasUsed:      _hexToInt(json['gasUsed'] as String? ?? '0x0'),
    baseFee:      json['baseFeePerGas'] as String?,
    proposer:     json['miner'] as String?,
    transactions: (json['transactions'] as List<dynamic>? ?? [])
        .whereType<String>().toList(),
  );
}

/// Transaction model.
class Transaction {
  final String hash;
  final int? blockNumber;
  final String fromAddr;
  final String? toAddr;
  final BigInt value;
  final int gas;
  final int nonce;
  final int txType;
  final bool? status;

  const Transaction({
    required this.hash,
    this.blockNumber,
    required this.fromAddr,
    this.toAddr,
    required this.value,
    required this.gas,
    required this.nonce,
    required this.txType,
    this.status,
  });

  factory Transaction.fromJson(Map<String, dynamic> json) => Transaction(
    hash:        json['hash'] as String,
    blockNumber: json['blockNumber'] != null
        ? _hexToInt(json['blockNumber'] as String) : null,
    fromAddr:    json['from'] as String,
    toAddr:      json['to'] as String?,
    value:       BigInt.parse(
        (json['value'] as String? ?? '0x0').replaceFirst('0x', ''),
        radix: 16,
    ),
    gas:    _hexToInt(json['gas'] as String? ?? '0x0'),
    nonce:  _hexToInt(json['nonce'] as String? ?? '0x0'),
    txType: _hexToInt(json['type'] as String? ?? '0x0'),
  );
}

/// Validator model.
class Validator {
  final String address;
  final String pubKey;
  final BigInt stake;
  final BigInt delegatedStake;
  final double commission;
  final String status;
  final double uptimePct;
  final int blocksProduced;

  const Validator({
    required this.address,
    required this.pubKey,
    required this.stake,
    required this.delegatedStake,
    required this.commission,
    required this.status,
    required this.uptimePct,
    required this.blocksProduced,
  });

  factory Validator.fromJson(Map<String, dynamic> json) => Validator(
    address:        json['address'] as String,
    pubKey:         json['pubKey'] as String? ?? '',
    stake:          BigInt.parse(json['stake'] as String? ?? '0'),
    delegatedStake: BigInt.parse(json['delegatedStake'] as String? ?? '0'),
    commission:     (json['commission'] as num? ?? 0).toDouble(),
    status:         json['status'] as String? ?? 'unknown',
    uptimePct:      (json['uptimePct'] as num? ?? 0).toDouble(),
    blocksProduced: (json['blocksProduced'] as num? ?? 0).toInt(),
  );
}

int _hexToInt(String hex) {
  if (hex.startsWith('0x') || hex.startsWith('0X')) {
    hex = hex.substring(2);
  }
  if (hex.isEmpty) return 0;
  return int.parse(hex, radix: 16);
}
