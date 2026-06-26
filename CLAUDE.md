# CLAUDE.md

## Project

**Opticrum** — A decentralized liquidity marketplace for the [Fiber Network](https://github.com/nervosnetwork/fiber) on [CKB](https://github.com/nervosnetwork/ckb). Fully decentralized version of [Amboss](https://amboss.tech/).

Built with the [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) framework (referenced as `../ckb-cinnabar` relative to this repo).

## Architecture

```
Order Cell (buyer creates, 65-byte lock args + 32-byte data)
    ├── Cancel  → buyer reclaims (Burn pattern)
    └── Match   → seller matches with pre-created channel, produces Match Cell
                  (Transfer pattern, no status — immediately active)
                      ├── Extract Rent → seller periodically withdraws linear rent
                      │                  (Transfer pattern, updates last_extraction_block)
                      ├── Inject/Withdraw → buyer adds or removes funds from Match
                      │                  (Transfer pattern, preserves rent_per_block)
                      └── Destroy      → seller sweeps when exhausted (Burn pattern)
```

Two Cell states discriminated by lock script `args` length:
- **Order** (65 bytes): Fiber Pubkey (33) + Buyer Lock Hash (32)
- **Match** (133 bytes): Order args (65) + Channel OutPoint (36) + Seller Lock Hash (32)

**Order Cell data** (32 bytes): xUDT Amount (u128 LE, 16) + Channel Capacity (u64 LE, 8) + Rent Per Block (u64 LE, 8)
**Match Cell data** (32 bytes): xUDT Amount (u128 LE, 16) + Rent Per Block (u64 LE, 8) + Last Extraction Blocknumber (u64 LE, 8)

### Key Design Decisions

**No MatchStatus.** After the seller matches an order, the Match cell is immediately
active — the seller can start extracting rent right away. There is no review phase,
no Frozen→Enabled transition, and no Discard. The buyer's recourse is to withdraw
their funds from the Match cell (emptying it), after which the seller can destroy
the exhausted cell.

**Buyer-specified `rent_per_block`.** The buyer directly specifies the per-block
rent rate in the OrderData. There is no on-chain `AnnualYield` conversion — `rent_per_block`
is the canonical protocol value. An off-chain helper (`annual_yield_to_rent_per_block`)
converts human-readable annual yield (basis points) to `rent_per_block` for convenience.
The total rent capacity locked up at order creation is chosen independently
(no `escrow_blocks` constraint).

**Buyer can inject/withdraw.** The buyer can add (inject) or remove (withdraw)
capacity or xUDT from a Match cell at any time, but cannot destroy it. The buyer
can at most empty the cell down to its minimum occupied capacity.

**Seller extracts + destroys.** The seller periodically extracts rent at the linear
rate `rent_per_block × elapsed_blocks`. When accumulated rent consumes all remaining
value, the match is "exhausted" and only the seller can destroy it.

**No MuSig2 Key Verification.** Fiber Network channels use per-channel generated
seeds, so MuSig2 key aggregation from buyer+seller pubkeys cannot match channel
funding script args. Channel identity is verified via outpoint + Fiber funding
type script.

**`channel_capacity` — verify then discard.** Stored in Order cell data so the match
verifier can load the real channel cell from CellDeps and check that the seller's
channel matches the capacity the buyer requested. Once verified, `channel_capacity`
is not carried into the Match cell — it has served its purpose.

**Channel OutPoint instead of Lock Hash.** Match args store a `channel_outpoint`
(36 bytes: tx_hash + index) rather than a raw lock hash (32 bytes). The verifier
looks up the channel cell by outpoint to confirm both its existence and its
capacity, which is stronger than just comparing a hash.

## Project Structure

```
opticrum/
├── contracts/opticrum/    # On-chain RISC-V verification (no_std, ckb-cinnabar-verifier)
│   ├── src/main.rs        # Entry: cinnabar_main! macro wiring Context + verifiers
│   ├── src/state.rs       # Branch, OpticrumState, OpticrumPattern, Context + convenience methods
│   ├── src/verifiers/     # Cinnabar verification tree
│   │   ├── root.rs           # Root: inspects args length → routes to branch verifier
│   │   ├── order_cancel.rs
│   │   ├── order_match.rs
│   │   ├── match_update.rs   # Match→Match: seller extract OR buyer inject/withdraw (auth-branched)
│   │   └── match_destroy.rs  # Seller destroys exhausted match
│   ├── src/utils.rs       # Helpers: header loading, auth checks, channel lookup, xUDT parsing
│   └── src/error.rs       # OpticrumError enum via define_errors! macro
├── calculator/opticrum/   # Off-chain transaction assembly (ckb-cinnabar-calculator)
│   ├── src/lib.rs         # Module declarations, re-exports
│   ├── src/calculator.rs  # create_order, cancel_order, match_order, extract_rent,
│   │                      #   update_match_buyer, destroy_match + yield helpers
│   ├── src/types.rs       # OrderArgs, OrderData, MatchArgs, MatchData, Xudt,
│   │                      #   OrderInfo, MatchInfo, length constants
│   ├── src/operation.rs   # opticrum_lock(), AddOpticrumContractCelldep Operation
│   ├── src/reader.rs      # scan_orders, scan_matches (shared scan_cells + parse_cell_prologue)
│   └── src/config.rs      # OPTICRUM_CONTRACT_NAME, CKB_DECIMAL, BLOCKS_PER_YEAR, type_id,
│   │                      #   ORDER_TO_MATCH_CAPACITY_RESERVE
├── opticrum-protocol/     # Canonical byte-level types (shared on/off chain)
│   └── src/lib.rs         # All data layouts, length constants, serialization
├── src/main.rs            # CLI runner: deploy/migrate/consume via ckb_cinnabar::dispatch
├── src/bin/               # CLI binaries
│   ├── create_order.rs    #   Buyer creates Order
│   ├── match_order.rs     #   Seller matches Order with channel
│   ├── extract_liquidity_rent.rs  # Seller extracts rent
│   ├── topup_rent.rs      #   Buyer injects rent capacity
│   ├── decline_rent.rs    #   Buyer withdraws rent capacity
│   ├── scan_orders.rs     #   List live Orders
│   └── scan_matches.rs    #   List live Matches
├── tests/                 # Integration tests (CKB simulator + FakeRpcClient)
├── scripts/               # find_clang, reproducible_build_docker
└── Makefile               # Top-level: build (contracts + crates), test, check, clippy, fmt
```

## Build & Test

```bash
make build          # Compile RISC-V contract binary → build/release/opticrum
make test           # Run integration tests (CKB transaction simulator)
make check          # cargo check
make clippy         # cargo clippy
make fmt            # cargo fmt

# Single contract
make build CONTRACT=opticrum

# Specific test
make test CARGO_ARGS="-- --nocapture"
```

## Key Dependencies

- **ckb-cinnabar** — On-chain script framework (verification tree, dispatch, ScriptPattern)
- **ckb-cinnabar-verifier** — `no_std` RISC-V verification primitives (used by contracts/)
- **ckb-cinnabar-calculator** — Off-chain transaction building (used by calculator/)
- All referenced via `path = "../ckb-cinnabar/..."` (sibling repo)

## Cinnabar Verification Tree

The contract uses `cinnabar_main!` with a `Context` struct carrying `old_state` and `new_state`:

```
Root (always runs first, discriminates by args length)
├── Order(65) + None           → "order_cancel"   (Burn)
├── Order(65) + Match(133)     → "order_match"    (Transfer)
├── Match(133) + Match(133)    → "match_update"   (Transfer, internal auth branch)
└── Match(133) + None          → "match_destroy"  (Burn)
```

`match_update` internally branches on auth (since state alone can't distinguish):
- **seller_lock_hash in inputs** → extraction: verifies `rent_per_block × elapsed`,
  updates `last_extraction_block`
- **buyer_lock_hash in inputs** → inject/withdraw: preserves `rent_per_block` and
  `last_extraction_block`, verifies cell viability

ScriptPattern:
- **Burn**: Cell consumed as input, no matching output (cancel/destroy)
- **Transfer**: Cell consumed as input, matching output produced (match/update)

## Calculator Instructions

### create_order(buyer, order_args, order_data, rent_capacity, xudt_type_script?)
Creates an Order Cell with Opticrum lock. Buyer's personal lock signs. `rent_per_block`
is embedded in `order_data`. Total `rent_capacity` is a separate parameter for CKB orders.

### cancel_order(buyer, order_info)
Burns the Order Cell, returning capacity (+ optional xUDT) to the buyer.

### match_order(seller, order_info, match_args)
Consumes Order Cell, produces Match Cell. Adds the pre-created Fiber channel cell as
a CellDep (not consumed). `rent_per_block` is copied directly from OrderData.
`last_extraction_block` is initialized to 0.

### extract_rent(seller, match_info, tip_block)
Seller withdraws linear rent from a Match Cell. On first extraction, a HeaderDep at
match creation block is added. Linear rent: `rent_per_block × (tip_block - last_extraction_block)`.
If exhausted, automatically delegates to `destroy_match`.

### update_match_buyer(buyer, match_info, new_xudt_amount, capacity_delta)
Buyer injects or withdraws capacity / xUDT from a Match Cell. Cannot destroy —
at most can empty the cell. `rent_per_block` and `last_extraction_block` are preserved.

### destroy_match(seller, match_info, tip_block)
Seller destroys an exhausted Match Cell. Only the seller can call this, and only
when accumulated rent >= remaining value.

## Rent Calculation

**Linear formula:** `extractable = rent_per_block × (tip_block - last_extraction_block)`

`rent_per_block` is specified directly by the buyer in OrderData. The on-chain verifier
uses byte-level comparison for `rent_per_block` preservation checks to ensure cross-platform
determinism. An off-chain convenience helper `annual_yield_to_rent_per_block(channel_capacity,
annual_yield_bps)` converts human-readable annual yield (e.g. 500 = 5%) to `rent_per_block`
using `BLOCKS_PER_YEAR ≈ 2,629,800`.

When `accumulated_rent >= remaining_capacity`, the match is **exhausted** — the
seller can destroy it.

## Type Reference

| Type | Fields | Bytes |
|------|--------|-------|
| `OrderArgs` | fiber_pubkey, buyer_lock_hash | 65 |
| `OrderData` | xudt_amount (u128), channel_capacity (u64), shannons_per_block (u64) | 32 |
| `MatchArgs` | order_args, channel_outpoint, seller_lock_hash | 133 |
| `MatchData` | xudt_amount (u128), shannons_per_block (u64), last_extraction_block (u64) | 32 |
| `Xudt` | amount (u128), type_script (Script) | — |
| `OrderInfo` | order_args, order_data, xudt?, ckb_capacity, order_outpoint | — |
| `MatchInfo` | match_args, match_data, xudt?, ckb_capacity, match_outpoint, match_current_block | — |

## Code Conventions

- `no_std` in contracts (RISC-V target: `riscv64imac-unknown-none-elf`)
- Error handling via `define_errors!` macro (CUSTOM_ERROR_START + offset)
- Verifiers implement `Verification<Context>` trait from ckb-cinnabar
- Args parsing: `from_slice()` constructors validating lengths
- Capacity values in shannons (1 CKB = 10^8 shannons)
- Little-endian encoding for all integer fields in args/data
- All rent-related values use `u64` for cross-platform determinism (no f64)
- `CKB_DECIMAL = 100_000_000`
- `BLOCKS_PER_YEAR = 2_629_800`
- `ORDER_TO_MATCH_CAPACITY_RESERVE` = 68 CKB (extra bytes for Match cell: args 133-65 + data 32-32 = 68 bytes × CKB_DECIMAL)
