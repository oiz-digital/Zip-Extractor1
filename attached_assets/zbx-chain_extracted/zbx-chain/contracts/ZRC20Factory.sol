// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ZRC20Token }      from "./ZRC20Token.sol";
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title ZRC20Factory — Deploy new ZRC-20 tokens on Zebvix Chain in one tx.
/// @notice Anyone can call `createToken` to deploy a ZRC-20-compliant token.
///         A small creation fee (in native ZBX) goes to the protocol treasury.
///
/// @dev Uses CREATE2 for deterministic addresses:
///         address = hash(factory, salt, bytecode)
///         This lets you know the token address before deployment.

contract ZRC20Factory is ReentrancyGuard {

    // ─── Events ───────────────────────────────────────────────────────────

    event TokenCreated(
        address indexed creator,
        address indexed token,
        string  name,
        string  symbol,
        uint256 initialSupply,
        bytes32 salt
    );

    // ─── Storage ──────────────────────────────────────────────────────────

    address public immutable treasury;
    uint256 public creationFee;       // in wei (ZBX)
    address[] public allTokens;
    mapping(address => address[]) public tokensByCreator;

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address treasury_, uint256 creationFee_) {
        require(treasury_ != address(0), "ZRC20Factory: zero treasury");
        treasury    = treasury_;
        creationFee = creationFee_; // e.g. 10 ZBX = 10e18
    }

    // ─── Deploy ───────────────────────────────────────────────────────────

    /// @notice Deploy a new ZRC-20 token.
    /// @param name_          Human-readable token name.
    /// @param symbol_        Ticker symbol (1–8 chars recommended).
    /// @param decimals_      Decimal places (18 recommended).
    /// @param initialSupply  Tokens minted to `msg.sender` at deployment.
    /// @param mintCap_       Maximum total supply (0 = unlimited).
    /// @param logoURI_       Token logo (IPFS or HTTPS).
    /// @param salt           CREATE2 salt for deterministic address.
    /// @dev S19: migrated `.transfer(...)` → `.call{value:...}("")` for both
    ///      the refund and the treasury-fee path so that smart-wallet callers
    ///      (multi-sigs, ERC-4337 accounts) and treasury contracts with
    ///      non-trivial `receive()` logic are not silently bricked by the
    ///      2300-gas stipend under EIP-2929 cold-account costs.
    ///
    ///      Critical: this function previously had broken CEI ordering —
    ///      transfers were sent BEFORE the `allTokens.push` / `tokensByCreator`
    ///      state writes. Under `.call`, a malicious caller could reenter via
    ///      `receive()` and spawn a second `createToken` before the first
    ///      one's state was recorded, double-counting indexes. `nonReentrant`
    ///      blocks this attack vector. Functional ordering is preserved
    ///      (refund → fee → deploy → register) to keep the existing event
    ///      semantics and CREATE2 address determinism.
    function createToken(
        string  calldata name_,
        string  calldata symbol_,
        uint8            decimals_,
        uint256          initialSupply,
        uint256          mintCap_,
        string  calldata logoURI_,
        bytes32          salt
    ) external payable nonReentrant returns (address token) {
        require(bytes(name_).length   >= 1  && bytes(name_).length   <= 64,  "ZRC20Factory: invalid name");
        require(bytes(symbol_).length >= 1  && bytes(symbol_).length <= 16,  "ZRC20Factory: invalid symbol");
        require(decimals_ <= 18,                                              "ZRC20Factory: decimals > 18");
        require(msg.value >= creationFee,                                     "ZRC20Factory: insufficient fee");

        // Refund excess fee.
        if (msg.value > creationFee) {
            (bool refundOk, ) = payable(msg.sender).call{value: msg.value - creationFee}("");
            require(refundOk, "ZRC20Factory: refund failed");
        }
        // Forward fee to treasury.
        if (creationFee > 0) {
            (bool feeOk, ) = payable(treasury).call{value: creationFee}("");
            require(feeOk, "ZRC20Factory: treasury fee failed");
        }

        // Deploy with CREATE2.
        // initialSupply is now passed to the constructor and minted in the
        // same tx — closes the previous bug where the factory called mint()
        // post-deploy but was never added as a minter, causing every
        // createToken with initialSupply > 0 to revert. (S16-ZRC20-ADV.)
        bytes memory bytecode = abi.encodePacked(
            type(ZRC20Token).creationCode,
            abi.encode(name_, symbol_, decimals_, initialSupply, mintCap_, logoURI_, msg.sender)
        );
        // S25-Y3 assembly: CREATE2 deploy of new ZRC20Token contract.
        // - Same `add(bytecode,32)` / `mload(bytecode)` pattern as ZbxAMMFactory.
        // - Caller-supplied salt → predictable address (see predictAddress() below).
        // - extcodesize check catches deploy-failure (insufficient gas, ctor revert, addr collision).
        assembly {
            token := create2(0, add(bytecode, 32), mload(bytecode), salt)
            if iszero(extcodesize(token)) { revert(0, 0) }
        }

        allTokens.push(token);
        tokensByCreator[msg.sender].push(token);

        emit TokenCreated(msg.sender, token, name_, symbol_, initialSupply, salt);
    }

    /// @notice Predict the address a token would be deployed to.
    function predictAddress(bytes32 salt, bytes memory bytecode) external view returns (address) {
        bytes32 h = keccak256(abi.encodePacked(bytes1(0xff), address(this), salt, keccak256(bytecode)));
        return address(uint160(uint256(h)));
    }

    function allTokensLength() external view returns (uint256) { return allTokens.length; }
    function tokensOf(address creator) external view returns (address[] memory) { return tokensByCreator[creator]; }
}