# Opticrum Calculator

Off-chain transaction assembly crate for the Opticrum decentralized liquidity marketplace. Built on [ckb-cinnabar-calculator](https://github.com/ashuralyk/ckb-cinnabar).

## Overview

This crate provides high-level `Instruction` builders that compose ckb-cinnabar's basic `Operation`s into ready-to-use transaction recipes. Application developers call these without understanding the underlying cell structure.

It also provides on-chain readers that query the CKB indexer and parse raw cell data into typed structs (`OrderInfo`, `MatchInfo`).

Protocol types (`OrderArgs`, `OrderData`, `MatchArgs`, `MatchData`, `OutPoint`, and all length constants) are defined canonically in `opticrum-protocol` and re-exported here.

## Modules

| Module | Purpose |
|--------|---------|
| `calculator` | Transaction instruction builders (create, cancel, match, extract, destroy) |
| `types` | Off-chain types: `Xudt`, `AnnualYield`, `OrderInfo`, `MatchInfo` |
| `reader` | On-chain cell scanning and parsing (`scan_orders`, `scan_matches`) |
| `operation` | Opticrum-specific `Operation` implementations (`AddOpticrumContractCelldep`) |
| `config` | Contract name, type_id resolution, constants |

## Calculator Instructions

All instructions return `Instruction<T>` where `T: RPC`. They compose `Operation`s that are executed by ckb-cinnabar's transaction builder.

### `create_order(buyer, order_args, order_data, annual_yield, xudt_type_script?)`

Creates an Order Cell with Opticrum lock. The buyer's personal lock signs the transaction.

- Computes `rent_capacity` from `AnnualYield` × Order Data for CKB-denominated orders
- For xUDT orders, stores the computed `xudt_amount` in Order data with 0 capacity (capacity comes from the xUDT cell)
- The Opticrum lock does NOT execute on creation — verification only runs on consumption

**Transaction structure:**
```
CellDeps:  [Opticrum contract]
Inputs:    [Buyer's cell]
Outputs:   [Order Cell (± xUDT type script)]
```

### `cancel_order(buyer, order_info)`

Burns the Order Cell, returning capacity (and optional xUDT) to the buyer.

- Order Cell appears only in inputs (Burn pattern → OrderCancel verifier)
- Buyer's cell proves authorization (lock hash must match `buyer_lock_hash`)

**Transaction structure:**
```
CellDeps:  [Opticrum contract]
Inputs:    [Order Cell, Buyer's cell]
Outputs:   [(xUDT return cell if applicable)]
```

### `match_order(seller, order_info, match_args)`

Consumes an Order Cell and produces a Match Cell. The pre-created Fiber channel cell is referenced as a CellDep (not consumed).

- Computes `MatchData` with `rent_per_block = total_rent / escrow_blocks`
- Match args embed the channel's `OutPoint` (36 bytes: tx_hash + index)
- Match capacity MUST equal Order capacity (rent transferred intact)

**Transaction structure:**
```
CellDeps:  [Opticrum contract, Channel Cell]
Inputs:    [Order Cell, Seller's cell]
Outputs:   [Match Cell, Seller change]
```

### `extract_rent(seller, match_info, tip_block)`

Seller withdraws linearly-vested rent from a Match Cell.

- Linear formula: `extractable = rent_per_block × (tip_block - last_extraction_block)`
- If `last_extraction_block == 0` (never extracted), the match creation block is used as the starting point
- On first extraction, adds a `HeaderDep` at match creation block to prove the match's age
- If accumulated rent exceeds remaining capacity, delegates to `destroy_match` internally

**Transaction structure:**
```
CellDeps:    [Opticrum contract]
HeaderDeps:  [tip_header] (+ creation_header on first extraction)
Inputs:      [Match Cell, Seller's cell]
Outputs:     [Updated Match Cell, Seller cell + rent]
```

### `destroy_match(claimant, match_info, tip_block)`

Destroys an exhausted Match Cell, returning remaining funds to the claimant.

- Match Cell appears only in inputs (Burn pattern → MatchDestroy verifier)
- Adds a `HeaderDep` at match creation block if the match was never extracted
- No authorization beyond the exhaustion check required on-chain

**Transaction structure:**
```
CellDeps:    [Opticrum contract]
HeaderDeps:  [tip_header] (+ creation_header if never extracted)
Inputs:      [Match Cell, Claimant's cell]
Outputs:     [Claimant cell + remaining funds]
```

## On-Chain Readers

### `scan_orders(rpc) → Vec<OrderInfo>`

Scans all live Order cells on-chain using the CKB indexer. Queries cells with the Opticrum lock whose args length is exactly `ORDER_ARGS_LEN` (65 bytes). Parses each into `OrderInfo`, computing real rent capacity as `total_capacity - occupied_capacity`.

### `scan_matches(rpc) → Vec<MatchInfo>`

Scans all live Match cells on-chain. Queries cells with the Opticrum lock whose args length is exactly `MATCH_ARGS_LEN` (133 bytes). Parses each into `MatchInfo`, including the block number at which the match was created (`match_current_block`).

Both use prefix search mode for efficient indexer queries. Results are paginated in batches of 50.

## Types

### `Xudt`

Specifies the xUDT token attached to an Order or Match Cell.

| Field | Type | Description |
|-------|------|-------------|
| `amount` | `u128` | Token amount (stored in cell data, first 16 bytes) |
| `type_script` | `Script` | xUDT type script set on the cell |

### `AnnualYield`

Represents the annual yield in percentage. Wraps a single `u8`.

```rust
pub struct AnnualYield(pub u8);
```

Methods:
- `to_ckb(order: &OrderData) -> u64` — Compute rent capacity in shannons for CKB-denominated orders
- `to_xudt(order: &OrderData) -> u128` — Compute rent amount for xUDT-denominated orders

Uses `ABOUT_ONE_DAY_BLOCKS = 10_000` for day-block conversion.

### `OrderInfo`

Parsed representation of a live Order cell on-chain.

| Field | Type | Description |
|-------|------|-------------|
| `order_args` | `OrderArgs` | Parsed lock args (fiber_pubkey + buyer_lock_hash) |
| `order_data` | `OrderData` | Parsed cell data (xudt_amount + channel_capacity + escrow_blocks) |
| `xudt` | `Option<Xudt>` | Token info if this is an xUDT order |
| `ckb_capacity` | `u64` | Real rent capacity (total - occupied) |
| `order_outpoint` | `OutPoint` | On-chain location (for consuming the cell) |

### `MatchInfo`

Parsed representation of a live Match cell on-chain.

| Field | Type | Description |
|-------|------|-------------|
| `match_args` | `MatchArgs` | Parsed lock args (order_args + channel_outpoint + seller_lock_hash) |
| `match_data` | `MatchData` | Parsed cell data (xudt_amount + rent_per_block + escrow_blocks + last_extraction_block) |
| `xudt` | `Option<Xudt>` | Token info if this is an xUDT match |
| `ckb_capacity` | `u64` | Real rent capacity (total - occupied) |
| `match_outpoint` | `OutPoint` | On-chain location (for consuming the cell) |
| `match_current_block` | `u64` | Block number when the match was created |

Methods:
- `extraction_amount(tip_block) -> u64` — Compute extractable rent using the linear formula
- `is_exhausted(tip_block) -> bool` — Check if accumulated rent >= remaining capacity

## Operations

### `opticrum_lock(args: Vec<u8>) -> ScriptEx`

Build a `ScriptEx::Reference` for the Opticrum lock script. The caller provides correctly encoded args (use `OrderArgs::to_bytes()` or `MatchArgs::to_bytes()`).

### `AddOpticrumContractCelldep`

An `Operation` that adds the Opticrum contract code cell as a CellDep to the transaction skeleton. Resolves the contract's type ID via the RPC network type.

## Config Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `OPTICRUM_CONTRACT_NAME` | `"opticrum"` | Name used in ScriptEx::Reference lookups |
| `ABOUT_ONE_DAY_BLOCKS` | `10_000` | Approximate CKB blocks per day (used in AnnualYield) |
| `CKB_DECIMAL` | `100_000_000` | Shannons per CKB |

## Dependencies

- `ckb-cinnabar-calculator` — Transaction skeleton, operations, RPC abstraction, indexer queries
- `opticrum-protocol` — Canonical byte-level data layouts shared with the on-chain contract

## Usage Example

```rust
use opticrum_calculator::{
    create_order, cancel_order, match_order, extract_rent,
    scan_orders, scan_matches,
    types::{AnnualYield, OrderArgs, OrderData, MatchArgs},
};
use opticrum_protocol::OutPoint;
use ckb_cinnabar_calculator::{address::Address, rpc::RPC};

async fn example<T: RPC>(rpc: &T, buyer: Address, seller: Address) {
    // Create an order offering rent for a 1000-CKB channel over ~30 days
    let order_args = OrderArgs::new(fiber_pubkey, buyer_lock_hash);
    let order_data = OrderData::new(0, 1000 * 100_000_000, 300_000);
    let annual_yield = AnnualYield(10); // 10% APR

    let instruction = create_order(
        buyer.clone(),
        &order_args,
        &order_data,
        annual_yield,
        None, // CKB-denominated
    );
    // Execute instruction...

    // Scan for orders
    let orders = scan_orders(rpc).await.unwrap();
    let order_info = &orders[0];

    // Match with a pre-created channel
    let channel_outpoint = OutPoint::new(channel_tx_hash, 0);
    let match_args = MatchArgs::new(order_args.clone(), channel_outpoint, seller_lock_hash);

    let instruction = match_order(seller.clone(), order_info.clone(), match_args);
    // Execute instruction...

    // Later: scan matches and extract rent
    let matches = scan_matches(rpc).await.unwrap();
    let match_info = &matches[0];

    let instruction = extract_rent(
        seller.clone(),
        match_info.clone(),
        current_tip_block,
    );
    // Execute instruction...
}
```

## Related Crates

- `opticrum-protocol` (`opticrum-protocol/`) — Shared canonical byte-level data layouts
- `opticrum` (`contracts/opticrum/`) — On-chain RISC-V verification contract
- `opticrum-runner` (`src/`) — CLI for deploy/migrate/consume
