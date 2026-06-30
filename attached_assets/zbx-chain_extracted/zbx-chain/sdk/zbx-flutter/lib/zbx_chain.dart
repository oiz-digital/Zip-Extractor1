/// Zebvix Chain Flutter/Dart SDK.
///
/// Production-ready client library for building Flutter and Dart applications
/// on Zebvix Chain. Supports iOS, Android, Web, macOS, Windows, and Linux.
///
/// ## Quick Start
///
/// ```dart
/// import 'package:zbx_chain/zbx_chain.dart';
///
/// final client = ZbxClient(rpcUrl: 'https://testnet-rpc.zebvix.com');
///
/// // Get latest block number
/// final blockNum = await client.getBlockNumber();
///
/// // Create or import a wallet
/// final wallet = Wallet.generate();
/// final balance = await client.getBalance(wallet.address);
/// ```
library zbx_chain;

export 'src/client.dart';
export 'src/wallet.dart';
export 'src/models.dart';
export 'src/constants.dart';
export 'src/utils.dart';
export 'src/exceptions.dart';
export 'src/staking.dart';
export 'src/bridge.dart';
