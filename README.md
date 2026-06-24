# Opticrum

A decentralized liquidity marketplace for the [Fiber Network](https://github.com/nervosnetwork/fiber) on [CKB](https://github.com/nervosnetwork/ckb).

Opticrum is the fully decentralized version of [Amboss](https://amboss.tech/) — a liquidity marketplace where:

- **Buyers** create on-chain Order Cells offering rent for inbound channel liquidity
- **Sellers** match orders by referencing pre-created Fiber channels, earning rent linearly over time

Built with the [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) framework.

## Architecture

```
Order Cell (buyer creates, 65-byte lock args + 32-byte data)
    ├── Cancel  → buyer reclaims (Burn pattern)
    └── Match   → seller matches with pre-created channel, produces Match Cell
                  (Transfer pattern)
                      ├── Extract Rent → seller periodically withdraws linear rent
                      │                  (Transfer pattern)
                      └── Destroy      → exhausted, seller or buyer sweeps remaining
                                         (Burn pattern)
```

Two Cell states discriminated by lock script `args` length:

| State | Args Length | Contents |
|-------|------------|----------|
| **Order** | 65 bytes | `fiber_pubkey` (33) + `buyer_lock_hash` (32) |
| **Match** | 133 bytes | Order args (65) + `channel_outpoint` (36) + `seller_lock_hash` (32) |

### Cell Data Layout

**Order Cell data** (32 bytes):

| Field | Offset | Size | Type | Description |
|-------|--------|------|------|-------------|
| `xudt_amount` | 0 | 16 | u128 LE | xUDT token amount (0 for CKB orders) |
| `channel_capacity` | 16 | 8 | u64 LE | Minimum channel capacity requested |
| `escrow_blocks` | 24 | 8 | u64 LE | Duration of the escrow period |

**Match Cell data** (40 bytes):

| Field | Offset | Size | Type | Description |
|-------|--------|------|------|-------------|
| `xudt_amount` | 0 | 16 | u128 LE | Remaining xUDT in escrow |
| `rent_per_block` | 16 | 8 | f64 LE | Pre-computed linear rent rate |
| `escrow_blocks` | 24 | 8 | u64 LE | Escrow duration (for expiry checks) |
| `last_extraction_block` | 32 | 8 | u64 LE | Block number of last extraction |

### Key Design Decisions

**`channel_capacity` — verify then discard.** Stored in Order cell data so the match
verifier can load the real channel cell from CellDeps and check that the seller's
channel matches the capacity the buyer requested. Once verified, `channel_capacity`
is not carried into the Match cell — it has served its purpose.

**`escrow_blocks` — stored in Match data for expiry.** The escrow duration is carried
into the Match cell so the destroy verifier can compute expiry without loading the
original Order cell from a header proof.

**Linear rent: `rent_per_block`.** At match time the total rent is converted into a
pre-computed linear rate: `rent_per_block = total_rent / escrow_blocks`. This reduces
on-chain extraction verification to a single multiplication: `rent_per_block × elapsed`.
The `rent_per_block` is set once at match time and never changes.

**Channel OutPoint instead of Lock Hash.** Match args store a `channel_outpoint`
(36 bytes: tx_hash[32] + index[4] u32 LE) rather than a raw lock hash (32 bytes).
The verifier looks up the channel cell by outpoint to confirm both its existence and
its capacity — stronger than just comparing a hash.

## Project Structure

```
opticrum/
├── contracts/opticrum/       # On-chain RISC-V verification (no_std)
│   ├── src/main.rs           # Entry: cinnabar_main! macro — Context + verifiers
│   ├── src/verifiers/        # Cinnabar verification tree
│   │   ├── root.rs           # Root: inspects args length → routes to branch
│   │   ├── order_cancel.rs   # Buyer reclaims unmatched order
│   │   ├── order_match.rs    # Seller matches order with channel
│   │   ├── match_extract.rs  # Seller withdraws linear rent
│   │   └── match_destroy.rs  # Exhausted match swept by seller or buyer
│   ├── src/utils.rs          # Channel lookup, authorization, header helpers
│   └── src/error.rs          # OpticrumError enum via define_errors! macro
├── opticrum-protocol/        # Shared canonical data layouts (no_std + std)
│   └── src/lib.rs            # OrderArgs, OrderData, MatchArgs, MatchData,
│                             #   OutPoint, all length constants
├── calculator/opticrum/      # Off-chain transaction assembly
│   ├── src/lib.rs            # Module declarations, re-exports
│   ├── src/calculator.rs     # create_order, cancel_order, match_order,
│   │                         #   extract_rent, destroy_match
│   ├── src/types.rs          # Xudt, AnnualYield, OrderInfo, MatchInfo
│   ├── src/operation.rs      # opticrum_lock(), AddOpticrumContractCelldep
│   ├── src/reader.rs         # scan_orders, scan_matches, get_order/matched_info
│   └── src/config.rs         # Contract name, type_id, constants
├── src/main.rs               # CLI runner: deploy/migrate/consume
├── tests/                    # Integration tests (CKB simulator + FakeRpcClient)
├── scripts/                  # find_clang, reproducible_build_docker
└── Makefile                  # Top-level: build, test, check, clippy, fmt
```

### Crate Dependency Graph

```
opticrum-protocol (canonical byte layouts, no_std)
    ↑                   ↑
contracts/opticrum    calculator/opticrum
(no_std, RISC-V)      (std, off-chain)
    ↑                   ↑
opticrum-runner (src/main.rs, CLI via ckb-cinnabar::dispatch)
```

## Cinnabar Verification Tree

The contract uses `cinnabar_main!` with a `Context` struct carrying `old_state` and
optional `new_state`. The Root verifier inspects lock script args length and
ScriptPattern to route to the correct branch:

```
Root (always runs first)
├── args_len == 65 (Order)
│   ├── Burn     → "order_cancel"
│   └── Transfer → "order_match"
└── args_len == 133 (Match)
    ├── Transfer → "match_extract"
    └── Burn     → "match_destroy"
```

**ScriptPattern** is determined by how the Cell is consumed in the transaction:
- **Burn**: Cell consumed as input, no matching Opticrum output (cancel / destroy)
- **Transfer**: Cell consumed as input, matching Opticrum output produced (match / extract)

## Operations

### 1. Create Order

Buyer creates an Order Cell with Opticrum lock. The buyer's personal lock signs
the transaction. The lock does NOT execute on creation — verification only runs
when the cell is consumed.

```
CellDeps:  [Opticrum contract]
Inputs:    [Buyer's cell]
Outputs:   [Order Cell]
```

The calculator computes `rent_capacity` from `AnnualYield` × Order Data, or
accepts `xudt_amount` for token-denominated orders.

### 2. Cancel Order

Buyer reclaims an unmatched Order Cell. Verifier checks that the buyer's lock
hash matches `buyer_lock_hash` in the Order args.

```
CellDeps:  [Opticrum contract]
Inputs:    [Order Cell (Burn), Buyer's cell]
```

### 3. Match Order

Seller matches an Order by referencing a pre-created Fiber channel. The channel
cell is added as a CellDep (not consumed). The verifier checks:
1. Channel Cell exists in CellDeps with matching OutPoint and Fiber funding type ID, sufficient capacity
2. Match args' first 65 bytes match Order args
3. Match data initialized (rent_per_block > 0, escrow_blocks > 0, last_extraction_block == 0)
4. Match capacity equals Order capacity (rent transferred intact)
5. xUDT amount unchanged from Order to Match
6. Seller authorizes the transaction

```
CellDeps:  [Opticrum contract, Channel Cell]
Inputs:    [Order Cell (Transfer), Seller's cell]
Outputs:   [Match Cell, Seller change]
```

### 4. Extract Rent

Seller withdraws linearly-vested rent from a Match Cell. The verifier checks:
1. Channel cell still exists in CellDeps (existence only)
2. Seller authorizes the transaction
3. Match is not already exhausted
4. Extraction amount equals `rent_per_block × (tip_block - last_extraction_block)`
5. Match data fields updated correctly (only `last_extraction_block` changes to `tip_block`)
6. Output cell remains viable (capacity >= occupied)

```
CellDeps:       [Opticrum contract]
HeaderDeps:     [tip_header] ( + creation_header on first extraction)
Inputs:         [Match Cell (Transfer), Seller's cell]
Outputs:        [Updated Match Cell, Seller cell + rent]
```

If the accumulated rent exceeds remaining capacity, the match is **exhausted** —
the extract function delegates to destroy internally.

### 5. Destroy Match

After the match is exhausted (accumulated linear rent >= remaining capacity),
the seller or buyer can destroy the Match Cell and sweep remaining funds.

```
CellDeps:       [Opticrum contract]
HeaderDeps:     [tip_header] ( + creation_header if never extracted)
Inputs:         [Match Cell (Burn), Claimant's cell]
```

## Rent Calculation

**Linear formula:** `extractable = rent_per_block × (tip_block - last_extraction_block)`

`rent_per_block` is pre-computed off-chain at match time as `total_rent / escrow_blocks`
(stored as f64). This eliminates proportional arithmetic, simplifying on-chain
verification to a single multiplication. However, `rent_per_block` is intentionally
not compared during extraction updates — f64 equality is unreliable across
platforms (hardware FPU vs RISC-V soft-float). It is an invariant set at match
time and never changes.

When `accumulated_rent >= remaining_capacity`, the match is **exhausted** — the
full remaining capacity (+ xUDT) is released and no updated Match Cell is produced.

## Type Reference

| Type | Fields | Size |
|------|--------|------|
| `OrderArgs` | `fiber_pubkey` (33), `buyer_lock_hash` (32) | 65 bytes |
| `OrderData` | `xudt_amount` (u128), `channel_capacity` (u64), `escrow_blocks` (u64) | 32 bytes |
| `MatchArgs` | `order_args` (65), `channel_outpoint` (36), `seller_lock_hash` (32) | 133 bytes |
| `MatchData` | `xudt_amount` (u128), `rent_per_block` (f64), `escrow_blocks` (u64), `last_extraction_block` (u64) | 40 bytes |
| `OutPoint` | `tx_hash` (32), `index` (u32) | 36 bytes |
| `Xudt` | `amount` (u128), `type_script` (Script) | — |
| `AnnualYield` | `percentage` (u8) | — |
| `OrderInfo` | `order_args`, `order_data`, `xudt?`, `ckb_capacity`, `order_outpoint` | — |
| `MatchInfo` | `match_args`, `match_data`, `xudt?`, `ckb_capacity`, `match_outpoint`, `match_current_block` | — |

## Build & Test

```bash
make build          # Compile RISC-V contract binary → build/release/opticrum
make test           # Run integration tests (CKB transaction simulator)
make check          # cargo check
make clippy         # cargo clippy
make fmt            # cargo fmt

# Single contract
make build CONTRACT=opticrum

# Specific test with output
make test CARGO_ARGS="-- --nocapture"

# Prepare RISC-V target
make prepare        # rustup target add riscv64imac-unknown-none-elf
```

## Key Dependencies

- **[ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar)** — On-chain script framework (verification tree, dispatch, ScriptPattern)
- **ckb-cinnabar-verifier** — `no_std` RISC-V verification primitives (used by contracts/ and opticrum-protocol/)
- **ckb-cinnabar-calculator** — Off-chain transaction building (used by calculator/)
- All referenced via `path = "../ckb-cinnabar/..."` (sibling repo)

## Error Codes

| Error | Description |
|-------|-------------|
| `BadOrderCancel` | Order cancellation validation failed |
| `BadOrderMatch` | Order matching validation failed |
| `ChannelCellNotInDep` | Required Channel Cell not found in CellDeps |
| `ChannelCapacityMismatch` | Order → Match capacity mismatch |
| `OrderDataNotSet` | Order data missing or malformed |
| `BadXudtAmount` | xUDT amount mismatch between Order and Match |
| `BadExtractionAmount` | Rent extraction amount differs from computed value |
| `MatchDataNotSet` | Match data incorrectly initialized |
| `HeaderNotSet` | Required header dependency missing |
| `BadMatchDataUpdate` | Match data fields incorrectly updated |
| `MatchAlreadyExhausted` | Attempt to extract from already-exhausted match |
| `MatchNotExhausted` | Attempt to destroy before match is exhausted |
| `BadArgsLength` | Lock args wrong length (not 65 or 133) |
| `BuyerAuthMissing` | Buyer not found in transaction inputs |
| `SellerAuthMissing` | Seller not found in transaction inputs |
| `AuthorizationMissing` | Neither seller nor buyer found in inputs (destroy) |
| `UnexpectedBranch` | Branch type mismatch (Order vs Match) |
| `UnknownState` | Unrecognized cell state or pattern |

## Code Conventions

- `no_std` in contracts and protocol crate (RISC-V target: `riscv64imac-unknown-none-elf`)
- Error handling via `define_errors!` macro (`CUSTOM_ERROR_START + offset`)
- Verifiers implement `Verification<Context>` trait from ckb-cinnabar
- Args parsing: `from_slice()` constructors validating lengths, returning `Result<T, &'static str>`
- Protocol types defined canonically in `opticrum-protocol/` and re-exported by both contracts and calculator
- Capacity values in shannons (1 CKB = 10^8 shannons)
- Little-endian encoding for all integer fields in args/data
- `ABOUT_ONE_DAY_BLOCKS = 10_000` (approximate, used in AnnualYield calculations)
- `CKB_DECIMAL = 100_000_000`
- RISC-V extensions: `+zba,+zbb,+zbc,+zbs`
