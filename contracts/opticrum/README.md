# Opticrum Contract

On-chain RISC-V verification for the Opticrum liquidity marketplace. Runs inside the
CKB-VM via [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar)'s verification tree.

## Design Philosophy

### Syscalls Are Expensive

The CKB-VM charges cycles for every syscall. Loading a cell, reading a header, checking a lock hash — each has a cost. The contract is structured to minimize redundant work:

1. **The root verifier does all I/O upfront.** It parses both input and output cells, computes unoccupied capacity, detects xUDT, and stores everything in `Context`. Branch verifiers only read pre-computed fields — they never issue syscalls for data the root already fetched.

2. **`unoccupied_capacity` is computed once, used everywhere.** The root calculates `capacity - occupied_capacity` for both input and output cells. The match verifier compares these values directly rather than re-issuing `load_cell_capacity` + `load_cell_occupied_capacity`. Two syscalls eliminated per verification.

3. **Channel lookup bails out early.** `find_channel_in_celldeps` iterates CellDeps, checks the outpoint, type script, capacity, and xUDT amount in a single pass. If any check fails, it short-circuits rather than loading the next dep.

### State Lives in the Lock Script

Discriminating Order vs Match by args length rather than a type script is a deliberate performance choice. Loading a type script costs an extra syscall. Checking `args.len()` costs nothing — it's already in memory from loading the lock. This pattern (length-based state discrimination) is common in well-optimized CKB contracts.

### Determinism Above All

Blockchain verification must be deterministic — the same inputs must produce the same result on every node. This is why `shannons_per_block` is `u64`, not floating-point. RISC-V soft-float and x86 hardware IEEE 754 can diverge on edge cases (NaN, subnormals, rounding modes). Integer arithmetic has one correct answer everywhere.

When the contract must compare `shannons_per_block` values, it uses direct `u64` equality — no tolerance, no epsilon, no ambiguity.

## Cell Model

### Order Cell (65-byte args)

```
fiber_pubkey[33] | buyer_lock_hash[32]
```

**Data** (32 bytes): `xudt_amount` (u128 LE) | `channel_capacity` (u64 LE) | `shannons_per_block` (u64 LE)

`fiber_pubkey` identifies which Fiber node the buyer wants to receive from. It's stored but never verified on-chain — Fiber's own protocol handles pubkey authentication. `buyer_lock_hash` is the on-chain identity; the verifier checks it against transaction inputs to prove the buyer authorized the cancellation.

`channel_capacity` is a **transient field**: verified at match time, discarded afterward. The Match cell does not carry it. This saves 8 bytes per Match cell and avoids carrying stale data through the lifecycle.

### Match Cell (133-byte args)

```
Order args[65] | channel_outpoint[36] | seller_lock_hash[32]
```

**Data** (32 bytes): `xudt_amount` (u128 LE) | `shannons_per_block` (u64 LE) | `last_extraction_block` (u64 LE)

The first 65 bytes are the original Order args — the buyer's identity is preserved across the transition. This is what allows the `match_update` verifier to check the buyer's authorization even though the Match cell was created by the seller.

`channel_outpoint` (36 bytes: tx_hash + index) replaces a simple lock hash. The verifier uses it to **load the actual channel cell from CellDeps** and inspect its capacity, type script, and xUDT balance. This is stronger than comparing a hash — it proves the channel exists right now, with the right properties, at the claimed location.

### Why the Match Data Is Only 32 Bytes

The Match cell carries the absolute minimum: current token balance, the immutable rent rate, and the last extraction block. Everything else is either:
- **Derivable** (rent owed = rate × elapsed — compute on demand)
- **Already in args** (channel outpoint, buyer identity, seller identity)
- **Off-chain** (escrow duration, annual yield, total pre-funded amount)

This minimalism keeps the on-chain footprint small. Every byte costs capacity.

## Verification Tree

### Root Verifier

The root is the entry point for every Opticrum transaction. It:

1. Parses the input cell's lock args by length into `Branch::Order` or `Branch::Match`
2. Computes `unoccupied_capacity` and detects xUDT
3. Parses the output cell (if one exists with the Opticrum lock)
4. Calls `OpticrumState::compare()` to determine the transition pattern

The comparison logic is a pure function on the two `OpticrumState` values. It returns one of four patterns:

```
Order + None    → OrderCancel   (Burn: consumed, no matching output)
Order + Match   → OrderMatch    (Transfer: Order args match, xUDT matches)
Match + Match   → MatchUpdate   (Transfer: Match args match, xUDT matches)
Match + None    → MatchDestroy  (Burn: consumed, no matching output)
anything else   → UnknownState  (rejected)
```

The root returns a string name (`"order_cancel"`, `"order_match"`, etc.) that `cinnabar_main!` dispatches to the matching verifier.

### order_cancel

The simplest verifier. One check: does the buyer's lock hash appear in any transaction input? If yes, the buyer authorized the cancellation and gets their funds back. If no, reject.

There's no check on *which* input — just presence. The buyer's own lock script on their input cell handles the actual signature verification. This verifier only confirms the right person is involved.

### order_match

The most complex verifier. Six sequential checks, each building on the previous:

1. **Channel exists and satisfies requirements.** `find_channel_in_celldeps` locates the CellDep matching `channel_outpoint`, confirms it has a recognized Fiber funding type ID, and checks capacity/xUDT against the order's requirements.

2. **Seller authorizes.** Their lock hash must appear in inputs.

3. **Match data is correctly initialized.** `shannons_per_block` must match the Order (byte-for-byte), and `last_extraction_block` must be 0 (no extraction has happened yet).

4. **Unoccupied capacity transfers intact.** The rent pool must not shrink or grow during Order→Match. Uses the pre-computed values from `Context` — no syscalls.

5. **xUDT amount preserved.** If the Order had tokens, the Match must have the same amount.

6. **Channel created after the Order.** Loads the Order creation header (from `Source::GroupInput`) and the channel creation header (from `Source::CellDep` at the matched index), then compares block numbers. This prevents a seller from front-running: they can't use a channel created before they saw the order.

### match_update

The most architecturally interesting verifier. It handles two entirely different operations — seller extracting rent, buyer adjusting funds — through a single code path. Why? Because both are Match→Match transitions with identical args, and the root verifier can't tell them apart from state alone.

The solution: **branch on authorization inside the verifier.**

```rust
match (seller_present, buyer_present) {
    (true, false)  → extraction path
    (false, true)  → inject/withdraw path
    _              → error (both or neither)
}
```

A key optimization: the `shannons_per_block` preservation check is hoisted **above** the branch. Both paths must preserve the rent rate, so checking it once before the split avoids duplication and makes the invariant explicit.

**Extraction path:**
- Channel still exists in CellDeps (existence only — capacity was verified at match time)
- Match not already exhausted (rent owed hasn't consumed all value)
- Extraction amount = `shannons_per_block × (tip_block - last_extraction_block)` for both CKB and xUDT
- `last_extraction_block` updated to tip block

The extraction amount check differs for CKB vs xUDT:
- **CKB**: `old_unoccupied - new_unoccupied` must equal the expected rent
- **xUDT**: `old_xudt_amount - new_xudt_amount` must equal the expected rent

Both use the same `expected_rent` calculation — the linear formula is currency-agnostic.

**Inject/withdraw path:**
- `last_extraction_block` preserved (extraction clock doesn't reset)
- For xUDT matches, type script preserved (can't change the token type)

### match_destroy

The safety valve. When accumulated rent exceeds remaining value, the match is "exhausted" — there's nothing left for the buyer. The seller can sweep the remainder.

Two checks:
1. Seller lock hash in inputs (only the seller can destroy)
2. Match is genuinely exhausted: `accumulated_rent >= remaining_capacity`

The exhaustion check uses `liquidity_rent()` which handles the first-extraction edge case: if `last_extraction_block` is 0, the base block is loaded from the match cell's creation header (`Source::GroupInput`). Otherwise, it uses the stored `last_extraction_block`.

## The Convenience Layer: `Context` Methods

The `Context` struct (in `state.rs`) provides three methods that eliminate repetitive destructuring:

```rust
ctx.expect_old_order()  → Result<(&OrderArgs, &OrderData)>
ctx.expect_old_match()  → Result<(&MatchArgs, &MatchData)>
ctx.expect_new_match()  → Result<(&MatchArgs, &MatchData)>
```

Each method returns the destructured fields or the appropriate error if the branch is wrong. These replace 4–6 line `let Branch::X(...) = &ctx.old_state.branch else { ... }` blocks with single-line calls. The error mapping is centralized: `expect_old_order` always returns `UnexpectedBranch`, `expect_new_match` returns `BadMatchUpdate` if `new_state` is `None` (correct for Transfer pattern).

## Utilities

All verifiers share helpers from `utils.rs`:

| Function | Insight |
|----------|---------|
| `load_header_block_number(index)` | Abstracts the `load_header → raw → number → unpack` chain |
| `has_input_lock(lock_hash)` | Uses `QueryIter` for efficient input scanning |
| `require_input_lock(name, hash, error)` | Combines check + debug message + error return in one call |
| `find_channel_in_celldeps(...)` | Single-pass channel lookup with optional capacity/xUDT checks |
| `check_channel_existence(outpoint)` | Lightweight boolean — no capacity/xUDT loading |
| `parse_xudt(index, source)` | Parses first 16 bytes as u128 + detects type script |
| `get_unoccupied_capacity(index, source)` | `capacity − occupied_capacity` — used by root verifier |
| `find_opticrum_script(source)` | Finds Opticrum-locked cells by code_hash matching |

The `ERR!` macro maps protocol `from_slice` errors (which return `&'static str`) into `OpticrumError` variants. It references `$crate::OpticrumError`, which is why `main.rs` re-exports the error type.

## Fiber Funding Type IDs

Three constants in `main.rs` identify Fiber channel cells by their type script hash:

| Constant | Environment |
|----------|-------------|
| `FIBER_FUNDING_TYPE_ID_TESTNET` | CKB testnet |
| `FIBER_FUNDING_TYPE_ID_MAINNET` | CKB mainnet |
| `FIBER_FUNDING_TYPE_ID_MOCK` | Integration tests (`code_hash=[0xCC;32]`, `hash_type=Data1`) |

`is_fiber_funding_contract()` matches against all three. The mock ID is the blake2b-256 hash of the test fixture's type script — it must stay in sync with `tests/src/faker.rs`.

## Error Reference

All errors via `define_errors!` starting from `CUSTOM_ERROR_START` (20):

| Code | Variant | When |
|------|---------|------|
| 20 | `BadOrderCancel` | Cancel verification failed |
| 21 | `BadOrderMatch` | Match verification failed |
| 22 | `ChannelCellNotInDep` | Channel not found or wrong funding type |
| 23 | `ChannelCapacityMismatch` | Rent pool changed during Order→Match |
| 24 | `ChannelCreatedBeforeOrder` | Channel predates Order (front-running) |
| 25 | `OrderDataNotSet` | Order data missing |
| 26 | `BadXudtAmount` | xUDT amount changed Order→Match |
| 27 | `BadExtractionAmount` | Wrong extraction amount |
| 28 | `MatchDataNotSet` | Match data missing |
| 29 | `HeaderNotSet` | Required header dep not provided |
| 30 | `BadMatchDataUpdate` | Fields changed incorrectly |
| 31 | `BadMatchUpdate` | Update verification failed |
| 32 | `MatchNotExhausted` | Destroy before exhaustion |
| 33 | `RentPerBlockMismatch` | Rate changed during update |
| 35 | `BadArgsLength` | Args not 65 or 133 bytes |
| 36 | `BuyerAuthMissing` | Buyer not in inputs |
| 37 | `SellerAuthMissing` | Seller not in inputs |
| 38 | `AuthorizationMissing` | Neither buyer nor seller |
| 39 | `UnexpectedBranch` | Wrong branch for transition |
| 40 | `UnknownState` | Can't determine transition |

Code 34 (`MatchNotViable`) is reserved — removing it would shift all subsequent codes and break deployed contracts.

## Build

```bash
make build          # RISC-V binary → build/release/opticrum
make prepare        # rustup target add riscv64imac-unknown-none-elf
```

Cross-compiles to `riscv64imac-unknown-none-elf` with B-extension features (`+zba,+zbb,+zbc,+zbs`).
Stripped via `llvm-objcopy`.
