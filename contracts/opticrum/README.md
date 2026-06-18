# Opticrum Contract

On-chain verification contract for the Opticrum decentralized liquidity marketplace. Runs on CKB's RISC-V VM via the [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) verification framework.

## Overview

This contract enforces the rules of Opticrum's liquidity marketplace on-chain. Each transaction that touches an Opticrum Cell must satisfy the verification logic appropriate to its state (Order vs Match) and the operation being performed.

The contract verifies structural correctness, authorization, and rent math. All byte-level protocol types are defined canonically in the `opticrum-protocol` crate and shared between this contract and the off-chain calculator.

## Cell Model

Two Cell states, discriminated by lock script `args` length:

### Order Cell (64-byte args)

| Field | Offset | Size | Type |
|-------|--------|------|------|
| Fiber Pubkey | 0 | 32 | bytes |
| Buyer Lock Hash | 32 | 32 | bytes |

### Order Cell Data (32 bytes)

Stored as the cell's `data` field.

| Field | Offset | Size | Type |
|-------|--------|------|------|
| xUDT Amount | 0 | 16 | u128 LE |
| Channel Capacity | 16 | 8 | u64 LE |
| Escrow Blocks | 24 | 8 | u64 LE |

### Match Cell (132-byte args)

| Field | Offset | Size | Type |
|-------|--------|------|------|
| *(Order fields)* | 0 | 64 | *(same as Order args)* |
| Channel OutPoint | 64 | 36 | tx_hash[32] + index[4] (u32 LE) |
| Seller Lock Hash | 100 | 32 | bytes |

### Match Cell Data (40 bytes)

| Field | Offset | Size | Type |
|-------|--------|------|------|
| xUDT Amount | 0 | 16 | u128 LE |
| Rent Per Block | 16 | 8 | f64 LE |
| Escrow Blocks | 24 | 8 | u64 LE |
| Last Extraction Block | 32 | 8 | u64 LE |

`rent_per_block` is pre-computed at match time as `total_rent / escrow_blocks` and never changes. `escrow_blocks` is stored so the destroy verifier can compute expiry without loading the original Order cell. `rent_per_block` is intentionally not compared during extraction updates — f64 equality is unreliable across platforms (hardware FPU vs RISC-V soft-float).

## Verification Tree

The `Root` verifier inspects `args` length and `ScriptPattern` to route to the correct branch:

```
Root
├── args_len == 64 (Order)
│   ├── Burn     → OrderCancel
│   └── Transfer → OrderMatch
└── args_len == 132 (Match)
    ├── Transfer → MatchExtract
    └── Burn     → MatchDestroy
```

`ScriptPattern` is determined by how the Cell appears in the transaction:
- **Burn** — Cell in inputs but no matching Opticrum output (being consumed/destroyed)
- **Transfer** — Cell in both inputs and outputs with matching Opticrum lock (being updated)
- **Create** — Cell only in outputs (rejected: lock doesn't execute on creation)

The Root verifier also populates the `Context` struct, parsing both the input cell's state (`old_state`) and, if present, the output cell's state (`new_state`). The branch verifiers consume this context.

## Operations

### 1. Cancel Order (`OrderCancel`)

Buyer reclaims an unmatched Order Cell.

**Checks:**
- Buyer's lock hash appears in transaction inputs (proves buyer authorized the cancel)
- Script pattern is Burn

### 2. Match Order (`OrderMatch`)

Seller matches an Order by referencing a pre-created Fiber channel.

**Checks:**
1. Channel Cell exists in CellDeps with matching OutPoint and Fiber funding type ID, and sufficient capacity and/or xUDT amount (depending on Order type)
2. Match Cell args correctly extend Order args (first 64 bytes must match)
3. Match Cell data initialized: `rent_per_block > 0`, `escrow_blocks > 0`, `last_extraction_block == 0`
4. Match Cell capacity equals Order Cell capacity (rent transferred intact)
5. xUDT amount unchanged from Order to Match
6. Seller's lock hash appears in transaction inputs

### 3. Extract Rent (`MatchExtract`)

Seller withdraws linearly-vested rent from a Match Cell.

**Linear rent formula:** `extractable = rent_per_block × (tip_block - last_extraction_block)`

If `last_extraction_block == 0` (never extracted), the creation block from `HeaderDeps[1]` is used instead.

**Checks:**
1. Channel cell still exists in CellDeps (existence only — amounts already verified at match time)
2. Seller's lock hash appears in transaction inputs
3. Match is not already exhausted (`accumulated_rent < remaining_capacity`)
4. Extraction amount exactly equals the computed linear rent
5. Match data fields correctly updated (only `last_extraction_block` changes to `tip_block`; `xudt_amount`, `rent_per_block`, and `escrow_blocks` stay the same)
6. Match args unchanged between input and output
7. Output cell remains viable (capacity >= occupied capacity)

**Exhaustion:** When `accumulated_rent >= remaining_capacity`, the match is exhausted. The extract function delegates to destroy internally — the seller receives everything and no Match output is produced.

### 4. Destroy Match (`MatchDestroy`)

After the match is exhausted, the seller or buyer can destroy the Match Cell and sweep remaining funds.

**Checks:**
1. Match is exhausted: `accumulated_rent >= remaining_capacity`
2. Seller's lock hash OR buyer's lock hash appears in transaction inputs

This is the safety valve against abandoned matches — once enough rent has vested, the remaining funds can be recovered. The economic incentive to extract regularly prevents premature destruction.

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
| `MatchDataNotSet` | Match data incorrectly initialized (rent_per_block, escrow_blocks, or last_extraction_block) |
| `HeaderNotSet` | Required header dependency missing |
| `BadMatchDataUpdate` | Match data fields incorrectly updated during extraction |
| `MatchAlreadyExhausted` | Attempt to extract from already-exhausted match |
| `MatchNotExhausted` | Attempt to destroy before match is exhausted |
| `BadArgsLength` | Lock args wrong length (not 64 or 132) |
| `BuyerAuthMissing` | Buyer not found in transaction inputs |
| `SellerAuthMissing` | Seller not found in transaction inputs |
| `AuthorizationMissing` | Neither seller nor buyer found in inputs (destroy) |
| `UnexpectedBranch` | Branch type mismatch (Order vs Match) |
| `UnknownState` | Unrecognized cell state or ScriptPattern |

## Build

```bash
# From repo root
make build CONTRACT=opticrum

# Or from this directory
make build

# Requires RISC-V target
make prepare   # rustup target add riscv64imac-unknown-none-elf
```

Output: `build/release/opticrum` (stripped RISC-V binary) and `build/release/opticrum.debug` (with debug info).

## Dependencies

- `ckb-cinnabar-verifier` — `no_std` verification primitives, macros (`cinnabar_main!`, `define_errors!`), CKB std re-exports
- `opticrum-protocol` — Canonical byte-level data layouts shared with the calculator
- Rust target: `riscv64imac-unknown-none-elf`
- RISC-V extensions: `+zba,+zbb,+zbc,+zbs`

## Related Crates

- `opticrum-protocol` (`opticrum-protocol/`) — Shared canonical data layouts (OrderArgs, OrderData, MatchArgs, MatchData, OutPoint, length constants)
- `opticrum-calculator` (`calculator/opticrum/`) — Off-chain transaction assembly and argument construction
- `opticrum-runner` (`src/`) — CLI for deploy/migrate/consume
- `opticrum-tests` (`tests/`) — Integration tests via CKB transaction simulator
