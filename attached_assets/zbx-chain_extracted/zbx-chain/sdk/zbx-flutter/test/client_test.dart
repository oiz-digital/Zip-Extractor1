import 'package:flutter_test/flutter_test.dart';
import 'package:zbx_chain/zbx_chain.dart';

void main() {
  group('ZbxClient validation', () {
    test('empty rpcUrl throws ZbxException', () {
      expect(() => ZbxClient(rpcUrl: ''), throwsA(isA<ZbxException>()));
    });
  });

  group('Utils', () {
    test('isValidAddress rejects short strings', () {
      expect(isValidAddress('0x1234'), isFalse);
    });

    test('isValidAddress accepts 20-byte hex', () {
      expect(isValidAddress('0x' + 'ab' * 20), isTrue);
    });

    test('isValidHash accepts 32-byte hex', () {
      expect(isValidHash('0x' + 'cd' * 32), isTrue);
    });

    test('toWei converts 1 ZBX correctly', () {
      expect(toWei(1.0), BigInt.parse('1000000000000000000'));
    });

    test('fromWei converts 1 ZBX correctly', () {
      expect(fromWei(BigInt.parse('1000000000000000000')), closeTo(1.0, 1e-9));
    });

    test('hexToInt converts 0x64 to 100', () {
      expect(hexToInt('0x64'), 100);
    });

    test('hexToBigInt converts large hex', () {
      final v = hexToBigInt('0xde0b6b3a7640000');
      expect(v, BigInt.parse('1000000000000000000'));
    });
  });

  group('Wallet', () {
    test('generate creates wallet with 32-byte key', () {
      final w = Wallet.generate();
      expect(w.privateKeyHex.length, 64);
    });

    test('fromPrivateKey handles 0x prefix', () {
      final key = '0x' + 'aa' * 32;
      final w = Wallet.fromPrivateKey(key);
      expect(w.privateKeyHex, 'aa' * 32);
    });

    test('fromPrivateKey handles no prefix', () {
      final key = 'bb' * 32;
      final w = Wallet.fromPrivateKey(key);
      expect(w.privateKeyHex, key);
    });

    test('invalid hex throws WalletException', () {
      expect(
        () => Wallet.fromPrivateKey('not-valid-hex'),
        throwsA(isA<WalletException>()),
      );
    });

    test('two generated wallets have different keys', () {
      final w1 = Wallet.generate();
      final w2 = Wallet.generate();
      expect(w1.privateKeyHex, isNot(equals(w2.privateKeyHex)));
    });
  });

  group('Constants', () {
    test('chain IDs are distinct', () {
      expect(chainIdMainnet, isNot(equals(chainIdTestnet)));
      expect(chainIdTestnet, isNot(equals(chainIdDevnet)));
    });

    test('weiPerZbx is 10^18', () {
      expect(weiPerZbx, BigInt.from(10).pow(18));
    });
  });
}
