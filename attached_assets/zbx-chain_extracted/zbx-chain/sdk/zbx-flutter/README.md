# zbx_chain — Zebvix Chain Flutter SDK

Production Flutter/Dart SDK for Zebvix Chain. Supports iOS, Android, Web, macOS, Windows, and Linux.

## Installation

```yaml
dependencies:
  zbx_chain: ^1.0.0
```

```bash
flutter pub get
```

## Quick Start

```dart
import 'package:zbx_chain/zbx_chain.dart';

final client = ZbxClient(rpcUrl: 'https://testnet-rpc.zebvix.com');

// Get latest block
final blockNum = await client.getBlockNumber();
print('Block: $blockNum');

// Get balance
final wallet = Wallet.generate();
final balWei = await client.getBalance(wallet.address);
final balZbx = fromWei(balWei);
print('Balance: $balZbx ZBX');

// Get validators
final validators = await client.getValidators();
print('Validators: ${validators.length}');

// Staking helper
final staking = StakingHelper(client);
final calldata = staking.encodeStakeCall(
  '0xValidatorAddress...',
  toWei(1000.0), // 1000 ZBX
);
```

## Chain IDs

| Network | Chain ID |
|---------|----------|
| Mainnet | 8989     |
| Testnet | 8990     |
| Devnet  | 8991     |

## Running Tests

```bash
flutter test
```
