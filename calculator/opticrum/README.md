# Opticrum Calculator

Off-chain transaction assembly for the Opticrum liquidity marketplace. Builds on
[ckb-cinnabar-calculator](https://github.com/ashuralyk/ckb-cinnabar) and shares all
protocol types with the on-chain contract via `opticrum-protocol`.

## Design Philosophy

### The Calculator Doesn't Validate

A key architectural decision: the calculator **assembles** transactions but never **validates** them. All validation happens on-chain in the RISC-V contract. The calculator constructs the right inputs, outputs, cell deps, and header deps for each operation, but it trusts the contract to reject anything invalid.

This separation keeps the off-chain code simple and stateless. There's no duplicate validation logic to keep in sync. The contract is the single source of truth.

### Operations, Not Transactions

Each instruction builds a `Vec<Box<dyn Operation<T>>>` — a list of composable, ordered steps that ckb-cinnabar assembles into a transaction skeleton. Operations are declarative: `AddInputCellByOutPoint`, `AddOutputCellByInputIndex`, `AddHeaderDepByBlockNumber`. The framework handles capacity balancing, change generation, and serialization.

This composability means instructions can be stacked. A CLI binary typically chains three instructions: `[prepare, business_logic, balance_and_sign]`. The prepare instruction adds common cell deps, the business instruction builds the Opticrum-specific cells, and the balance instruction handles fee calculation and signing.

## Instructions

### create_order

```rust
pub fn create_order<T: RPC>(
    buyer: Address,
    order_args: &OrderArgs,
    order_data: &OrderData,
    rent_capacity: u64,
    xudt_type_script: Option<Script>,
) -> Instruction<T>
```

Creates an Order cell. The Opticrum lock does **not** execute on creation — verification only runs when the cell is consumed. This avoids redundant VM cycles and keeps creation cheap (a simple transfer with a special lock).

For CKB-only orders, `xudt_amount` is zeroed in the stored data regardless of what's in `order_data`. This normalization prevents stale xUDT amounts from leaking into CKB orders.

**Yield helpers** compute `rent_per_block` from human-readable annual yield:

```rust
let rent_per_block = annual_yield_to_rent_per_block(channel_capacity, 500); // 5%
let rent_capacity = rent_per_block.saturating_mul(100_000); // ~10 days
```

The formula: `channel_capacity × yield_bps / (10_000 × BLOCKS_PER_YEAR)` with u128 intermediate precision. `BLOCKS_PER_YEAR ≈ 2,629,800` assumes ~12s CKB blocks.

### cancel_order

```rust
pub fn cancel_order<T: RPC>(buyer: Address, order_info: OrderInfo) -> Instruction<T>
```

Burns the Order cell, returning capacity to the buyer. The output cell uses the buyer's personal lock (not Opticrum), so the funds are fully released. If the Order had xUDT, an xUDT cell dep is added and the tokens are returned alongside the CKB.

### match_order

```rust
pub fn match_order<T: RPC>(
    seller: Address,
    order_info: OrderInfo,
    match_args: MatchArgs,
) -> Instruction<T>
```

The most complex instruction. Consumes the Order cell, produces a Match cell. Key details:

- **Channel CellDep**: The seller's pre-created Fiber channel is added as a CellDep with `with_data: true`. The on-chain verifier inspects the channel's capacity and type script from this dep.
- **Two header deps**: One for the channel's creation block (via `AddHeaderDepByCellDepIndex`) and one for the Order's creation block (via `AddHeaderDepByInputIndex`). Both are needed for the temporal check (channel created after order).
- **`CapacityAdjustment::Keep`**: The output Match cell inherits the Order cell's capacity. No capacity is added or removed — the `ORDER_TO_MATCH_CAPACITY_RESERVE` (68 CKB) pre-funded on the Order covers the extra occupied bytes.
- **Match data initialization**: `shannons_per_block` is copied from OrderData. `last_extraction_block` starts at 0. `xudt_amount` is preserved.

### extract_rent

```rust
pub fn extract_rent<T: RPC>(
    seller: Address,
    match_info: MatchInfo,
    tip_block: u64,
) -> Instruction<T>
```

Seller withdraws vested rent. Auto-delegates to `destroy_match` if the match is exhausted — a pure off-chain convenience; the on-chain contract only sees the Burn pattern.

**First extraction edge case**: When `last_extraction_block == 0`, a HeaderDep for the match creation block is added via `AddHeaderDepByInputIndex`. Since the match cell is the last input added, the framework resolves this to the Match cell's creation header. For subsequent extractions, `last_extraction_block` suffices.

**CKB vs xUDT**: For CKB matches, capacity is reduced via `CapacityAdjustment::Subtract(rent)`. For xUDT matches, the xUDT amount in MatchData is decremented and capacity is kept unchanged.

The `xudt_amount` assignment uses a clean if/else — the CKB branch explicitly sets it to 0, the xUDT branch subtracts the rent. No wasted assignment, no overwrite.

### update_match_buyer

```rust
pub fn update_match_buyer<T: RPC>(
    buyer: Address,
    match_info: MatchInfo,
    new_xudt_amount: u128,
    capacity_delta: i64,
) -> Instruction<T>
```

The buyer's lever on the Match cell. They can inject (positive delta) or withdraw (negative delta) capacity, or adjust the xUDT amount. They cannot destroy the cell — at most they can empty it to its minimum occupied capacity.

`capacity_delta` sign determines the `CapacityAdjustment` variant: `≥ 0 → Add`, `< 0 → Subtract`. `shannons_per_block` and `last_extraction_block` are preserved by not touching them — the new MatchData inherits them from the old MatchData.

### destroy_match

```rust
pub fn destroy_match<T: RPC>(
    seller: Address,
    match_info: MatchInfo,
    tip_block: u64,
) -> Instruction<T>
```

Sweeps an exhausted Match cell. Only the seller can call this (enforced on-chain).

**Header dep strategy**: Uses `AddHeaderDepByBlockNumber{match_current_block}` for the creation block header. This differs from `extract_rent` which uses `AddHeaderDepByInputIndex`. The reason: `destroy_match` may be called after multiple extractions, so the Match cell's input header is no longer the creation block. Using the stored `match_current_block` from when the cell was scanned is more reliable.

## On-Chain Readers

### Shared Infrastructure

Both `scan_orders` and `scan_matches` delegate to a private generic `scan_cells<T, U>(rpc, parse_fn)` helper that handles pagination, iteration, and error suppression. Cells that fail to parse are silently skipped — the indexer may return cells that happen to share the Opticrum lock prefix but aren't actually Order or Match cells.

Similarly, `parse_order_cell` and `parse_match_cell` share a `parse_cell_prologue` function that extracts the common envelope: lock args bytes, output data, outpoint, block number, and the raw `CellOutput`. Each parser then does only its type-specific `from_slice` calls. This eliminates the duplicate OutPoint parsing and xUDT extraction that existed in the original two independent parsers.

### scan_orders

```rust
pub async fn scan_orders<T: RPC>(rpc: &T) -> eyre::Result<Vec<OrderInfo>>
```

Queries the indexer for cells with Opticrum lock (prefix search). Returns fully parsed `OrderInfo` structs with real rent capacity (`total - occupied`).

### scan_matches

```rust
pub async fn scan_matches<T: RPC>(rpc: &T) -> eyre::Result<Vec<MatchInfo>>
```

Same query pattern, returns `MatchInfo` with the additional `match_current_block` field — critical for computing elapsed blocks during extraction and destruction.

## Key Types

### OrderInfo / MatchInfo

Off-chain aggregates that bundle everything needed to consume a cell:

```rust
pub struct OrderInfo {
    pub order_args: OrderArgs,
    pub order_data: OrderData,
    pub xudt: Option<Xudt>,
    pub ckb_capacity: u64,       // real rent = total - occupied
    pub order_outpoint: OutPoint,
}

pub struct MatchInfo {
    pub match_args: MatchArgs,
    pub match_data: MatchData,
    pub xudt: Option<Xudt>,
    pub ckb_capacity: u64,       // real rent = total - occupied
    pub match_outpoint: OutPoint,
    pub match_current_block: u64, // for elapsed-block calculation
}
```

MatchInfo provides two computed methods:
- `extraction_amount(tip_block) → u64` — `shannons_per_block × (tip_block - base_block)`, where `base_block` is `match_current_block` on first extraction (when `last_extraction_block == 0`) or `last_extraction_block` otherwise.
- `is_exhausted(tip_block) → bool` — whether accumulated rent exceeds remaining CKB or xUDT value.

### Protocol Types

All canonical byte-layout types live in `opticrum-protocol` and are re-exported:

| Type | Fields | Bytes |
|------|--------|-------|
| `OrderArgs` | `fiber_pubkey`, `buyer_lock_hash` | 65 |
| `OrderData` | `xudt_amount` (u128), `channel_capacity` (u64), `shannons_per_block` (u64) | 32 |
| `MatchArgs` | `order_args`, `channel_outpoint`, `seller_lock_hash` | 133 |
| `MatchData` | `xudt_amount` (u128), `shannons_per_block` (u64), `last_extraction_block` (u64) | 32 |
| `OutPoint` | `tx_hash` ([u8; 32]), `index` (u32) | 36 |
| `CompressedPubkey` | 33-byte secp256k1 compressed key | 33 |

Every type implements `from_slice(&[u8]) → Result<Self, &'static str>` and `to_bytes() → [u8; N]`. The error type is `&'static str` by design — consumers map it into their own error types (the contract uses `ERR!` → `OpticrumError`, the calculator uses `eyre!`).

## The Reserve Constant

```rust
pub const ORDER_TO_MATCH_CAPACITY_RESERVE: u64 =
    (MATCH_ARGS_LEN - ORDER_ARGS_LEN + MATCH_DATA_LEN - ORDER_DATA_LEN) as u64 * CKB_DECIMAL;
```

This evaluates to 68 CKB. Here's why it exists:

A Match cell occupies 133 bytes of args + 32 bytes of data = 165 bytes. An Order cell occupies 65 + 32 = 97 bytes. The difference is 68 bytes. On CKB, each occupied byte requires 1 CKB of capacity (the `CKB_DECIMAL` constant = 100,000,000 shannons). So the Match cell needs 68 more CKB just to exist.

Without this reserve, the Order→Match transition would require the seller to inject 68 CKB — awkward UX for the party providing a service. Instead, the buyer pre-funds this reserve on the Order cell. The `CapacityAdjustment::Keep` in `match_order` carries it through automatically.

## Config

| Constant | Value | Purpose |
|----------|-------|---------|
| `OPTICRUM_CONTRACT_NAME` | `"opticrum"` | Script lookup via `ScriptEx::Reference` |
| `CKB_DECIMAL` | `100,000,000` | Shannons per CKB |
| `BLOCKS_PER_YEAR` | `2,629,800` | Blocks per year (~12s interval) |
| `ORDER_TO_MATCH_CAPACITY_RESERVE` | 68 CKB | Extra occupied bytes in Match vs Order |

## Module Structure

```
calculator/opticrum/src/
├── lib.rs           # Module declarations + re-exports
├── calculator.rs    # Six instruction builders + fiber_channel_celldep + yield helpers
├── config.rs        # Constants, type_id resolution per network
├── operation.rs     # opticrum_lock(), AddOpticrumContractCelldep
├── reader.rs        # scan_orders, scan_matches, scan_cells, parse_cell_prologue
└── types.rs         # Xudt, OrderInfo, MatchInfo + protocol type re-exports
```

## Dependencies

- `ckb-cinnabar-calculator` — Transaction skeleton, Operation trait, RPC client, indexer
- `opticrum-protocol` — Shared byte-level types (no_std compatible, used by both calculator and contract)
