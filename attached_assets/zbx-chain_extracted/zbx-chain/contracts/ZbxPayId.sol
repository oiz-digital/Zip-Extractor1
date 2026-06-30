// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/**
 * @title ZbxPayId
 * @notice UPI-style Pay ID registry for ZBX Chain.
 *
 * Format (exactly like UPI):
 *   ali@zbx            ← standard format (UPI style)
 *   shop.ali@zbx       ← sub-ID (business issues to customer)
 *
 * On-chain lookup:
 *   resolve("ali@zbx") → 0x742d35Cc...
 *   resolve("ali")     → same (network suffix optional for internal calls)
 *
 * Features:
 * - Register a Pay ID for 0.01 ZBX (prevents name squatting)
 * - Link to any address (EOA or AA smart wallet)
 * - Multi-chain addresses (ZBX, ETH, BTC under one Pay ID)
 * - Transfer / sell your Pay ID
 * - Reverse lookup: address → ali@zbx
 * - Sub-IDs: shop.ali@zbx (businesses issue to customers/branches)
 *
 * @dev ZBX Chain ID: 8989 (mainnet) / 8990 (testnet+devnet shared).
 */
contract ZbxPayId is ReentrancyGuard {

    // ─── Events ───────────────────────────────────────────────────────────

    event PayIdRegistered(string indexed name, address indexed owner, address wallet);
    event PayIdUpdated(string indexed name, address indexed newWallet);
    event PayIdTransferred(string indexed name, address indexed from, address indexed to);
    event PayIdReleased(string indexed name, address indexed owner);
    event ChainAddressSet(string indexed name, uint256 chainId, string chainAddress);
    event SubIdIssued(string indexed parentName, string indexed subId, address indexed to);

    // ─── Structs ──────────────────────────────────────────────────────────

    struct PayIdRecord {
        /// Owner of this Pay ID.
        address owner;
        /// Primary ZBX wallet address.
        address wallet;
        /// Block number at registration.
        uint64  registeredAt;
        /// Whether active.
        bool    active;
    }

    // ─── State ────────────────────────────────────────────────────────────

    /// Registration fee: 0.01 ZBX.
    uint256 public registrationFee = 0.01 ether;

    address public immutable owner;

    /// name (lowercase, without @zbx) → record.
    mapping(string => PayIdRecord) private _records;

    /// Reverse: wallet address → "name@zbx".
    mapping(address => string) private _reverse;

    /// Multi-chain: name → chainId → address string.
    mapping(string => mapping(uint256 => string)) private _chainAddresses;

    uint256 public totalRegistered;

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier onlyPayIdOwner(string memory name) {
        require(_records[_normalize(name)].owner == msg.sender, "ZbxPayId: not owner");
        _;
    }

    modifier onlyOwner() {
        require(msg.sender == owner, "ZbxPayId: not contract owner");
        _;
    }

    constructor() {
        owner = msg.sender;
    }

    // ─── Registration ─────────────────────────────────────────────────────

    /**
     * @notice Register a Pay ID.
     * @param payId  Can be "ali" or "ali@zbx" — both accepted.
     *               Result: ali@zbx maps to wallet.
     * @param wallet Wallet address to link.
     */
    function register(string calldata payId, address wallet) external payable {
        require(msg.value >= registrationFee, "ZbxPayId: insufficient fee");
        require(wallet != address(0), "ZbxPayId: zero address");

        string memory name = _stripSuffix(_normalize(payId));
        _validateName(name);

        require(!_records[name].active, "ZbxPayId: already taken");

        _records[name] = PayIdRecord({
            owner:        msg.sender,
            wallet:       wallet,
            registeredAt: uint64(block.number),
            active:       true
        });

        // Reverse: "ali@zbx"
        if (bytes(_reverse[wallet]).length == 0) {
            _reverse[wallet] = string(abi.encodePacked(name, "@zbx"));
        }

        totalRegistered++;
        emit PayIdRegistered(name, msg.sender, wallet);
    }

    // ─── Update ──────────────────────────────────────────────────────────

    /// Update the linked wallet address.
    function updateWallet(string calldata payId, address newWallet) external {
        string memory name = _stripSuffix(_normalize(payId));
        require(_records[name].owner == msg.sender, "ZbxPayId: not owner");
        require(newWallet != address(0), "ZbxPayId: zero address");

        address oldWallet = _records[name].wallet;
        string memory thisPayId = string(abi.encodePacked(name, "@zbx"));

        if (keccak256(bytes(_reverse[oldWallet])) == keccak256(bytes(thisPayId))) {
            delete _reverse[oldWallet];
        }

        _records[name].wallet = newWallet;

        if (bytes(_reverse[newWallet]).length == 0) {
            _reverse[newWallet] = thisPayId;
        }

        emit PayIdUpdated(name, newWallet);
    }

    /// Set address on another chain (e.g., your BTC or ETH address).
    function setChainAddress(
        string calldata payId,
        uint256 chainId,
        string  calldata chainAddress
    ) external onlyPayIdOwner(payId) {
        string memory name = _stripSuffix(_normalize(payId));
        _chainAddresses[name][chainId] = chainAddress;
        emit ChainAddressSet(name, chainId, chainAddress);
    }

    // ─── Sub-IDs ─────────────────────────────────────────────────────────

    /**
     * @notice Issue a sub-ID.
     *   "ali" issues "shop" → result: shop.ali@zbx
     *
     * @param parentId  Your Pay ID name (e.g., "ali" or "ali@zbx").
     * @param subName   Sub-name (e.g., "shop"). Result: shop.ali@zbx
     * @param to        Wallet address for this sub-ID.
     */
    function issueSubId(
        string calldata parentId,
        string calldata subName,
        address to
    ) external onlyPayIdOwner(parentId) {
        string memory parent = _stripSuffix(_normalize(parentId));
        string memory sub    = _normalize(subName);
        _validateName(sub);

        // Stored as "sub.parent" internally
        string memory fullName = string(abi.encodePacked(sub, ".", parent));
        require(!_records[fullName].active, "ZbxPayId: sub-ID already issued");
        require(to != address(0), "ZbxPayId: zero address");

        _records[fullName] = PayIdRecord({
            owner:        msg.sender,
            wallet:       to,
            registeredAt: uint64(block.number),
            active:       true
        });

        emit SubIdIssued(parent, fullName, to);
    }

    // ─── Transfer ─────────────────────────────────────────────────────────

    /// Transfer Pay ID ownership.
    function transfer(string calldata payId, address newOwner) external {
        string memory name = _stripSuffix(_normalize(payId));
        require(_records[name].owner == msg.sender, "ZbxPayId: not owner");
        require(newOwner != address(0), "ZbxPayId: zero address");
        address prev = _records[name].owner;
        // SEC-2026-05-09 Pass-15 (HIGH-S07 / Pass-12 Tier-2 PayId-stale-reverse):
        // Pre-fix `_reverse[oldWallet]` continued to point at `name`
        // after a transfer, so reverse-lookups for the old wallet
        // returned a stale Pay ID and the new owner had no reverse
        // mapping. Clear the old reverse + repoint to newOwner.
        address oldWallet = _records[name].wallet;
        string memory thisPayId = string(abi.encodePacked(name, "@zbx"));
        if (keccak256(bytes(_reverse[oldWallet])) == keccak256(bytes(thisPayId))) {
            delete _reverse[oldWallet];
        }
        _records[name].owner = newOwner;
        _records[name].wallet = newOwner;
        _reverse[newOwner] = thisPayId;
        emit PayIdTransferred(name, prev, newOwner);
    }

    // ─── Release ──────────────────────────────────────────────────────────

    /// Release (delete) your Pay ID.
    function release(string calldata payId) external {
        string memory name = _stripSuffix(_normalize(payId));
        require(_records[name].owner == msg.sender, "ZbxPayId: not owner");

        address wallet = _records[name].wallet;
        string memory thisPayId = string(abi.encodePacked(name, "@zbx"));

        if (keccak256(bytes(_reverse[wallet])) == keccak256(bytes(thisPayId))) {
            delete _reverse[wallet];
        }

        delete _records[name];
        totalRegistered--;
        emit PayIdReleased(name, msg.sender);
    }

    // ─── Resolution ───────────────────────────────────────────────────────

    /**
     * @notice Resolve a Pay ID to a ZBX address.
     * @param payId Can be "ali", "ali@zbx", or "shop.ali@zbx".
     */
    function resolve(string calldata payId) external view returns (address) {
        string memory name = _stripSuffix(_normalize(payId));
        PayIdRecord storage rec = _records[name];
        require(rec.active, "ZbxPayId: not registered");
        return rec.wallet;
    }

    /// Resolve to address on a specific chain.
    function resolveChain(string calldata payId, uint256 chainId)
        external view returns (string memory)
    {
        string memory name = _stripSuffix(_normalize(payId));
        require(_records[name].active, "ZbxPayId: not registered");
        return _chainAddresses[name][chainId];
    }

    /// Reverse lookup: address → "ali@zbx".
    function reverseLookup(address wallet) external view returns (string memory) {
        return _reverse[wallet]; // returns "ali@zbx" or "" if none
    }

    /// Check availability.
    function isAvailable(string calldata payId) external view returns (bool) {
        return !_records[_stripSuffix(_normalize(payId))].active;
    }

    /// Full record.
    function getRecord(string calldata payId) external view returns (PayIdRecord memory) {
        return _records[_stripSuffix(_normalize(payId))];
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setRegistrationFee(uint256 fee) external onlyOwner {
        registrationFee = fee;
    }

    /// @dev S19: migrated off `.transfer(...)` to `.call{value:...}("")` so
    ///      the immutable `owner` (commonly a multi-sig at deploy time) can
    ///      receive the registration-fee balance regardless of EIP-2929
    ///      cold-account gas costs. `nonReentrant` because the recipient is
    ///      an arbitrary contract chosen at constructor time; even though it
    ///      cannot call back into `withdraw()` (already protected by
    ///      onlyOwner), the cross-function guard blocks reentry into ANY
    ///      future `nonReentrant` flow.
    function withdraw() external onlyOwner nonReentrant {
        uint256 bal = address(this).balance;
        (bool ok, ) = payable(owner).call{value: bal}("");
        require(ok, "ZbxPayId: withdraw failed");
    }

    // ─── Internal ─────────────────────────────────────────────────────────

    /// Strip "@zbx" suffix if present.
    function _stripSuffix(string memory input) internal pure returns (string memory) {
        bytes memory b = bytes(input);
        // Check if ends with "@zbx" (4 bytes)
        if (b.length > 4) {
            bool hasAt = b[b.length-4] == 0x40; // '@'
            bool hasZ  = b[b.length-3] == 0x7A; // 'z'
            bool hasB  = b[b.length-2] == 0x62; // 'b'
            bool hasX  = b[b.length-1] == 0x78; // 'x'
            if (hasAt && hasZ && hasB && hasX) {
                bytes memory stripped = new bytes(b.length - 4);
                for (uint i = 0; i < stripped.length; i++) {
                    stripped[i] = b[i];
                }
                return string(stripped);
            }
        }
        return input;
    }

    /// Lowercase all characters.
    function _normalize(string memory input) internal pure returns (string memory) {
        bytes memory b = bytes(input);
        for (uint i = 0; i < b.length; i++) {
            if (b[i] >= 0x41 && b[i] <= 0x5A) {
                b[i] = bytes1(uint8(b[i]) + 32);
            }
        }
        return string(b);
    }

    /**
     * @dev Validate Pay ID name:
     *   - 3 to 32 chars
     *   - a-z, 0-9, hyphen (-), dot (.) for sub-IDs
     *   - No leading/trailing hyphen
     */
    function _validateName(string memory name) internal pure {
        bytes memory b = bytes(name);
        require(b.length >= 3 && b.length <= 32, "ZbxPayId: length 3-32 required");
        require(b[0] != 0x2D && b[b.length-1] != 0x2D, "ZbxPayId: no leading/trailing hyphen");
        for (uint i = 0; i < b.length; i++) {
            bytes1 c = b[i];
            bool ok = (c >= 0x61 && c <= 0x7A) || // a-z
                      (c >= 0x30 && c <= 0x39) || // 0-9
                      c == 0x2D ||                 // -
                      c == 0x2E;                   // . (sub-IDs)
            require(ok, "ZbxPayId: invalid character");
        }
    }
}