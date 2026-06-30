/// Zebvix SDK exceptions.

/// Base exception for all Zebvix SDK errors.
class ZbxException implements Exception {
  final String message;
  const ZbxException(this.message);

  @override
  String toString() => 'ZbxException: $message';
}

/// JSON-RPC error returned by the node.
class RpcException extends ZbxException {
  final int code;
  const RpcException(this.code, String message) : super(message);

  @override
  String toString() => 'RpcException($code): $message';
}

/// Wallet operation error (key derivation, signing, import).
class WalletException extends ZbxException {
  const WalletException(super.message);

  @override
  String toString() => 'WalletException: $message';
}

/// Input validation error (bad address, hash, amount).
class ValidationException extends ZbxException {
  const ValidationException(super.message);

  @override
  String toString() => 'ValidationException: $message';
}
