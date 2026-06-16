# Opticrum Contract

On-chain verification contract for the Opticrum decentralized liquidity marketplace. Runs on CKB's RISC-V VM via the [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) verification framework.

## Overview

This contract enforces the rules of Opticrum's liquidity marketplace on-chain. Each transaction that touches an Opticrum Cell must satisfy the verification logic appropriate to its state (Order vs Match) and the operation being performed.

**No economic logic lives on-chain** — the contract only verifies structural correctness and authorization. Rent math is checked here to prevent theft, but the actual rent amount is determined off-chain by the calculator crate.

## Cell Model

Two Cell states, discriminated by lock script `args` length:

### Order Cell (68-byte args)

| Field | Offset | Size | Type |
|-------|--------|------|------|
| Fiber Pubkey | 0 | 32 | bytes |
| Buyer Pubkey Hash | 32 | 20 | bytes |
| Channel Capacity | 52 | 8 | u64 LE |
| Escrow Blocks | 60 | 8 | u64 LE |

### Match Cell (120-byte args)

| Field | Offset | Size | Type |
|-------|--------|------|------|
| *(Order fields)* | 0 | 68 | *(same as above)* |
| Channel Lock Hash | 68 | 32 | bytes |
| Seller Pubkey Hash | 100 | 20 | bytes |

### Match Cell Data (32 bytes)

| Field | Offset | Size | Type |
|-------|--------|------|------|
| xUDT Amount | 0 | 16 | u128 LE |
| Match Created Block | 16 | 8 | u64 LE |
| Last Extraction Block | 24 | 8 | u64 LE |

## Verification Tree

The `Root` verifier inspects `args` length and `ScriptPattern` to route to the correct branch:

```
Root
├── args_len == 68 (Order)
│   ├── Burn     → OrderCancel
│   └── Transfer → OrderMatch
└── args_len == 120 (Match)
    ├── Transfer → MatchExtract
    └── Burn     → MatchDestroy
```

`ScriptPattern` is determined by how the Cell appears in the transaction:
- **Burn** — Cell in inputs but not outputs (being consumed/destroyed)
- **Transfer** — Cell in both inputs and outputs (being updated)
- **Create** — Cell only in outputs (rejected: lock doesn't execute on creation)

## Operations

### 1. Cancel Order (`OrderCancel`)

Buyer reclaims an unmatched Order Cell.

**Checks:**
- Buyer's lock appears in transaction inputs (proves buyer authorized the cancel)
- Script pattern is Burn

### 2. Match Order (`OrderMatch`)

Seller matches an Order by referencing a pre-created Fiber channel.

**Checks:**
1. Channel Cell exists in CellDeps with sufficient capacity (≥ `channel_capacity`)
2. Match Cell args correctly extend Order args (first 68 bytes must match)
3. Match Cell data initialized: `match_created_block > 0`, `last_extraction_block == 0`
4. Match Cell capacity equals Order Cell capacity (rent transferred intact)
5. Channel Lock Hash in Match args matches the actual Channel Cell's lock hash

### 3. Extract Rent (`MatchExtract`)

Seller withdraws proportionally vested rent from a Match Cell.

**Before expiry:** Rent is proportional to elapsed blocks — `extractable = remaining * (elapsed / remaining_at_last_extraction)`.

**After expiry:** Seller can extract all remaining capacity.

**Checks:**
1. Seller's lock appears in transaction inputs
2. Exactly 1 Match Cell input and 1 Match Cell output
3. Extraction amount ≤ computed extractable (1-shannon tolerance for rounding)
4. Match Cell data updated correctly (only `last_extraction_block` changes to `tip_block`)
5. Match Cell args unchanged between input and output
6. HeaderDeps provided: index 0 = channel creation header, index 1 = tip header

### 4. Destroy Match (`MatchDestroy`)

After escrow expires, anyone can destroy the Match Cell and sweep remaining capacity. No authorization check — economic incentive ensures sellers extract regularly.

**Checks:**
- `tip_block >= match_created_block + escrow_blocks` (expiry condition met)

## Error Codes

| Error | Description |
|-------|-------------|
| `BadOrderCancel` | Order cancellation validation failed |
| `BadOrderMatch` | Order matching validation failed |
| `ChannelCellNotInDep` | Required Channel Cell not found in CellDeps |
| `ChannelCapacityMismatch` | Channel Cell capacity insufficient |
| `ChannelLockHashMismatch` | Channel lock hash doesn't match |
| `BadMatchDataInit` | Match Cell data incorrectly initialized |
| `BadExtractionAmount` | Rent extraction amount exceeds allowed |
| `HeaderNotSet` | Required header dependency missing |
| `BadMatchDataUpdate` | Match Cell data update invalid |
| `MatchAlreadyExpired` | Attempt to destroy before expiry |
| `BadArgsLength` | Lock args wrong length (not 68 or 120) |
| `BadScriptPattern` | Unexpected ScriptPattern for this state |
| `UnknownState` | Unrecognized cell state |
| `SellerAuthMissing` | Seller not found in transaction inputs |
| `BuyerAuthMissing` | Buyer not found in transaction inputs |

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
- Rust target: `riscv64imac-unknown-none-elf`
- RISC-V extensions: `+zba,+zbb,+zbc,+zbs`

## Related Crates

- `opticrum-calculator` (`calculator/opticrum/`) — Off-chain transaction assembly and argument construction
- `opticrum-runner` (`src/`) — CLI for deploy/migrate/consume
- `opticrum-tests` (`tests/`) — Integration tests via CKB transaction simulator
