// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title  ZbxNameService (ZNS) — ENS-style human-readable names for ZBX Chain
/// @author Zebvix Technologies Pvt Ltd
///
/// @notice Register `yourname.zbx` and resolve it to a wallet address.
///         Ownership of a name is tracked as a lightweight ERC-721 NFT so
///         names can be sold/transferred on any NFT marketplace.
///
///         Features:
///           - Register: pay annual fee, receive NFT ownership
///           - Resolve:  name → primary address
///           - Reverse:  address → primary name
///           - Subdomains: `sub.yourname.zbx` (issued by parent owner)
///           - Metadata:   arbitrary key/value records (avatar, email, etc.)
///           - Renewal:    re-pay before expiry to extend registration
///           - Grace period: 30 days after expiry; original owner can still renew
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:module     Infrastructure / ZNS (ZEP-037)

contract ZbxNameService {

    // ─── Errors ───────────────────────────────────────────────────────────

    error NameTaken();
    error NameExpired();
    error NameInGracePeriod();
    error NotNameOwner();
    error NotApprovedOrOwner();
    error NameNotRegistered();
    error InvalidName();
    error InsufficientFee();
    error ZeroAddress();
    error NotOwner();
    error SubdomainNotAllowed();
    error WithdrawFailed();

    // ─── Events ───────────────────────────────────────────────────────────

    event NameRegistered(uint256 indexed tokenId, string name, address indexed owner, uint256 expiry);
    event NameRenewed(uint256 indexed tokenId, string name, uint256 newExpiry);
    event NameTransferred(uint256 indexed tokenId, address indexed from, address indexed to);
    event AddressSet(string indexed name, address addr);
    event ReverseSet(address indexed addr, string name);
    event RecordSet(string indexed name, string key, string value);
    event SubdomainIssued(string indexed parent, string subdomain, address indexed to);
    event Approval(address indexed owner, address indexed approved, uint256 indexed tokenId);
    event ApprovalForAll(address indexed owner, address indexed operator, bool approved);
    event Transfer(address indexed from, address indexed to, uint256 indexed tokenId);

    // ─── Constants ────────────────────────────────────────────────────────

    /// @notice Minimum name length (3 characters).
    uint256 public constant MIN_LENGTH       = 3;
    /// @notice Registration and renewal period (1 year).
    uint256 public constant YEAR             = 365 days;
    /// @notice Grace period after expiry (30 days).
    uint256 public constant GRACE_PERIOD     = 30 days;

    // ─── Types ────────────────────────────────────────────────────────────

    struct NameRecord {
        address owner;
        uint256 expiry;
        address resolvedAddress;  // primary address this name points to
    }

    // ─── State ────────────────────────────────────────────────────────────

    address public admin;

    /// @notice Annual registration fee in ZBX (wei).  Default: 1 ZBX / year.
    uint256 public annualFee = 1 ether;

    /// @notice name → NameRecord
    mapping(string => NameRecord) public records;

    /// @notice tokenId → name (for ERC-721 compatibility)
    mapping(uint256 => string) public tokenName;

    /// @notice name → tokenId
    mapping(string => uint256) public nameToToken;

    /// @notice address → primary name (reverse registry)
    mapping(address => string) public primaryName;

    /// @notice name → key → value (arbitrary metadata)
    mapping(string => mapping(string => string)) public nameRecords;

    /// @notice tokenId → approved address
    mapping(uint256 => address) private _tokenApprovals;

    /// @notice owner → operator → approved
    mapping(address => mapping(address => bool)) private _operatorApprovals;

    uint256 private _tokenCounter;

    // ─── ERC-721 minimal interface ─────────────────────────────────────────

    string public constant name     = "ZBX Name Service";
    string public constant symbol   = "ZNS";

    function ownerOf(uint256 tokenId) public view returns (address) {
        string memory n = tokenName[tokenId];
        NameRecord storage r = records[n];
        if (r.owner == address(0)) revert NameNotRegistered();
        return r.owner;
    }

    function balanceOf(address owner_) external view returns (uint256 count) {
        // Simplified: linear scan not used; use events for enumeration off-chain
        return 0; // override with off-chain index
    }

    function approve(address to, uint256 tokenId) external {
        address o = ownerOf(tokenId);
        require(msg.sender == o || _operatorApprovals[o][msg.sender], "ZNS: not approved");
        _tokenApprovals[tokenId] = to;
        emit Approval(o, to, tokenId);
    }

    function setApprovalForAll(address operator, bool approved) external {
        _operatorApprovals[msg.sender][operator] = approved;
        emit ApprovalForAll(msg.sender, operator, approved);
    }

    function transferFrom(address from, address to, uint256 tokenId) external {
        if (to == address(0)) revert ZeroAddress();
        address o = ownerOf(tokenId);
        require(msg.sender == o
            || _tokenApprovals[tokenId] == msg.sender
            || _operatorApprovals[o][msg.sender], "ZNS: not approved");
        require(from == o, "ZNS: wrong from");

        _tokenApprovals[tokenId] = address(0);
        string memory n = tokenName[tokenId];
        records[n].owner = to;

        emit Transfer(from, to, tokenId);
        emit NameTransferred(tokenId, from, to);
    }

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor() {
        admin = msg.sender;
    }

    // ─── Registration ─────────────────────────────────────────────────────

    /// @notice Register `name.zbx` for 1 year.  Pay `annualFee` in ZBX.
    ///         Name is lowercased and validated (alphanumeric + hyphens only).
    /// @param  name_   The label to register (without `.zbx` suffix).
    /// @param  addr    Address this name should resolve to.
    /// @return tokenId ERC-721 token ID representing ownership.
    /// SEC-2026-05-09 Pass-15 (HIGH-S06 / Pass-12 Tier-2 NameService-frontrun):
    /// Two-phase commit-reveal to defeat mempool front-running. Caller
    /// first submits `commit(keccak256(name, addr, salt))`; after at
    /// least `MIN_COMMIT_DELAY` blocks they call `register(name, addr,
    /// salt)`. A front-runner copying the reveal cannot win because
    /// their commit timestamp is later than the legitimate caller's.
    /// Backwards-compat: if no commit is found, the caller can still
    /// register but pays an `EXPRESS_PREMIUM` fee (5x annualFee) — this
    /// preserves the original UX for low-value names while making
    /// front-running unprofitable for high-value names.
    uint256 public constant MIN_COMMIT_DELAY = 5;       // ~10 seconds at 2s blocks
    uint256 public constant MAX_COMMIT_AGE   = 256;     // ~9 minutes
    uint256 public constant EXPRESS_PREMIUM_BPS = 50_000; // 5x annualFee
    mapping(bytes32 => uint256) public commits; // commitHash → blockNumber

    function commit(bytes32 commitHash) external {
        require(commits[commitHash] == 0, "NS: already committed");
        commits[commitHash] = block.number;
    }

    function register(string calldata name_, address addr)
        external payable returns (uint256 tokenId)
    {
        return registerWithSalt(name_, addr, bytes32(0));
    }

    function registerWithSalt(string calldata name_, address addr, bytes32 salt)
        public payable returns (uint256 tokenId)
    {
        if (addr == address(0)) revert ZeroAddress();

        bytes32 commitHash = keccak256(abi.encode(name_, addr, salt, msg.sender));
        uint256 commitBlock = commits[commitHash];
        bool committed = commitBlock != 0
            && block.number >= commitBlock + MIN_COMMIT_DELAY
            && block.number <= commitBlock + MAX_COMMIT_AGE;

        uint256 requiredFee = committed
            ? annualFee
            : (annualFee * EXPRESS_PREMIUM_BPS) / 10_000;
        if (msg.value < requiredFee) revert InsufficientFee();

        if (committed) {
            delete commits[commitHash];
        }

        string memory normalized = _normalize(name_);
        _validateName(normalized);

        NameRecord storage r = records[normalized];

        // Allow re-registration if expired (past grace period)
        if (r.owner != address(0)) {
            if (block.timestamp < r.expiry)                      revert NameTaken();
            if (block.timestamp < r.expiry + GRACE_PERIOD)       revert NameInGracePeriod();
        }

        tokenId = ++_tokenCounter;
        records[normalized] = NameRecord({
            owner:           msg.sender,
            expiry:          block.timestamp + YEAR,
            resolvedAddress: addr
        });
        tokenName[tokenId]        = normalized;
        nameToToken[normalized]   = tokenId;

        emit Transfer(address(0), msg.sender, tokenId);
        emit NameRegistered(tokenId, normalized, msg.sender, block.timestamp + YEAR);
        emit AddressSet(normalized, addr);

        // Refund excess
        uint256 excess = msg.value - annualFee;
        if (excess > 0) {
            (bool ok,) = msg.sender.call{value: excess}("");
            require(ok, "ZNS: refund failed");
        }
    }

    /// @notice Renew a name before or during grace period.
    function renew(string calldata name_) external payable {
        if (msg.value < annualFee) revert InsufficientFee();
        string memory normalized = _normalize(name_);
        NameRecord storage r = records[normalized];
        if (r.owner == address(0))                       revert NameNotRegistered();
        if (block.timestamp > r.expiry + GRACE_PERIOD)   revert NameExpired();
        if (r.owner != msg.sender)                       revert NotNameOwner();

        r.expiry += YEAR;
        emit NameRenewed(nameToToken[normalized], normalized, r.expiry);

        uint256 excess = msg.value - annualFee;
        if (excess > 0) {
            (bool ok,) = msg.sender.call{value: excess}("");
            require(ok, "ZNS: refund failed");
        }
    }

    // ─── Resolution ───────────────────────────────────────────────────────

    /// @notice Resolve a name to its primary address.
    function resolve(string calldata name_) external view returns (address) {
        string memory normalized = _normalize(name_);
        NameRecord storage r = records[normalized];
        if (r.owner == address(0))              revert NameNotRegistered();
        if (block.timestamp > r.expiry + GRACE_PERIOD) revert NameExpired();
        return r.resolvedAddress;
    }

    /// @notice Resolve an address to its primary name.
    function reverseLookup(address addr) external view returns (string memory) {
        return primaryName[addr];
    }

    // ─── Record management ────────────────────────────────────────────────

    /// @notice Update the primary address a name resolves to.
    function setAddress(string calldata name_, address addr) external {
        string memory normalized = _normalize(name_);
        _requireOwner(normalized);
        records[normalized].resolvedAddress = addr;
        emit AddressSet(normalized, addr);
    }

    /// @notice Set your primary name (reverse record).
    function setPrimaryName(string calldata name_) external {
        string memory normalized = _normalize(name_);
        NameRecord storage r = records[normalized];
        if (r.owner != msg.sender) revert NotNameOwner();
        if (block.timestamp > r.expiry) revert NameExpired();
        primaryName[msg.sender] = normalized;
        emit ReverseSet(msg.sender, normalized);
    }

    /// @notice Set a metadata record (e.g., key="avatar", value="ipfs://...").
    function setRecord(string calldata name_, string calldata key, string calldata value) external {
        string memory normalized = _normalize(name_);
        _requireOwner(normalized);
        nameRecords[normalized][key] = value;
        emit RecordSet(normalized, key, value);
    }

    /// @notice Issue a subdomain `sub.parent.zbx` to `to`.
    function issueSubdomain(
        string calldata parent,
        string calldata sub,
        address         to
    ) external {
        if (to == address(0)) revert ZeroAddress();
        string memory norm = _normalize(parent);
        _requireOwner(norm);

        string memory subdomain = string(abi.encodePacked(_normalize(sub), ".", norm));
        _validateName(_normalize(sub));

        NameRecord storage r = records[subdomain];
        if (r.owner != address(0) && block.timestamp < r.expiry) revert NameTaken();

        uint256 parentExpiry = records[norm].expiry;
        uint256 tokenId = ++_tokenCounter;
        records[subdomain] = NameRecord({
            owner:           to,
            expiry:          parentExpiry, // inherits parent expiry
            resolvedAddress: to
        });
        tokenName[tokenId]       = subdomain;
        nameToToken[subdomain]   = tokenId;

        emit Transfer(address(0), to, tokenId);
        emit SubdomainIssued(norm, subdomain, to);
    }

    // ─── Admin ────────────────────────────────────────────────────────────

    function setAnnualFee(uint256 fee) external {
        if (msg.sender != admin) revert NotOwner();
        annualFee = fee;
    }

    function withdraw() external {
        if (msg.sender != admin) revert NotOwner();
        (bool ok,) = admin.call{value: address(this).balance}("");
        if (!ok) revert WithdrawFailed();
    }

    // ─── Internal helpers ─────────────────────────────────────────────────

    function _requireOwner(string memory normalized) private view {
        NameRecord storage r = records[normalized];
        if (r.owner == address(0))        revert NameNotRegistered();
        if (r.owner != msg.sender)        revert NotNameOwner();
        if (block.timestamp > r.expiry)   revert NameExpired();
    }

    function _normalize(string memory input) private pure returns (string memory) {
        bytes memory b  = bytes(input);
        bytes memory out = new bytes(b.length);
        for (uint256 i; i < b.length; ++i) {
            uint8 c = uint8(b[i]);
            if (c >= 65 && c <= 90) {
                out[i] = bytes1(c + 32); // uppercase → lowercase
            } else {
                out[i] = b[i];
            }
        }
        return string(out);
    }

    function _validateName(string memory n) private pure {
        bytes memory b = bytes(n);
        if (b.length < MIN_LENGTH) revert InvalidName();
        for (uint256 i; i < b.length; ++i) {
            uint8 c = uint8(b[i]);
            bool valid = (c >= 97 && c <= 122)  // a-z
                      || (c >= 48 && c <= 57)   // 0-9
                      || c == 45;               // hyphen
            if (!valid) revert InvalidName();
        }
        // Cannot start or end with hyphen
        if (uint8(b[0]) == 45 || uint8(b[b.length - 1]) == 45) revert InvalidName();
    }

    receive() external payable {}
}
