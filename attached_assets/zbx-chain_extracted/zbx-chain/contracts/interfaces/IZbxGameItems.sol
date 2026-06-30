// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title IZbxGameItems — Interface for ZbxGameItems ERC-1155 multi-token game items.
interface IZbxGameItems {
    event TransferSingle(address indexed operator, address indexed from, address indexed to, uint256 id, uint256 value);
    event TransferBatch(address indexed operator, address indexed from, address indexed to, uint256[] ids, uint256[] values);
    event ApprovalForAll(address indexed account, address indexed operator, bool approved);
    event ItemTypeCreated(uint256 indexed itemId, string name, uint256 maxSupply);

    function createItemType(string calldata name, uint256 maxSupply, string calldata uri) external returns (uint256 itemId);
    function mint(address to, uint256 itemId, uint256 amount) external;
    function mintBatch(address to, uint256[] calldata itemIds, uint256[] calldata amounts) external;
    function burn(address from, uint256 itemId, uint256 amount) external;
    function safeTransferFrom(address from, address to, uint256 id, uint256 amount, bytes calldata data) external;
    function safeBatchTransferFrom(address from, address to, uint256[] calldata ids, uint256[] calldata amounts, bytes calldata data) external;
    function setApprovalForAll(address operator, bool approved) external;
    function balanceOf(address account, uint256 id) external view returns (uint256);
    function balanceOfBatch(address[] calldata accounts, uint256[] calldata ids) external view returns (uint256[] memory);
    function isApprovedForAll(address account, address operator) external view returns (bool);
    function totalSupply(uint256 id) external view returns (uint256);
    function maxSupply(uint256 id) external view returns (uint256);
    function uri(uint256 id) external view returns (string memory);
    function supportsInterface(bytes4 interfaceId) external view returns (bool);
}
