# Opticrum Calculator

Off-chain transaction assembly for the Opticrum liquidity marketplace. Builds on
[ckb-cinnabar-calculator](https://github.com/ashuralyk/ckb-cinnabar) and shares all
protocol types with the on-chain contract via `opticrum-protocol`.

## How It Works

The calculator provides high-level `Instruction` builders that compose ckb-cinnabar
`Operation`s into ready-to-execute transaction recipes. Each instruction mirrors one
step in the Opticrum lifecycle and constructs the corresponding CKB transaction with
correct cell deps, inputs, outputs, and witnesses.

It also provides on-chain readers that query the CKB indexer and parse raw cell data
into typed structs for application use.

## Instructions

### `create_order(buyer, order_args, order_data, annual_yield, xudt_type_script?)`

Creates an Order cell locked by the Opticrum contract. The buyer's personal lock signs.
For CKB-denominated orders, `rent_capacity` is computed from `AnnualYield` × order data;
for xUDT orders, the token amount is stored in cell data with 0 extra capacity.

The Opticrum lock does NOT execute on creation — verification only runs when the cell
is consumed. This avoids redundant checks and keeps creation cheap.

### `cancel_order(buyer, order_info)`

Burns the Order cell, returning capacity (and optional xUDT) to the buyer. The on-chain
verifier checks the buyer's lock hash matches `buyer_lock_hash` in Order args.

### `match_order(seller, order_info, match_args)`

Consumes an Order cell and produces a Match cell. The seller's pre-created Fiber channel
is added as a CellDep — it is **referenced**, not consumed.

The calculator computes `MatchData` with `rent_per_block = total_rent / escrow_blocks`
and embeds the channel `OutPoint` (36 bytes) in Match args. The seller's `fiber_pubkey`
(33 bytes, compressed secp256k1) is appended to Match args alongside the buyer's pubkey
carried from Order args, enabling the on-chain MuSig2 verification.

Match capacity must equal Order capacity — rent transfers intact. The `ORDER_TO_MATCH_CAPACITY_RESERVE`
constant (10.9 CKB) is pre-funded on Order cells to cover the 109 extra occupied bytes of
Match cells, so the seller doesn't need to inject CKB.

### `extract_rent(seller, match_info, tip_block)`

Seller withdraws linearly-vested rent using the formula:

```
extractable = rent_per_block × (tip_block - last_extraction_block)
```

On first extraction, a `HeaderDep` at the match creation block is added to prove the
match's age. The channel CellDep is included so the contract can verify the channel
still exists. If accumulated rent exceeds remaining capacity, the instruction delegates
to `destroy_match` internally — the match is exhausted.

### `destroy_match(claimant, match_info, tip_block)`

Sweeps remaining funds from an exhausted/expired Match cell. Adds a creation-block
HeaderDep if the match was never extracted. The on-chain verifier allows either the
buyer or seller to claim.

## On-Chain Readers

### `scan_orders(rpc) → Vec<OrderInfo>`

Queries the CKB indexer for live Order cells (Opticrum lock with exactly 65-byte args).
Parses each into `OrderInfo`, computing real rent capacity as `total - occupied`.

### `scan_matches(rpc) → Vec<MatchInfo>`

Queries for live Match cells (166-byte args). Parses into `MatchInfo` including
`match_current_block` — the block number when the match was created.

Both use prefix search for efficient indexer queries, paginated in batches of 50.

## Key Types

### `AnnualYield`

Wraps a `u8` percentage. Converts to rent capacity via `ABOUT_ONE_DAY_BLOCKS ≈ 10,000`:

- CKB: `rent = yield% × channel_capacity × escrow_blocks / (365 × 10,000)`
- xUDT: same formula applied to token amounts

### `OrderInfo` / `MatchInfo`

Parsed representations of on-chain cells. Carry all fields needed to consume or update
the cell: parsed args, parsed data, optional xUDT info, real rent capacity, and the
on-chain outpoint. `MatchInfo` additionally holds the match creation block number.

`MatchInfo` provides helpers:
- `extraction_amount(tip_block) → u64` — linear rent calculation
- `is_exhausted(tip_block) → bool` — exhaustion check

### `Xudt`

Token attachment: `amount` (u128) + `type_script` (Script). Set on Order/Match cells
for token-denominated orders.

## Config

| Constant | Value | Purpose |
|----------|-------|---------|
| `OPTICRUM_CONTRACT_NAME` | `"opticrum"` | Script lookup name |
| `ABOUT_ONE_DAY_BLOCKS` | `10,000` | Blocks per day (approximate) |
| `CKB_DECIMAL` | `100,000,000` | Shannons per CKB |
| `ORDER_TO_MATCH_CAPACITY_RESERVE` | 10.9 CKB | Extra bytes in Match vs Order |

## Usage

```rust
use opticrum_calculator::{create_order, match_order, extract_rent, scan_orders, scan_matches};
use opticrum_calculator::types::{AnnualYield, OrderArgs, OrderData, MatchArgs};

// Create an order
let order_args = OrderArgs::new(buyer_fiber_pk, buyer_lock_hash);
let order_data = OrderData::new(/*xudt*/ 0, channel_capacity, escrow_blocks);
let instruction = create_order(buyer, &order_args, &order_data, AnnualYield(10), None);

// Scan and match
let orders = scan_orders(rpc).await?;
let match_args = MatchArgs::new(order_args, channel_outpoint, seller_lock_hash, seller_fiber_pk);
let instruction = match_order(seller, orders[0].clone(), match_args);

// Extract rent later
let matches = scan_matches(rpc).await?;
let instruction = extract_rent(seller, matches[0].clone(), tip_block);
```

## Dependencies

- `ckb-cinnabar-calculator` — Transaction skeleton, operations, RPC, indexer
- `opticrum-protocol` — Shared byte-level data layouts (OrderArgs, MatchArgs, etc.)
