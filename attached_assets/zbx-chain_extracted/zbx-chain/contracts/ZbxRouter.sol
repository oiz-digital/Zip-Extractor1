// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

import { ReentrancyGuard } from "./libraries/ReentrancyGuard.sol";

/// @title ZbxRouter — DEX router for Zebvix Chain (ZbxAMM).
/// @notice Aggregates ZbxAMM liquidity pairs. Supports:
///           - Exact-in / exact-out swaps (token↔token)
///           - Multi-hop routes (A→B→C in one tx, up to MAX_PATH_LENGTH hops)
///           - Add / remove liquidity (token pairs)
///           - Native ZBX ↔ ZRC-20 swaps (via WZBX auto-wrap/unwrap)
///           - EIP-2612 permit-based single-tx remove liquidity
///
/// @dev   Compatible with UniswapV2Router02 interface so existing
///        front-ends and aggregators work out of the box.
///
/// @custom:zbx-chain  Chain ID 8989
/// @custom:upgrade    DEX-V2 — ABI mismatch fix, native ZBX swaps,
///                    path-length cap, zero-pair guard, permit support

interface IZRC20Min {
    function transferFrom(address, address, uint256) external returns (bool);
    function transfer(address, uint256) external returns (bool);
    function approve(address, uint256) external returns (bool);
    function balanceOf(address) external view returns (uint256);
    function permit(address,address,uint256,uint256,uint8,bytes32,bytes32) external;
}

interface IWZBX {
    function deposit() external payable;
    function withdraw(uint256 amount) external;
    function transfer(address, uint256) external returns (bool);
    function transferFrom(address, address, uint256) external returns (bool);
    function approve(address, uint256) external returns (bool);
    function balanceOf(address) external view returns (uint256);
}

interface IZbxPair {
    /// @dev DEX-V2 fix: now 4-arg matching ZbxAMM.swap(amount0Out,amount1Out,to,data)
    function swap(uint256 amount0Out, uint256 amount1Out, address to, bytes calldata data) external;
    function getReserves() external view returns (uint112 r0, uint112 r1, uint32 ts);
    function token0() external view returns (address);
    function token1() external view returns (address);
    function mint(address to) external returns (uint256 liquidity);
    function burn(address to) external returns (uint256 amount0, uint256 amount1);
    function balanceOf(address) external view returns (uint256);
    function transferFrom(address, address, uint256) external returns (bool);
    function permit(address,address,uint256,uint256,uint8,bytes32,bytes32) external;
}

interface IZbxFactory {
    function getPair(address, address) external view returns (address);
    function createPair(address, address) external returns (address);
}

contract ZbxRouter is ReentrancyGuard {

    // ─── Constants ────────────────────────────────────────────────────────

    /// @notice Maximum number of hops in a swap path.
    ///         A path of length N requires N-1 pairs. Capping at 4 hops
    ///         (3 pairs) prevents DoS via extremely long path arrays in
    ///         getAmountsOut / getAmountsIn loops.
    uint256 public constant MAX_PATH_LENGTH = 4;

    // ─── Immutable wiring ────────────────────────────────────────────────

    address public immutable factory;
    /// @notice Wrapped ZBX ERC-20 (WZBX) — used for native ZBX swaps.
    address public immutable WZBX;

    constructor(address factory_, address wzbx_) {
        require(factory_ != address(0), "Router: zero factory");
        require(wzbx_    != address(0), "Router: zero WZBX");
        factory = factory_;
        WZBX    = wzbx_;
    }

    receive() external payable {
        // Only accept native ZBX from WZBX contract during unwrap.
        require(msg.sender == WZBX, "Router: ETH from non-WZBX");
    }

    // ─── Modifiers ────────────────────────────────────────────────────────

    modifier ensure(uint256 deadline) {
        require(deadline >= block.timestamp, "Router: expired");
        _;
    }

    modifier validPath(address[] calldata path) {
        require(path.length >= 2 && path.length <= MAX_PATH_LENGTH,
                "Router: bad path length");
        _;
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Token ↔ Token swaps
    // ═══════════════════════════════════════════════════════════════════════

    /// @notice Swap an exact amount of input tokens for as many output tokens
    ///         as possible, along the given path.
    function swapExactTokensForTokens(
        uint256        amountIn,
        uint256        amountOutMin,
        address[] calldata path,
        address        to,
        uint256        deadline
    ) external ensure(deadline) validPath(path) nonReentrant returns (uint256[] memory amounts) {
        require(to != address(0), "Router: zero recipient");
        amounts = getAmountsOut(amountIn, path);
        require(amounts[amounts.length - 1] >= amountOutMin, "Router: insufficient output");
        IZRC20Min(path[0]).transferFrom(
            msg.sender,
            _pairForStrict(path[0], path[1]),
            amounts[0]
        );
        _swap(amounts, path, to);
    }

    /// @notice Swap as few input tokens as possible for an exact amount of output.
    function swapTokensForExactTokens(
        uint256        amountOut,
        uint256        amountInMax,
        address[] calldata path,
        address        to,
        uint256        deadline
    ) external ensure(deadline) validPath(path) nonReentrant returns (uint256[] memory amounts) {
        require(to != address(0), "Router: zero recipient");
        amounts = getAmountsIn(amountOut, path);
        require(amounts[0] <= amountInMax, "Router: excessive input");
        IZRC20Min(path[0]).transferFrom(
            msg.sender,
            _pairForStrict(path[0], path[1]),
            amounts[0]
        );
        _swap(amounts, path, to);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Native ZBX ↔ Token swaps
    // ═══════════════════════════════════════════════════════════════════════

    /// @notice Swap exact native ZBX for tokens.
    ///         ZBX is auto-wrapped to WZBX before routing through the pair.
    function swapExactETHForTokens(
        uint256        amountOutMin,
        address[] calldata path,
        address        to,
        uint256        deadline
    ) external payable ensure(deadline) validPath(path) nonReentrant returns (uint256[] memory amounts) {
        require(path[0] == WZBX, "Router: path[0] must be WZBX");
        require(to != address(0), "Router: zero recipient");
        amounts = getAmountsOut(msg.value, path);
        require(amounts[amounts.length - 1] >= amountOutMin, "Router: insufficient output");
        IWZBX(WZBX).deposit{value: msg.value}();
        require(IWZBX(WZBX).transfer(_pairForStrict(path[0], path[1]), amounts[0]),
                "Router: WZBX transfer failed");
        _swap(amounts, path, to);
    }

    /// @notice Swap tokens for an exact amount of native ZBX.
    function swapTokensForExactETH(
        uint256        amountOut,
        uint256        amountInMax,
        address[] calldata path,
        address        to,
        uint256        deadline
    ) external ensure(deadline) validPath(path) nonReentrant returns (uint256[] memory amounts) {
        require(path[path.length - 1] == WZBX, "Router: path[-1] must be WZBX");
        require(to != address(0), "Router: zero recipient");
        amounts = getAmountsIn(amountOut, path);
        require(amounts[0] <= amountInMax, "Router: excessive input");
        IZRC20Min(path[0]).transferFrom(
            msg.sender,
            _pairForStrict(path[0], path[1]),
            amounts[0]
        );
        _swap(amounts, path, address(this));
        IWZBX(WZBX).withdraw(amounts[amounts.length - 1]);
        _safeTransferETH(to, amounts[amounts.length - 1]);
    }

    /// @notice Swap an exact amount of tokens for as much native ZBX as possible.
    function swapExactTokensForETH(
        uint256        amountIn,
        uint256        amountOutMin,
        address[] calldata path,
        address        to,
        uint256        deadline
    ) external ensure(deadline) validPath(path) nonReentrant returns (uint256[] memory amounts) {
        require(path[path.length - 1] == WZBX, "Router: path[-1] must be WZBX");
        require(to != address(0), "Router: zero recipient");
        amounts = getAmountsOut(amountIn, path);
        require(amounts[amounts.length - 1] >= amountOutMin, "Router: insufficient output");
        IZRC20Min(path[0]).transferFrom(
            msg.sender,
            _pairForStrict(path[0], path[1]),
            amounts[0]
        );
        _swap(amounts, path, address(this));
        IWZBX(WZBX).withdraw(amounts[amounts.length - 1]);
        _safeTransferETH(to, amounts[amounts.length - 1]);
    }

    /// @notice Swap native ZBX for an exact amount of tokens.
    function swapETHForExactTokens(
        uint256        amountOut,
        address[] calldata path,
        address        to,
        uint256        deadline
    ) external payable ensure(deadline) validPath(path) nonReentrant returns (uint256[] memory amounts) {
        require(path[0] == WZBX, "Router: path[0] must be WZBX");
        require(to != address(0), "Router: zero recipient");
        amounts = getAmountsIn(amountOut, path);
        require(amounts[0] <= msg.value, "Router: excessive input");
        IWZBX(WZBX).deposit{value: amounts[0]}();
        require(IWZBX(WZBX).transfer(_pairForStrict(path[0], path[1]), amounts[0]),
                "Router: WZBX transfer failed");
        _swap(amounts, path, to);
        // Refund unused ZBX
        if (msg.value > amounts[0]) {
            _safeTransferETH(msg.sender, msg.value - amounts[0]);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Liquidity — token pairs
    // ═══════════════════════════════════════════════════════════════════════

    function addLiquidity(
        address tokenA, address tokenB,
        uint256 amountADesired, uint256 amountBDesired,
        uint256 amountAMin,     uint256 amountBMin,
        address to,
        uint256 deadline
    ) external ensure(deadline) nonReentrant returns (uint256 amountA, uint256 amountB, uint256 liquidity) {
        require(to != address(0), "Router: zero recipient");
        address pair = IZbxFactory(factory).getPair(tokenA, tokenB);
        if (pair == address(0)) {
            pair = IZbxFactory(factory).createPair(tokenA, tokenB);
        }
        (amountA, amountB) = _computeLiquidityAmounts(
            tokenA, tokenB, amountADesired, amountBDesired, amountAMin, amountBMin
        );
        require(IZRC20Min(tokenA).transferFrom(msg.sender, pair, amountA), "Router: transferA failed");
        require(IZRC20Min(tokenB).transferFrom(msg.sender, pair, amountB), "Router: transferB failed");
        liquidity = IZbxPair(pair).mint(to);
        require(liquidity > 0, "Router: insufficient liquidity minted");
    }

    function removeLiquidity(
        address tokenA, address tokenB,
        uint256 liquidity,
        uint256 amountAMin, uint256 amountBMin,
        address to,
        uint256 deadline
    ) external ensure(deadline) nonReentrant returns (uint256 amountA, uint256 amountB) {
        require(to != address(0), "Router: zero recipient");
        address pair = _pairForStrict(tokenA, tokenB);
        require(IZbxPair(pair).transferFrom(msg.sender, pair, liquidity), "Router: LP transfer failed");
        (uint256 amount0, uint256 amount1) = IZbxPair(pair).burn(to);
        (amountA, amountB) = tokenA < tokenB
            ? (amount0, amount1)
            : (amount1, amount0);
        require(amountA >= amountAMin, "Router: insufficient A out");
        require(amountB >= amountBMin, "Router: insufficient B out");
    }

    /// @notice Remove liquidity using an EIP-2612 permit — single tx,
    ///         no separate approve needed.
    function removeLiquidityWithPermit(
        address tokenA, address tokenB,
        uint256 liquidity,
        uint256 amountAMin, uint256 amountBMin,
        address to,
        uint256 deadline,
        bool    approveMax,
        uint8   v, bytes32 r, bytes32 s
    ) external returns (uint256 amountA, uint256 amountB) {
        address pair = _pairForStrict(tokenA, tokenB);
        uint256 value = approveMax ? type(uint256).max : liquidity;
        IZbxPair(pair).permit(msg.sender, address(this), value, deadline, v, r, s);
        (amountA, amountB) = removeLiquidity(
            tokenA, tokenB, liquidity, amountAMin, amountBMin, to, deadline
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Liquidity — native ZBX + token
    // ═══════════════════════════════════════════════════════════════════════

    function addLiquidityETH(
        address token,
        uint256 amountTokenDesired,
        uint256 amountTokenMin,
        uint256 amountETHMin,
        address to,
        uint256 deadline
    ) external payable ensure(deadline) nonReentrant
      returns (uint256 amountToken, uint256 amountETH, uint256 liquidity) {
        require(to != address(0), "Router: zero recipient");
        address pair = IZbxFactory(factory).getPair(token, WZBX);
        if (pair == address(0)) {
            pair = IZbxFactory(factory).createPair(token, WZBX);
        }
        (amountToken, amountETH) = _computeLiquidityAmounts(
            token, WZBX, amountTokenDesired, msg.value, amountTokenMin, amountETHMin
        );
        require(IZRC20Min(token).transferFrom(msg.sender, pair, amountToken),
                "Router: token transfer failed");
        IWZBX(WZBX).deposit{value: amountETH}();
        require(IWZBX(WZBX).transfer(pair, amountETH), "Router: WZBX transfer failed");
        liquidity = IZbxPair(pair).mint(to);
        require(liquidity > 0, "Router: insufficient liquidity minted");
        // Refund unused ZBX
        if (msg.value > amountETH) {
            _safeTransferETH(msg.sender, msg.value - amountETH);
        }
    }

    function removeLiquidityETH(
        address token,
        uint256 liquidity,
        uint256 amountTokenMin,
        uint256 amountETHMin,
        address to,
        uint256 deadline
    ) external ensure(deadline) nonReentrant returns (uint256 amountToken, uint256 amountETH) {
        require(to != address(0), "Router: zero recipient");
        (amountToken, amountETH) = removeLiquidity(
            token, WZBX, liquidity, amountTokenMin, amountETHMin, address(this), deadline
        );
        require(IZRC20Min(token).transfer(to, amountToken), "Router: token transfer failed");
        IWZBX(WZBX).withdraw(amountETH);
        _safeTransferETH(to, amountETH);
    }

    function removeLiquidityETHWithPermit(
        address token,
        uint256 liquidity,
        uint256 amountTokenMin,
        uint256 amountETHMin,
        address to,
        uint256 deadline,
        bool    approveMax,
        uint8   v, bytes32 r, bytes32 s
    ) external returns (uint256 amountToken, uint256 amountETH) {
        address pair = _pairForStrict(token, WZBX);
        uint256 value = approveMax ? type(uint256).max : liquidity;
        IZbxPair(pair).permit(msg.sender, address(this), value, deadline, v, r, s);
        (amountToken, amountETH) = removeLiquidityETH(
            token, liquidity, amountTokenMin, amountETHMin, to, deadline
        );
    }

    // ═══════════════════════════════════════════════════════════════════════
    // Price calculation (view)
    // ═══════════════════════════════════════════════════════════════════════

    function getAmountsOut(uint256 amountIn, address[] memory path)
        public view returns (uint256[] memory amounts)
    {
        require(path.length >= 2 && path.length <= MAX_PATH_LENGTH, "Router: bad path length");
        amounts = new uint256[](path.length);
        amounts[0] = amountIn;
        for (uint256 i; i < path.length - 1; ++i) {
            (uint112 r0, uint112 r1) = _getReserves(path[i], path[i+1]);
            amounts[i+1] = _getAmountOut(amounts[i], r0, r1);
        }
    }

    function getAmountsIn(uint256 amountOut, address[] memory path)
        public view returns (uint256[] memory amounts)
    {
        require(path.length >= 2 && path.length <= MAX_PATH_LENGTH, "Router: bad path length");
        amounts = new uint256[](path.length);
        amounts[amounts.length - 1] = amountOut;
        for (uint256 i = path.length - 1; i > 0; --i) {
            (uint112 r0, uint112 r1) = _getReserves(path[i-1], path[i]);
            amounts[i-1] = _getAmountIn(amounts[i], r0, r1);
        }
    }

    // ─── AMM formula helpers ──────────────────────────────────────────────

    /// @dev 0.30 % fee: amountOut = (amountIn*9970*reserveOut)/(reserveIn*10000+amountIn*9970)
    ///      Uses 9970/10000 to match ZbxAMM exactly (FEE_NUM=9970 FEE_DEN=10000).
    function _getAmountOut(uint256 amountIn, uint112 reserveIn, uint112 reserveOut)
        internal pure returns (uint256)
    {
        require(amountIn > 0,                       "Router: zero input");
        require(reserveIn > 0 && reserveOut > 0,    "Router: empty reserves");
        uint256 amountInFee = amountIn * 9970;
        uint256 num = amountInFee * reserveOut;
        uint256 den = (uint256(reserveIn) * 10000) + amountInFee;
        return num / den;
    }

    function _getAmountIn(uint256 amountOut, uint112 reserveIn, uint112 reserveOut)
        internal pure returns (uint256)
    {
        require(amountOut > 0,                          "Router: zero output");
        require(reserveIn > 0 && reserveOut > 0,        "Router: empty reserves");
        require(amountOut < uint256(reserveOut),        "Router: insufficient liquidity");
        uint256 num = uint256(reserveIn) * amountOut * 10000;
        uint256 den = (uint256(reserveOut) - amountOut) * 9970;
        return (num / den) + 1;
    }

    // ─── Internal routing ─────────────────────────────────────────────────

    function _swap(uint256[] memory amounts, address[] memory path, address to) internal {
        for (uint256 i; i < path.length - 1; ++i) {
            address input  = path[i];
            address output = path[i + 1];
            (address t0, ) = _sortTokens(input, output);
            (uint256 out0, uint256 out1) = input == t0
                ? (uint256(0), amounts[i + 1])
                : (amounts[i + 1], uint256(0));

            // DEX-V2 fix: intermediate hops route to the next pair address,
            // final hop routes to `to`. Zero pair address means no liquidity
            // exists — hard revert here prevents tokens being sent to address(0).
            address recipient = i + 2 < path.length
                ? _pairForStrict(path[i + 1], path[i + 2])
                : to;

            IZbxPair(_pairForStrict(input, output)).swap(out0, out1, recipient, "");
        }
    }

    function _pairFor(address tA, address tB) internal view returns (address) {
        return IZbxFactory(factory).getPair(tA, tB);
    }

    /// @dev Like _pairFor but reverts if the pair does not exist.
    ///      Used in all swap/liquidity flows to prevent routing through
    ///      address(0) and silently burning user tokens.
    function _pairForStrict(address tA, address tB) internal view returns (address pair) {
        pair = IZbxFactory(factory).getPair(tA, tB);
        require(pair != address(0), "Router: pair does not exist");
    }

    function _getReserves(address tA, address tB) internal view returns (uint112 rA, uint112 rB) {
        address pair = _pairFor(tA, tB);
        if (pair == address(0)) return (0, 0);
        (uint112 r0, uint112 r1,) = IZbxPair(pair).getReserves();
        (address t0,) = _sortTokens(tA, tB);
        (rA, rB) = tA == t0 ? (r0, r1) : (r1, r0);
    }

    function _sortTokens(address tA, address tB) internal pure returns (address, address) {
        require(tA != tB, "Router: identical tokens");
        return tA < tB ? (tA, tB) : (tB, tA);
    }

    function _computeLiquidityAmounts(
        address tA, address tB,
        uint256 aDesired, uint256 bDesired,
        uint256 aMin, uint256 bMin
    ) internal view returns (uint256 amountA, uint256 amountB) {
        (uint112 rA, uint112 rB) = _getReserves(tA, tB);
        if (rA == 0 && rB == 0) {
            return (aDesired, bDesired);
        }
        uint256 bOptimal = aDesired * rB / rA;
        if (bOptimal <= bDesired) {
            require(bOptimal >= bMin, "Router: insufficient B");
            return (aDesired, bOptimal);
        }
        uint256 aOptimal = bDesired * rA / rB;
        require(aOptimal >= aMin, "Router: insufficient A");
        return (aOptimal, bDesired);
    }

    function _safeTransferETH(address to, uint256 value) internal {
        (bool ok, ) = to.call{value: value}("");
        require(ok, "Router: ETH transfer failed");
    }
}
