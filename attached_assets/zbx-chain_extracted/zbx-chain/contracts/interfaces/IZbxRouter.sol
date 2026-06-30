// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxRouter — Interface for ZbxRouter swap router (Uniswap V2–style).
interface IZbxRouter {
    function addLiquidity(address tokenA, address tokenB, uint256 amountADesired, uint256 amountBDesired, uint256 amountAMin, uint256 amountBMin, address to, uint256 deadline) external returns (uint256 amountA, uint256 amountB, uint256 liquidity);
    function addLiquidityNative(address token, uint256 amountTokenDesired, uint256 amountTokenMin, uint256 amountNativeMin, address to, uint256 deadline) external payable returns (uint256 amountToken, uint256 amountNative, uint256 liquidity);
    function removeLiquidity(address tokenA, address tokenB, uint256 liquidity, uint256 amountAMin, uint256 amountBMin, address to, uint256 deadline) external returns (uint256 amountA, uint256 amountB);
    function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
    function swapTokensForExactTokens(uint256 amountOut, uint256 amountInMax, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
    function swapExactNativeForTokens(uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external payable returns (uint256[] memory amounts);
    function swapExactTokensForNative(uint256 amountIn, uint256 amountOutMin, address[] calldata path, address to, uint256 deadline) external returns (uint256[] memory amounts);
    function getAmountsOut(uint256 amountIn, address[] calldata path) external view returns (uint256[] memory amounts);
    function getAmountsIn(uint256 amountOut, address[] calldata path) external view returns (uint256[] memory amounts);
    function factory() external view returns (address);
    function WNATIVE() external view returns (address);
}
