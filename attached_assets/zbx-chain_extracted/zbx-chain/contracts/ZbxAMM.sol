// SPDX-License-Identifier: MIT
pragma solidity =0.8.24;

/// @title ZbxAMM — Uniswap V2-style constant-product AMM pair (single-pool)
/// @author Zebvix Technologies Pvt Ltd
/// @notice Holds reserves of two ERC-20 tokens (e.g. ZBX/ZUSD or ZBX/WBNB)
///         and enforces the invariant `reserve0 * reserve1 = k`.
///         Charges a 0.30 % swap fee; LP shares are minted as ERC-20 LP
///         tokens that represent a proportional claim on the reserves.
///
/// @dev    Mirrors the full Uniswap V2 Pair public surface including:
///           - 4-arg swap with optional flash-loan callback (IZbxCallee)
///           - skim() / sync() for reserve reconciliation
///           - EIP-2612 permit() for gasless LP approvals
///           - TWAP cumulative-price oracle
///         All unchecked blocks are tagged to one of the proven-safe categories
///         (a)-(e) documented in the policy block below.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:upgrade    DEX-V2 — ABI mismatch fix, flash loans, skim/sync, EIP-2612
import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

contract ZbxAMM is ReentrancyGuard {

    // ─── S25-Y4 unchecked policy ─────────────────────────────────────────
    // All `unchecked { ... }` blocks fall into ONE of these proven-safe categories:
    //   (a) post-require subtraction — preceding `require(x >= y)` proves
    //       `x - y` cannot underflow (token balance debits, allowance debits).
    //   (b) conservation pair — incrementing one slot by exactly the value
    //       just decremented (or vice-versa) from another, totalSupply
    //       invariant pre-checked (mint/burn/transfer accounting pair).
    //   (c) bounded for-loop counter — `for (i; i < len; ) { ...; unchecked
    //       { i++; } }` where `len` is the bound; standard gas-saving pattern.
    //   (d) modular wrap intentional — uint32 timestamp wrap arithmetic
    //       (Uniswap V2 style); the wrap IS the spec.
    //   (e) UQ112x112 fixed-point shift — pre-bounded by uint112 reserve
    //       invariants (Uniswap V2 oracle accumulator).
    // Reviewers MUST classify any future `unchecked` block against one of
    // (a)-(e) before merging; new categories require AUDIT entry.
    // ─────────────────────────────────────────────────────────────────────

    // ─── Flash-loan callback interface ───────────────────────────────────

    /// @notice Callback interface that flash-loan receivers must implement.
    ///         Mirrored from IUniswapV2Callee for drop-in compatibility.
    interface IZbxCallee {
        function zbxV2Call(
            address sender,
            uint256 amount0,
            uint256 amount1,
            bytes calldata data
        ) external;
    }

    // ─── Immutable wiring ────────────────────────────────────────────────

    /// @notice token0 < token1 (sorted) so the pair canonical-form is unique.
    address public immutable token0;
    address public immutable token1;

    string  public constant name     = "Zebvix LP";
    string  public constant symbol   = "ZBX-LP";
    uint8   public constant decimals = 18;

    /// @notice Minimum liquidity locked forever in the contract on first
    ///         mint — prevents the pool being drained to zero (which would
    ///         brick price discovery for future LPs). Same value as Uni V2.
    uint256 public constant MINIMUM_LIQUIDITY = 10**3;

    /// @notice 0.30 % swap fee, expressed as `(1 - 30/10000)` numerator.
    ///         Fee accrues to LPs by way of `k` growing on every swap.
    uint256 private constant FEE_NUM = 9970;
    uint256 private constant FEE_DEN = 10000;

    // ─── ERC-20 LP token storage ─────────────────────────────────────────

    uint256 public totalSupply;
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    event Transfer(address indexed from, address indexed to, uint256 value);
    event Approval(address indexed owner, address indexed spender, uint256 value);

    // ─── EIP-2612 permit ─────────────────────────────────────────────────

    /// @notice EIP-712 domain separator for permit() signatures.
    bytes32 public immutable DOMAIN_SEPARATOR;

    /// @notice Per-account nonce for permit() replay protection.
    mapping(address => uint256) public nonces;

    bytes32 private constant PERMIT_TYPEHASH = keccak256(
        "Permit(address owner,address spender,uint256 value,uint256 nonce,uint256 deadline)"
    );

    // ─── Reserves + price oracle ──────────────────────────────────────────

    uint112 private _reserve0;
    uint112 private _reserve1;
    uint32  private _blockTimestampLast;

    /// @notice TWAP cumulative prices (Uni V2 formulas), scaled by 2^112.
    uint256 public price0CumulativeLast;
    uint256 public price1CumulativeLast;

    /// @notice Last `reserve0 * reserve1` after the most recent mint/burn.
    ///         Kept for protocol-fee computation compatibility (fee is off by
    ///         default; 100 % of swap fee goes to LPs).
    uint256 public kLast;

    // ─── Reentrancy ───────────────────────────────────────────────────────

    // SEC-2026-05-09: migrated to libraries/ReentrancyGuard.sol.

    // ─── Errors ───────────────────────────────────────────────────────────

    error IdenticalAddresses();
    error InsufficientLiquidityMinted();
    error InsufficientLiquidityBurned();
    error InsufficientOutputAmount();
    error InsufficientInputAmount();
    error InsufficientLiquidity();
    error InvalidTo();
    error K();
    error Overflow();

    // ─── Events ───────────────────────────────────────────────────────────

    event Mint(address indexed sender, uint256 amount0, uint256 amount1);
    event Burn(address indexed sender, uint256 amount0, uint256 amount1, address indexed to);
    event Swap(
        address indexed sender,
        uint256 amount0In,
        uint256 amount1In,
        uint256 amount0Out,
        uint256 amount1Out,
        address indexed to
    );
    event Sync(uint112 reserve0, uint112 reserve1);

    // ─── Constructor ──────────────────────────────────────────────────────

    constructor(address _tokenA, address _tokenB) {
        if (_tokenA == _tokenB) revert IdenticalAddresses();
        require(_tokenA != address(0) && _tokenB != address(0), "ZERO_ADDRESS");

        // Sort so the pair has a canonical orientation.
        (token0, token1) = _tokenA < _tokenB
            ? (_tokenA, _tokenB)
            : (_tokenB, _tokenA);

        // EIP-712 domain separator — bound to this contract's address and
        // the chain ID at deploy time so signatures cannot be replayed on
        // other chains or against other pair contracts.
        DOMAIN_SEPARATOR = keccak256(abi.encode(
            keccak256(
                "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)"
            ),
            keccak256(bytes(name)),
            keccak256(bytes("1")),
            block.chainid,
            address(this)
        ));
    }

    // ─── ERC-20 LP token ─────────────────────────────────────────────────

    function transfer(address to, uint256 value) external returns (bool) {
        _transfer(msg.sender, to, value);
        return true;
    }

    function transferFrom(address from, address to, uint256 value)
        external
        returns (bool)
    {
        uint256 a = allowance[from][msg.sender];
        if (a != type(uint256).max) {
            require(a >= value, "ZBXLP/INSUF_ALLOWANCE");
            unchecked { allowance[from][msg.sender] = a - value; } // (a)
        }
        _transfer(from, to, value);
        return true;
    }

    function approve(address spender, uint256 value) external returns (bool) {
        allowance[msg.sender][spender] = value;
        emit Approval(msg.sender, spender, value);
        return true;
    }

    /// @notice EIP-2612 permit — set allowance via a signed message.
    ///         Eliminates a separate `approve` tx for LP token operations
    ///         (e.g. single-tx add-and-remove via router).
    function permit(
        address owner,
        address spender,
        uint256 value,
        uint256 deadline,
        uint8   v,
        bytes32 r,
        bytes32 s
    ) external {
        require(deadline >= block.timestamp, "ZBX-LP: permit expired");
        bytes32 digest = keccak256(abi.encodePacked(
            "\x19\x01",
            DOMAIN_SEPARATOR,
            keccak256(abi.encode(
                PERMIT_TYPEHASH,
                owner,
                spender,
                value,
                nonces[owner]++,
                deadline
            ))
        ));
        address recovered = ecrecover(digest, v, r, s);
        require(recovered != address(0) && recovered == owner, "ZBX-LP: invalid sig");
        allowance[owner][spender] = value;
        emit Approval(owner, spender, value);
    }

    function _transfer(address from, address to, uint256 value) private {
        require(to != address(0) && from != address(0), "ZBXLP/ZERO");
        uint256 bal = balanceOf[from];
        require(bal >= value, "ZBXLP/INSUF_BAL");
        unchecked {
            balanceOf[from] = bal - value; // (a)
            balanceOf[to]  += value;       // (b)
        }
        emit Transfer(from, to, value);
    }

    function _mintLp(address to, uint256 value) private {
        totalSupply += value;
        unchecked { balanceOf[to] += value; } // (b)
        emit Transfer(address(0), to, value);
    }

    function _burnLp(address from, uint256 value) private {
        uint256 bal = balanceOf[from];
        require(bal >= value, "ZBXLP/INSUF_BAL");
        unchecked {
            balanceOf[from] = bal - value; // (a)
            totalSupply    -= value;       // (b)
        }
        emit Transfer(from, address(0), value);
    }

    // ─── Reserve view + sync ──────────────────────────────────────────────

    function getReserves()
        public
        view
        returns (
            uint112 reserve0,
            uint112 reserve1,
            uint32  blockTimestampLast
        )
    {
        reserve0           = _reserve0;
        reserve1           = _reserve1;
        blockTimestampLast = _blockTimestampLast;
    }

    /// @dev Update reserves + cumulative-price oracle. Borrowed verbatim
    ///      from UniswapV2Pair._update.
    function _update(
        uint256 balance0,
        uint256 balance1,
        uint112 reserve0_,
        uint112 reserve1_
    ) private {
        if (balance0 > type(uint112).max || balance1 > type(uint112).max)
            revert Overflow();

        uint32 blockTimestamp = uint32(block.timestamp % 2**32);
        uint32 timeElapsed;
        unchecked {
            timeElapsed = blockTimestamp - _blockTimestampLast; // (d) uint32 wrap
        }

        if (timeElapsed > 0 && reserve0_ != 0 && reserve1_ != 0) {
            unchecked {
                // (e) UQ112x112: bounded by uint112 reserve invariants
                price0CumulativeLast +=
                    (uint256(reserve1_) << 112) / reserve0_ * timeElapsed;
                price1CumulativeLast +=
                    (uint256(reserve0_) << 112) / reserve1_ * timeElapsed;
            }
        }

        _reserve0 = uint112(balance0);
        _reserve1 = uint112(balance1);
        _blockTimestampLast = blockTimestamp;

        emit Sync(_reserve0, _reserve1);
    }

    // ─── mint / burn ──────────────────────────────────────────────────────

    /// @notice Caller must transfer token0 + token1 to this contract first,
    ///         then call `mint(to)`. Returns LP tokens minted.
    function mint(address to) external nonReentrant returns (uint256 liquidity) {
        (uint112 r0, uint112 r1, ) = getReserves();
        uint256 bal0 = _bal(token0);
        uint256 bal1 = _bal(token1);
        uint256 amount0 = bal0 - r0;
        uint256 amount1 = bal1 - r1;

        uint256 _totalSupply = totalSupply;
        if (_totalSupply == 0) {
            liquidity = _sqrt(amount0 * amount1) - MINIMUM_LIQUIDITY;
            // Permanently lock MINIMUM_LIQUIDITY by minting to address(0).
            _mintLp(address(0), MINIMUM_LIQUIDITY);
        } else {
            uint256 a = (amount0 * _totalSupply) / r0;
            uint256 b = (amount1 * _totalSupply) / r1;
            liquidity = a < b ? a : b;
        }
        if (liquidity == 0) revert InsufficientLiquidityMinted();

        _mintLp(to, liquidity);
        _update(bal0, bal1, r0, r1);
        kLast = uint256(_reserve0) * uint256(_reserve1);

        emit Mint(msg.sender, amount0, amount1);
    }

    /// @notice Caller must transfer LP tokens to this contract first,
    ///         then call `burn(to)` to redeem underlying.
    function burn(address to)
        external
        nonReentrant
        returns (uint256 amount0, uint256 amount1)
    {
        (uint112 r0, uint112 r1, ) = getReserves();
        uint256 bal0 = _bal(token0);
        uint256 bal1 = _bal(token1);
        uint256 liquidity = balanceOf[address(this)];

        uint256 _totalSupply = totalSupply;
        amount0 = (liquidity * bal0) / _totalSupply;
        amount1 = (liquidity * bal1) / _totalSupply;
        if (amount0 == 0 || amount1 == 0) revert InsufficientLiquidityBurned();

        _burnLp(address(this), liquidity);
        _safeTransfer(token0, to, amount0);
        _safeTransfer(token1, to, amount1);

        bal0 = _bal(token0);
        bal1 = _bal(token1);
        _update(bal0, bal1, r0, r1);
        kLast = uint256(_reserve0) * uint256(_reserve1);

        emit Burn(msg.sender, amount0, amount1, to);
    }

    // ─── swap — Uni V2 full surface (4-arg, flash callback) ──────────────

    /// @notice Caller must transfer the input token to this contract first,
    ///         then call swap with the desired output amounts.
    ///
    /// @param amount0Out  Amount of token0 to send to `to` (0 if unused).
    /// @param amount1Out  Amount of token1 to send to `to` (0 if unused).
    /// @param to          Recipient of the output tokens. Cannot be either
    ///                    reserve token (prevents reserve drain) or address(0).
    /// @param data        Arbitrary callback data. If non-empty, `to` must
    ///                    implement IZbxCallee.zbxV2Call and repay the flash
    ///                    loan within the same transaction before the k-check.
    ///
    /// @dev DEX-V2 upgrade: added `data` parameter for flash-loan support.
    ///      Flash loans are repaid atomically — the k-invariant is enforced
    ///      AFTER the callback returns. Reentrancy is blocked by nonReentrant.
    function swap(
        uint256 amount0Out,
        uint256 amount1Out,
        address to,
        bytes calldata data
    ) external nonReentrant {
        if (amount0Out == 0 && amount1Out == 0) revert InsufficientOutputAmount();
        (uint112 r0, uint112 r1, ) = getReserves();
        if (amount0Out >= r0 || amount1Out >= r1) revert InsufficientLiquidity();

        // Reject outputs to zero address or the pair's own tokens. The zero
        // check prevents silent loss; the token checks prevent the reserves
        // themselves being used as a flash-loan delivery target (which would
        // corrupt the balance accounting used for the k-invariant check).
        if (to == address(0) || to == token0 || to == token1) revert InvalidTo();

        if (amount0Out > 0) _safeTransfer(token0, to, amount0Out);
        if (amount1Out > 0) _safeTransfer(token1, to, amount1Out);

        // Flash-loan callback: `to` must repay before this function returns.
        // Reentrancy guard prevents re-entry via the callback.
        if (data.length > 0)
            IZbxCallee(to).zbxV2Call(msg.sender, amount0Out, amount1Out, data);

        uint256 bal0 = _bal(token0);
        uint256 bal1 = _bal(token1);
        uint256 amount0In = bal0 > r0 - amount0Out ? bal0 - (r0 - amount0Out) : 0;
        uint256 amount1In = bal1 > r1 - amount1Out ? bal1 - (r1 - amount1Out) : 0;
        if (amount0In == 0 && amount1In == 0) revert InsufficientInputAmount();
        // SEC-2026-05-09 Pass-15 (CRIT-05): minimum-swap guard.
        // Pre-fix dust-sized swaps where `amountIn * (FEE_DEN - FEE_NUM)`
        // (= 3) is dwarfed by `bal * FEE_DEN` could pass the K-invariant
        // check despite paying effectively 0 fee, because the
        // multiplicative term lost precision in the `bal*FEE_DEN`
        // dominant addend. Many small swaps drain LP cumulatively while
        // each individual swap looks legitimate. Floor input at
        // 1000 wei guarantees the fee term is materially above noise
        // and forces drainers to expose themselves with larger swaps
        // that fail the K-check. (Standard AMMs use minimum-out checks
        // off-chain via routers; this is a defence-in-depth on-chain
        // floor independent of router behaviour.)
        uint256 MIN_SWAP_IN = 1_000;
        require(amount0In + amount1In >= MIN_SWAP_IN, "AMM: dust-swap rejected");

        // Apply 0.30 % fee on input and verify k invariant holds net of fee.
        uint256 bal0Adj = (bal0 * FEE_DEN) - (amount0In * (FEE_DEN - FEE_NUM));
        uint256 bal1Adj = (bal1 * FEE_DEN) - (amount1In * (FEE_DEN - FEE_NUM));
        // SEC-2026-05-09 Pass-19 (Tier-2 #1): STRICT K-growth — was `<`
        // (allowed K-equality, i.e. zero-fee swap if precision loss
        // happened to land exactly). Now `<=` requires bal0Adj*bal1Adj
        // > r0*r1*FEE_DEN^2 strictly, guaranteeing every swap pays a
        // non-zero fee to LPs. Combined with Pass-15's MIN_SWAP_IN
        // (1000 wei) floor, dust-swap fee-evasion is closed end-to-end.
        if (bal0Adj * bal1Adj <= uint256(r0) * uint256(r1) * (FEE_DEN ** 2))
            revert K();

        _update(bal0, bal1, r0, r1);
        emit Swap(msg.sender, amount0In, amount1In, amount0Out, amount1Out, to);
    }

    // ─── skim / sync — reserve reconciliation ────────────────────────────

    /// @notice Transfer any surplus token balance (above recorded reserves)
    ///         to `to`. Useful when a direct ERC-20 transfer inflates the pool
    ///         balance beyond uint112 max, or as a reserve-reset primitive.
    /// @dev    DEX-V2 upgrade: was missing (`S21-FOLLOWUP-AMM-INTERFACE-RECONCILIATION`).
    function skim(address to) external nonReentrant {
        require(to != address(0), "AMM: skim to zero");
        uint256 excess0 = _bal(token0) - uint256(_reserve0);
        uint256 excess1 = _bal(token1) - uint256(_reserve1);
        if (excess0 > 0) _safeTransfer(token0, to, excess0);
        if (excess1 > 0) _safeTransfer(token1, to, excess1);
    }

    /// @notice Force the stored reserves to match the current token balances.
    ///         Use after a direct transfer that changed balances without a swap.
    /// @dev    DEX-V2 upgrade: was missing (`S21-FOLLOWUP-AMM-INTERFACE-RECONCILIATION`).
    function sync() external nonReentrant {
        _update(_bal(token0), _bal(token1), _reserve0, _reserve1);
    }

    // ─── Convenience: amountOut quoted at current reserves ────────────────

    function getAmountOut(uint256 amountIn, address tokenIn)
        external
        view
        returns (uint256 amountOut)
    {
        (uint112 r0, uint112 r1, ) = getReserves();
        (uint256 reserveIn, uint256 reserveOut) =
            tokenIn == token0 ? (uint256(r0), uint256(r1)) : (uint256(r1), uint256(r0));
        require(reserveIn > 0 && reserveOut > 0, "INSUF_LIQ");

        uint256 amountInWithFee = amountIn * FEE_NUM;
        amountOut = (amountInWithFee * reserveOut) /
                    (reserveIn * FEE_DEN + amountInWithFee);
    }

    // ─── Internal — ERC-20 helpers + sqrt ────────────────────────────────

    function _bal(address token) private view returns (uint256) {
        (bool ok, bytes memory data) = token.staticcall(
            abi.encodeWithSignature("balanceOf(address)", address(this))
        );
        require(ok && data.length >= 32, "BAL_FAIL");
        return abi.decode(data, (uint256));
    }

    function _safeTransfer(address token, address to, uint256 value) private {
        (bool ok, bytes memory data) = token.call(
            abi.encodeWithSignature("transfer(address,uint256)", to, value)
        );
        require(ok && (data.length == 0 || abi.decode(data, (bool))), "XFER_FAIL");
    }

    /// @dev Babylonian sqrt — same routine Uniswap V2 uses for initial mint.
    function _sqrt(uint256 y) private pure returns (uint256 z) {
        if (y > 3) {
            z = y;
            uint256 x = y / 2 + 1;
            while (x < z) {
                z = x;
                x = (y / x + x) / 2;
            }
        } else if (y != 0) {
            z = 1;
        }
    }

    // ─── EIP-165 ──────────────────────────────────────────────────────────

    /// @notice Supports EIP-165 (interface detection) and IZbxAMM.
    ///         DEX-V2 upgrade: IZbxAMM claim is now valid because all three
    ///         previously-divergent functions (skim, sync, 4-arg swap) have
    ///         been added. Closes S21-FOLLOWUP-AMM-INTERFACE-RECONCILIATION.
    function supportsInterface(bytes4 interfaceId) external pure returns (bool) {
        return interfaceId == 0x01ffc9a7   // EIP-165
            || interfaceId == 0xd9e5f6b5;  // IZbxAMM (skim+sync+4-arg swap)
    }
}
