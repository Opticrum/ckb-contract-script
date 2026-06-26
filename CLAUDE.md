# CLAUDE.md

## Project

**Opticrum** — A decentralized liquidity marketplace for the [Fiber Network](https://github.com/nervosnetwork/fiber) on [CKB](https://github.com/nervosnetwork/ckb). Fully decentralized version of [Amboss](https://amboss.tech/).

Built with the [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) framework (referenced as `../ckb-cinnabar` relative to this repo).

## Architecture

```
Order Cell (buyer creates, 65-byte lock args + 32-byte data)
    ├── Cancel  → buyer reclaims (Burn pattern)
    └── Match   → seller matches with pre-created channel, produces Match Cell
                  (Transfer pattern, status=Frozen)
                      ├── Confirm     → buyer enables rent extraction (Frozen→Enabled)
                      ├── Auto-Enable → seller enables after 3 days (Frozen→Enabled)
                      ├── Discard     → buyer rejects, extracts extra capacity (Frozen→Discarded)
                      │                  └── Destroy → seller sweeps (Burn pattern)
                      ├── Extract Rent → seller periodically withdraws linear rent
                      │                  (Enabled→Enabled, Transfer pattern)
                      └── Destroy      → exhausted, anyone sweeps (Burn pattern)
```

Two Cell states discriminated by lock script `args` length:
- **Order** (65 bytes): Fiber Pubkey (33) + Buyer Lock Hash (32)
- **Match** (133 bytes): Order args (65) + Channel OutPoint (36) + Seller Lock Hash (32)

**Order Cell data** (32 bytes): xUDT Amount (u128 LE, 16) + Channel Capacity (u64 LE, 8) + Escrow Blocks (u64 LE, 8)
**Match Cell data** (41 bytes): xUDT Amount (u128 LE, 16) + Rent Per Block (f64 LE, 8) + Escrow Blocks (u64 LE, 8) + Last Extraction Blocknumber (u64 LE, 8) + Status (u8: 0=Frozen, 1=Enabled, 2=Discarded)

### Key Design Decisions

**Match Review Phase.** After matching, the Match cell enters a Frozen state where
the seller cannot extract rent. The buyer must manually verify the channel (via
its outpoint) and confirm the match. If the buyer doesn't act within 3 days
(`ABOUT_THREE_DAYS_BLOCKS = 30_000`), the seller can auto-enable it. The buyer
can also discard the match, extracting their extra capacity and leaving the cell
for the seller to destroy.

**No MuSig2 Key Verification.** Fiber Network channels use per-channel generated
seeds, so MuSig2 key aggregation from buyer+seller pubkeys cannot match channel
funding script args. Channel identity is verified via outpoint + Fiber funding
type script. The seller's fiber_pubkey is removed from MatchArgs (buyer's
fiber_pubkey remains in OrderArgs for counterparty identification).

**`channel_capacity` — verify then discard.** Stored in Order cell data so the match
verifier can load the real channel cell from CellDeps and check that the seller's
channel matches the capacity the buyer requested. Once verified, `channel_capacity`
is not carried into the Match cell — it has served its purpose.

**`escrow_blocks` — transform into `rent_per_block`.** At match time the escrow
duration is converted into a pre-computed linear rate: `rent_per_block = total_rent / escrow_blocks`.
This replaces the old proportional formula with a single multiplication:
`rent_per_block × elapsed`. The original `escrow_blocks` value is carried into
Match data for expiry verification.

**Channel OutPoint instead of Lock Hash.** Match args store a `channel_outpoint`
(36 bytes: tx_hash + index) rather than a raw lock hash (32 bytes). The verifier
looks up the channel cell by outpoint to confirm both its existence and its
capacity, which is stronger than just comparing a hash.

Match args now total 133 bytes (removed seller fiber_pubkey).

## Project Structure

```
opticrum/
├── contracts/opticrum/    # On-chain RISC-V verification (no_std, ckb-cinnabar-verifier)
│   ├── src/main.rs        # Entry: cinnabar_main! macro wiring Context + verifiers
│   ├── src/verifiers/     # Cinnabar verification tree
│   │   ├── root.rs           # Root: inspects args length + status → routes to branch verifier
│   │   ├── order_cancel.rs
│   │   ├── order_match.rs
│   │   ├── match_enable.rs    # Frozen → Enabled (buyer confirm or seller auto-enable)
│   │   ├── match_discard.rs   # Frozen → Discarded (buyer reject)
│   │   ├── match_extract.rs
│   │   └── match_destroy.rs
│   ├── src/utils.rs       # Args parsing (OrderArgs, MatchArgs), MatchData, rent math
│   └── src/error.rs       # OpticrumError enum via define_errors! macro
├── calculator/opticrum/   # Off-chain transaction assembly (ckb-cinnabar-calculator)
│   ├── src/lib.rs         # Module declarations, re-exports
│   ├── src/calculator.rs  # create_order, cancel_order, match_order, extract_rent
│   ├── src/types.rs       # OrderArgs, OrderData, MatchArgs, MatchData, Xudt,
│   │                      #   AnnualYield, OrderInfo, MatchInfo, length constants
│   ├── src/operation.rs   # opticrum_lock(), AddOpticrumContractCelldep Operation
│   ├── src/reader.rs      # scan_orders, scan_matches, get_order_info, get_match_info
│   └── src/config.rs      # OPTICRUM_CONTRACT_NAME, ABOUT_ONE_DAY_BLOCKS, type_id
├── src/main.rs            # CLI runner: deploy/migrate/consume via ckb_cinnabar::dispatch
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
Root (always runs first)
├── Order(65) + None                     → "order_cancel"   (Burn)
├── Order(65) + Match(Frozen)            → "order_match"    (Transfer)
├── Match(133) + Match(Enabled)          → "match_enable"   (Transfer)
├── Match(133) + Match(Discarded)        → "match_discard"  (Transfer)
├── Match(133) + Match(Enabled)          → "match_extract"  (Transfer)
└── Match(133) + None                    → "match_destroy"  (Burn)
```

MatchEnable handles both paths:
- **Buyer confirm**: `buyer_lock_hash` in inputs — no timing requirement
- **Seller auto-enable**: `seller_lock_hash` in inputs + HeaderDep proves `tip - creation >= ABOUT_THREE_DAYS_BLOCKS`

MatchDestroy handles:
- **Frozen**: rejected (cannot destroy)
- **Discarded**: only seller can destroy
- **Enabled + exhausted**: buyer or seller can destroy

ScriptPattern is determined by how the Cell is consumed in the transaction:
- **Burn**: Cell consumed as input, no matching output (cancel/destroy)
- **Transfer**: Cell consumed as input, matching output produced (match/enable/discard/extract)

## Calculator Instructions

### create_order(buyer, order_args, order_data, annual_yield, xudt_type_script?)
Creates an Order Cell with Opticrum lock. The buyer's personal lock signs. Order data holds xUDT amount (or empty for CKB). Capacity is rent_capacity computed from AnnualYield × OrderData.

### cancel_order(buyer, order_info)
Burns the Order Cell, returning capacity (+ optional xUDT) to the buyer. Verifier checks buyer's lock hash matches `buyer_lock_hash` in Order args.

### match_order(seller, order_info, match_args)
Consumes Order Cell, produces Match Cell (Frozen). Adds the pre-created Fiber channel cell as a CellDep (not consumed). MatchData's `rent_per_block` is computed as `total_rent / escrow_blocks`. Match args embed the channel's OutPoint. Status is set to Frozen.

### confirm_match(buyer, match_info)
Buyer confirms a Frozen Match → Enabled. Buyer signs. No timing requirement. The match must have been manually verified (channel outpoint) offline.

### auto_enable_match(seller, match_info, tip_block)
Seller auto-enables a Frozen Match → Enabled after 3-day review window. Seller signs. HeaderDep[0]=tip, HeaderDep[1]=match_creation_block. Verifier checks `tip - creation >= ABOUT_THREE_DAYS_BLOCKS`.

### discard_match(buyer, match_info)
Buyer rejects a Frozen Match → Discarded. Buyer signs. Buyer extracts extra capacity (unoccupied above minimum occupied). Seller must later destroy.

### extract_rent(seller, match_info, tip_block)
Seller withdraws rent from Enabled Match Cell. On first extraction, a HeaderDep at match creation block is added to prove the match's age. Linear rent: `rent_per_block × (tip_block - last_extraction_block)`. If the accumulated rent exceeds remaining capacity ("exhausted"), all remaining goes to the seller (effectively destroying the Match).

### destroy_match(claimant, match_info, tip_block)
Destroys a Match Cell. Authorization depends on status:
- Frozen: rejected (cannot destroy)
- Discarded: only seller
- Enabled + exhausted: buyer or seller

## Rent Calculation

**Linear formula:** `extractable = rent_per_block × (tip_block - last_extraction_block)`

`rent_per_block` is pre-computed off-chain at match time as `total_rent_capacity / escrow_blocks` (floating point). This eliminates the old proportional formula (`remaining × elapsed / remaining_at_last`), simplifying on-chain verification to a single multiplication.

When `accumulated_rent >= remaining_capacity`, the match is **exhausted** — the seller receives everything and no updated Match Cell is produced (the cell is effectively burned via the exhausted path).

## Type Reference

| Type | Fields | Bytes |
|------|--------|-------|
| `OrderArgs` | fiber_pubkey, buyer_lock_hash | 65 |
| `OrderData` | xudt_amount, channel_capacity, escrow_blocks | 32 |
| `MatchArgs` | order_args, channel_outpoint, seller_lock_hash | 133 |
| `MatchData` | xudt_amount, rent_per_block, escrow_blocks, last_extraction_block, status | 41 |
| `MatchStatus` | Frozen=0, Enabled=1, Discarded=2 | 1 |
| `Xudt` | amount (u128), type_script (Script) | — |
| `AnnualYield` | percentage (u8) | — |
| `OrderInfo` | order_args, order_data, xudt?, ckb_capacity, order_outpoint | — |
| `MatchInfo` | match_args, match_data, xudt?, ckb_capacity, match_outpoint, match_current_block | — |

## Code Conventions

- `no_std` in contracts (RISC-V target: `riscv64imac-unknown-none-elf`)
- Error handling via `define_errors!` macro (CUSTOM_ERROR_START + offset)
- Verifiers implement `Verification<Context>` trait from ckb-cinnabar
- Args parsing: `from_slice()` constructors validating lengths
- Capacity values in shannons (1 CKB = 10^8 shannons)
- Little-endian encoding for all integer fields in args/data
- `ABOUT_ONE_DAY_BLOCKS = 10_000` (approximate, used in AnnualYield calculations)
- `ABOUT_THREE_DAYS_BLOCKS = 30_000` (buyer review window)
- `CKB_DECIMAL = 100_000_000`
- `ORDER_TO_MATCH_CAPACITY_RESERVE` = 77 CKB (extra bytes for Match cell: args 133-65 + data 41-32 = 77 bytes × CKB_DECIMAL)
