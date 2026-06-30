// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title ZbxProxy — UUPS (Universal Upgradeable Proxy Standard) proxy.
/// @notice Allows ZBX Chain contracts to be upgraded without changing address.
///         Storage layout is preserved across upgrades.
///
/// @dev   EIP-1967 compliant: implementation address stored at a known slot.
///        Slot: keccak256("eip1967.proxy.implementation") - 1
///
///        Upgrade process:
///          1. Governance proposes upgrade (new implementation address).
///          2. Timelock waits 48 hours.
///          3. Governance executes: proxy.upgradeTo(newImpl).
///          4. All subsequent calls delegated to newImpl.

contract ZbxProxy {

    // EIP-1967 implementation slot: keccak256("eip1967.proxy.implementation") - 1
    bytes32 private constant IMPL_SLOT =
        0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    // EIP-1967 admin slot: keccak256("eip1967.proxy.admin") - 1
    bytes32 private constant ADMIN_SLOT =
        0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;

    event Upgraded(address indexed implementation);
    event AdminChanged(address indexed previousAdmin, address indexed newAdmin);

    constructor(address implementation, address admin, bytes memory initData) {
        _setImpl(implementation);
        _setAdmin(admin);
        if (initData.length > 0) {
            (bool ok, ) = implementation.delegatecall(initData);
            require(ok, "ZbxProxy: initialisation failed");
        }
    }

    modifier onlyAdmin() {
        require(msg.sender == _admin(), "ZbxProxy: not admin");
        _;
    }

    // ─── Upgrade ──────────────────────────────────────────────────────────

    function upgradeTo(address newImpl) external onlyAdmin {
        require(newImpl.code.length > 0, "ZbxProxy: not a contract");
        _setImpl(newImpl);
        emit Upgraded(newImpl);
    }

    function upgradeToAndCall(address newImpl, bytes calldata data) external onlyAdmin {
        upgradeTo(newImpl);
        (bool ok, ) = newImpl.delegatecall(data);
        require(ok, "ZbxProxy: upgrade call failed");
    }

    function changeAdmin(address newAdmin) external onlyAdmin {
        emit AdminChanged(_admin(), newAdmin);
        _setAdmin(newAdmin);
    }

    // ─── Views ────────────────────────────────────────────────────────────

    function implementation() external view returns (address) { return _impl(); }
    function admin()          external view returns (address) { return _admin(); }

    // ─── Fallback: delegate to implementation ─────────────────────────────

    fallback() external payable {
        address impl = _impl();
        assembly {
            calldatacopy(0, 0, calldatasize())
            let result := delegatecall(gas(), impl, 0, calldatasize(), 0, 0)
            returndatacopy(0, 0, returndatasize())
            switch result
            case 0 { revert(0, returndatasize()) }
            default { return(0, returndatasize()) }
        }
    }

    receive() external payable {}

    // ─── Storage helpers (EIP-1967) ───────────────────────────────────────

    function _impl() private view returns (address impl) {
        assembly { impl := sload(IMPL_SLOT) }
    }
    function _setImpl(address impl) private {
        assembly { sstore(IMPL_SLOT, impl) }
    }
    function _admin() private view returns (address adm) {
        assembly { adm := sload(ADMIN_SLOT) }
    }
    function _setAdmin(address adm) private {
        assembly { sstore(ADMIN_SLOT, adm) }
    }
}