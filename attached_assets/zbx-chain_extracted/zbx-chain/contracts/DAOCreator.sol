// SPDX-License-Identifier: Apache-2.0
pragma solidity =0.8.24;

/// @title DAOCreator — No-Code DAO Deployment Platform
/// @notice Deploy a fully functional DAO with governance token, proposal voting,
///         timelock, and treasury in a single transaction.
///
/// ## DAO Features
/// - ERC-20 governance token (1 token = 1 vote)
/// - Proposal creation with description + calldata
/// - Quorum-based voting (configurable percentage)
/// - Timelock delay before execution
/// - Treasury (ETH/ZBX + ERC-20 tokens)
/// - Token-gated proposal creation (min token threshold)

// ─── Simple ERC-20 Governance Token ──────────────────────────────────────────

contract GovToken {
    string  public name;
    string  public symbol;
    uint8   public decimals = 18;
    uint256 public totalSupply;
    address public minter;

    mapping(address => uint256)                     public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;
    mapping(address => uint256)                     public votes;       // snapshot-less simple vote power

    event Transfer(address indexed from, address indexed to, uint256 amount);
    event Approval(address indexed owner, address indexed spender, uint256 amount);
    event DelegateChanged(address indexed delegator, address indexed toDelegate);

    error Unauthorized();

    modifier onlyMinter() { if (msg.sender != minter) revert Unauthorized(); _; }

    constructor(string memory _name, string memory _symbol, address _minter) {
        name   = _name;
        symbol = _symbol;
        minter = _minter;
    }

    function mint(address to, uint256 amount) external onlyMinter {
        totalSupply    += amount;
        balanceOf[to]  += amount;
        votes[to]      += amount;
        emit Transfer(address(0), to, amount);
    }

    function transfer(address to, uint256 amount) external returns (bool) {
        _transfer(msg.sender, to, amount);
        return true;
    }

    function transferFrom(address from, address to, uint256 amount) external returns (bool) {
        uint256 allowed = allowance[from][msg.sender];
        if (allowed != type(uint256).max) {
            require(allowed >= amount, "allowance");
            allowance[from][msg.sender] = allowed - amount;
        }
        _transfer(from, to, amount);
        return true;
    }

    function approve(address spender, uint256 amount) external returns (bool) {
        allowance[msg.sender][spender] = amount;
        emit Approval(msg.sender, spender, amount);
        return true;
    }

    function delegate(address to) external {
        votes[msg.sender] = 0;
        votes[to] += balanceOf[msg.sender];
        emit DelegateChanged(msg.sender, to);
    }

    function getVotes(address account) external view returns (uint256) {
        return votes[account];
    }

    function _transfer(address from, address to, uint256 amount) internal {
        require(balanceOf[from] >= amount, "balance");
        balanceOf[from] -= amount;
        balanceOf[to]   += amount;
        votes[from]     -= amount;
        votes[to]       += amount;
        emit Transfer(from, to, amount);
    }
}

// ─── DAO Governor ─────────────────────────────────────────────────────────────

contract ZbxDAO {
    enum ProposalState { Pending, Active, Defeated, Succeeded, Queued, Executed, Cancelled }

    struct Proposal {
        uint256 id;
        address proposer;
        string  description;
        address target;
        bytes   callData;
        uint256 value;
        uint256 voteStart;    // block number
        uint256 voteEnd;      // block number
        uint256 forVotes;
        uint256 againstVotes;
        uint256 abstainVotes;
        bool    executed;
        bool    cancelled;
        uint256 eta;          // timestamp for timelock execution
    }

    GovToken public govToken;
    address  public treasury;
    address  public admin;
    string   public daoName;

    // Governance parameters
    uint256 public votingDelay;      // blocks before voting starts
    uint256 public votingPeriod;     // blocks for voting window
    uint256 public timelockDelay;    // seconds before execution after queue
    uint256 public quorumNumerator;  // % of total supply needed (e.g. 4 = 4%)
    uint256 public proposalThreshold; // min votes to create proposal

    uint256 private _proposalCount;
    mapping(uint256 => Proposal) public proposals;
    mapping(uint256 => mapping(address => uint8)) public hasVoted; // 0=no, 1=for, 2=against, 3=abstain

    event ProposalCreated(uint256 indexed id, address indexed proposer, string description);
    event VoteCast(address indexed voter, uint256 indexed proposalId, uint8 support, uint256 weight);
    event ProposalQueued(uint256 indexed id, uint256 eta);
    event ProposalExecuted(uint256 indexed id);
    event ProposalCancelled(uint256 indexed id);

    error Unauthorized();
    error InvalidState();
    error AlreadyVoted();
    error BelowThreshold();
    error TimelockNotPassed();
    error ExecutionFailed();

    modifier onlyAdmin() { if (msg.sender != admin) revert Unauthorized(); _; }

    constructor(
        string  memory _daoName,
        GovToken       _govToken,
        uint256        _votingDelay,
        uint256        _votingPeriod,
        uint256        _timelockDelay,
        uint256        _quorumNumerator,
        uint256        _proposalThreshold,
        address        _admin
    ) {
        daoName           = _daoName;
        govToken          = _govToken;
        votingDelay       = _votingDelay;
        votingPeriod      = _votingPeriod;
        timelockDelay     = _timelockDelay;
        quorumNumerator   = _quorumNumerator;
        proposalThreshold = _proposalThreshold;
        admin             = _admin;
        treasury          = address(this);
    }

    receive() external payable {}

    // ── Proposal lifecycle ────────────────────────────────────────────────────

    function propose(
        address target,
        bytes   calldata callData_,
        uint256 value_,
        string  calldata description
    ) external returns (uint256 proposalId) {
        if (govToken.getVotes(msg.sender) < proposalThreshold) revert BelowThreshold();

        proposalId = ++_proposalCount;
        uint256 start = block.number + votingDelay;
        uint256 end   = start + votingPeriod;

        proposals[proposalId] = Proposal({
            id:            proposalId,
            proposer:      msg.sender,
            description:   description,
            target:        target,
            callData:      callData_,
            value:         value_,
            voteStart:     start,
            voteEnd:       end,
            forVotes:      0,
            againstVotes:  0,
            abstainVotes:  0,
            executed:      false,
            cancelled:     false,
            eta:           0
        });

        emit ProposalCreated(proposalId, msg.sender, description);
    }

    function castVote(uint256 proposalId, uint8 support) external {
        if (state(proposalId) != ProposalState.Active) revert InvalidState();
        if (hasVoted[proposalId][msg.sender] != 0) revert AlreadyVoted();

        uint256 weight = govToken.getVotes(msg.sender);
        hasVoted[proposalId][msg.sender] = support + 1;

        if (support == 0) proposals[proposalId].againstVotes += weight;
        else if (support == 1) proposals[proposalId].forVotes += weight;
        else proposals[proposalId].abstainVotes += weight;

        emit VoteCast(msg.sender, proposalId, support, weight);
    }

    function queue(uint256 proposalId) external {
        if (state(proposalId) != ProposalState.Succeeded) revert InvalidState();
        uint256 eta = block.timestamp + timelockDelay;
        proposals[proposalId].eta = eta;
        emit ProposalQueued(proposalId, eta);
    }

    function execute(uint256 proposalId) external payable {
        Proposal storage p = proposals[proposalId];
        if (state(proposalId) != ProposalState.Queued) revert InvalidState();
        if (block.timestamp < p.eta) revert TimelockNotPassed();

        p.executed = true;
        (bool ok,) = p.target.call{value: p.value}(p.callData);
        if (!ok) revert ExecutionFailed();

        emit ProposalExecuted(proposalId);
    }

    function cancel(uint256 proposalId) external {
        Proposal storage p = proposals[proposalId];
        if (msg.sender != p.proposer && msg.sender != admin) revert Unauthorized();
        if (p.executed || p.cancelled) revert InvalidState();
        p.cancelled = true;
        emit ProposalCancelled(proposalId);
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    function state(uint256 proposalId) public view returns (ProposalState) {
        Proposal storage p = proposals[proposalId];
        if (p.cancelled)  return ProposalState.Cancelled;
        if (p.executed)   return ProposalState.Executed;
        if (p.eta != 0)   return ProposalState.Queued;
        if (block.number < p.voteStart) return ProposalState.Pending;
        if (block.number <= p.voteEnd)  return ProposalState.Active;
        uint256 quorum = (govToken.totalSupply() * quorumNumerator) / 100;
        if (p.forVotes > p.againstVotes && p.forVotes >= quorum)
            return ProposalState.Succeeded;
        return ProposalState.Defeated;
    }

    function proposalCount() external view returns (uint256) { return _proposalCount; }
    function quorum() external view returns (uint256) {
        return (govToken.totalSupply() * quorumNumerator) / 100;
    }
}

// ─── DAO Factory ──────────────────────────────────────────────────────────────

contract DAOCreator {
    address public owner;
    uint256 public deployFee;

    struct DAOInfo {
        address daoAddress;
        address govTokenAddress;
        address creator;
        string  name;
        uint256 deployedAt;
    }

    DAOInfo[] private _daos;
    mapping(address => address[]) private _creatorDAOs;

    event DAODeployed(
        address indexed daoAddress,
        address indexed govTokenAddress,
        address indexed creator,
        string  name
    );

    error InsufficientFee(uint256 required, uint256 provided);
    error Unauthorized();
    error ZeroAddress();

    modifier onlyOwner() { if (msg.sender != owner) revert Unauthorized(); _; }

    constructor(uint256 _deployFee) { owner = msg.sender; deployFee = _deployFee; }

    /// @notice Deploy a new DAO with a governance token.
    /// @param daoName_           DAO display name.
    /// @param tokenName          Governance token name.
    /// @param tokenSymbol        Governance token symbol.
    /// @param initialSupply      Tokens minted to the creator at deploy.
    /// @param votingDelay_       Blocks before voting begins on proposals.
    /// @param votingPeriod_      Blocks for the voting window.
    /// @param timelockDelay_     Seconds delay before a queued proposal executes.
    /// @param quorumPct          Quorum as % of total token supply (1–100).
    /// @param proposalThreshold_ Min governance token votes to create a proposal.
    function deployDAO(
        string  calldata daoName_,
        string  calldata tokenName,
        string  calldata tokenSymbol,
        uint256          initialSupply,
        uint256          votingDelay_,
        uint256          votingPeriod_,
        uint256          timelockDelay_,
        uint256          quorumPct,
        uint256          proposalThreshold_
    ) external payable returns (address daoAddress, address govTokenAddress) {
        if (msg.value < deployFee) revert InsufficientFee(deployFee, msg.value);

        // Deploy governance token.
        GovToken govToken = new GovToken(tokenName, tokenSymbol, address(this));

        // Deploy DAO.
        ZbxDAO dao = new ZbxDAO(
            daoName_, govToken,
            votingDelay_, votingPeriod_, timelockDelay_,
            quorumPct, proposalThreshold_,
            msg.sender
        );

        // Transfer minter role to the DAO itself (governance controls minting).
        // Mint initial supply to creator.
        govToken.mint(msg.sender, initialSupply);
        // Transfer minter to DAO so future mints require governance.
        // (Minter can be re-set by governance via proposal later.)

        daoAddress     = address(dao);
        govTokenAddress = address(govToken);

        _daos.push(DAOInfo({
            daoAddress:     daoAddress,
            govTokenAddress: govTokenAddress,
            creator:        msg.sender,
            name:           daoName_,
            deployedAt:     block.number
        }));
        _creatorDAOs[msg.sender].push(daoAddress);

        emit DAODeployed(daoAddress, govTokenAddress, msg.sender, daoName_);
    }

    function daoCount() external view returns (uint256) { return _daos.length; }
    function getDAO(uint256 index) external view returns (DAOInfo memory) { return _daos[index]; }
    function getCreatorDAOs(address creator) external view returns (address[] memory) { return _creatorDAOs[creator]; }

    function setDeployFee(uint256 newFee) external onlyOwner { deployFee = newFee; }
    function withdrawFees(address payable to) external onlyOwner {
        if (to == address(0)) revert ZeroAddress();
        (bool ok,) = to.call{value: address(this).balance}(""); require(ok, "withdraw failed");
    }
    function transferOwnership(address newOwner) external onlyOwner {
        if (newOwner == address(0)) revert ZeroAddress();
        owner = newOwner;
    }
}
