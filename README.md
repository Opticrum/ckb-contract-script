# Opticrum

A decentralized liquidity marketplace for the [Fiber Network](https://github.com/nervosnetwork/fiber) on [CKB](https://github.com/nervosnetwork/ckb). Fully decentralized version of [Amboss](https://amboss.tech/).

Built with [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar).

## Motivation

Lightning Network liquidity markets like Amboss connect node operators who need inbound capacity with capital providers who earn yield. But they rely on a trusted intermediary: Amboss holds the funds, matches parties, and enforces terms. If Amboss disappears, goes rogue, or gets hacked, both sides lose their money.

Opticrum replaces the intermediary with a RISC-V contract running inside the CKB-VM. Every step — creating an order, matching with a channel, extracting rent, topping up, withdrawing — is enforced on-chain by a deterministic program. No oracle, no admin key, no trusted server. The contract **is** the marketplace.

## The Core Insight: Linear Rent Without Proportional Math

The hardest part of on-chain rent distribution is computing proportional shares. If a seller is owed *fraction* of the total rent based on elapsed time, you need division — which is expensive and imprecise on a VM.

Opticrum sidesteps this entirely. Instead of storing a "total rent" and computing a fraction each extraction, the buyer specifies a **per-block rate** (`shannons_per_block`) directly in the Order. Extraction becomes a single multiplication:

```
extractable = shannons_per_block × (tip_block - last_extraction_block)
```

No division. No floating-point. One `u64 × u64` multiply. The entire rent curve is encoded in that one constant.

This also means the on-chain contract never needs to know the "total escrow duration" — that's an off-chain concept. The buyer pre-funds however much capacity they want, and the seller extracts at the agreed rate until it runs out. If the buyer wants to extend the match, they inject more capacity. If they want out early, they withdraw what remains.

## Lifecycle

```
Order Cell (buyer creates, 65-byte lock args + 32-byte data)
    ├── Cancel  → buyer reclaims (Burn pattern)
    └── Match   → seller matches with pre-created channel, produces Match Cell
                  (Transfer pattern, no status — immediately active)
                      ├── Extract Rent → seller periodically withdraws linear rent
                      │                  (Transfer pattern, updates last_extraction_block)
                      ├── Inject/Withdraw → buyer adds or removes funds from Match
                      │                  (Transfer pattern, preserves shannons_per_block)
                      └── Destroy      → seller sweeps when exhausted (Burn pattern)
```

### Why No Status Machine?

Most escrow contracts have a status field: Frozen → Active → Expired. Opticrum has none. After matching, the Match cell is **immediately active**. The seller can extract right away; the buyer can withdraw funds at any time. There's no review phase because the seller already committed a real Fiber channel — the channel outpoint in Match args proves they have skin in the game.

The buyer's protection isn't a "dispute window." It's the ability to **withdraw their money**. If the seller never provides the channel or the terms aren't honored, the buyer empties the Match cell and walks away. This is simpler, cheaper, and more trustless than any dispute resolution mechanism.

## Cell Architecture

Two states discriminated purely by lock script `args` length — no type script, no enum tag:

| State | Args Length | Composition |
|-------|-------------|-------------|
| Order | 65 bytes | `fiber_pubkey` (33) + `buyer_lock_hash` (32) |
| Match | 133 bytes | Order args (65) + `channel_outpoint` (36) + `seller_lock_hash` (32) |

Length-based discrimination is a deliberate choice. The CKB-VM charges per byte loaded, so avoiding a separate type script saves syscalls and cycles. It also means the lock script itself is the sole authority on cell identity — there's no coordination between lock and type to get wrong.

### Order Cell Data (32 bytes)

| Field | Bytes | Type | Purpose |
|-------|-------|------|---------|
| `xudt_amount` | 16 | u128 | Token amount (0 for CKB-only) |
| `channel_capacity` | 8 | u64 | Minimum seller channel size |
| `shannons_per_block` | 8 | u64 | Rent rate, specified by buyer |

`channel_capacity` is a **verify-then-discard** field. The match verifier loads the real channel CellDep, checks its capacity against this value, and then does not carry it into the Match cell. It served its purpose at match time — storing it longer would waste on-chain bytes.

### Match Cell Data (32 bytes)

| Field | Bytes | Type | Purpose |
|-------|-------|------|---------|
| `xudt_amount` | 16 | u128 | Remaining token amount |
| `shannons_per_block` | 8 | u64 | Copied from Order, never changes |
| `last_extraction_block` | 8 | u64 | Last block when rent was withdrawn |

### Why u64 and Not f64?

Earlier versions used IEEE 754 `f64` for `rent_per_block`. We switched to `u64` for one reason: **determinism across floating-point implementations**. Hardware x86 FPUs and RISC-V soft-float can produce different results for the same bit pattern. In a blockchain context, that means a transaction valid on one node could be invalid on another — a consensus fork.

`u64` integer arithmetic is identical everywhere. No edge cases, no NaN, no rounding modes. The precision cost is negligible: at CKB's 10⁸ shannon decimal places, a `u64` can express rates from 1 shannon/block up to ~18 million CKB/block.

## Channel Verification: OutPoint, Not MuSig2

Fiber Network creates every channel with a per-channel generated seed. This means the channel's funding lock script does **not** derive from the buyer's and seller's public keys — MuSig2 key aggregation can't verify channel ownership.

Opticrum verifies channel identity through a stronger mechanism: the **outpoint**. Match args store the full 36-byte channel outpoint (tx_hash + index). The on-chain verifier loads the channel cell from CellDeps, confirms it has the correct Fiber funding type script, and checks its capacity. This is more robust than comparing a hash — it proves not just that the channel *exists*, but that it has the exact capacity the buyer demanded.

## Economic Model

### Yield Convention

The protocol only knows `shannons_per_block` — a raw integer. But humans think in annual percentage yield. Two off-chain helpers bridge the gap:

```rust
/// 5% annual yield → shannons_per_block
let rent_per_block = annual_yield_to_rent_per_block(channel_capacity, 500);

/// Pre-fund ~10 days of rent
let rent_capacity = rent_per_block.saturating_mul(100_000);
```

`BLOCKS_PER_YEAR ≈ 2,629,800` assumes a ~12-second CKB block interval. The formula:

```
shannons_per_block = channel_capacity × yield_bps / (10_000 × blocks_per_year)
```

These helpers are pure off-chain conveniences. The contract has no concept of "annual yield" or "escrow duration" — it only sees the per-block integer.

### The Reserve Trick

Match cells are 68 bytes larger than Order cells (133 − 65 args + 32 − 32 data). On CKB, each occupied byte costs 1 CKB in capacity. So the Match cell needs 68 extra CKB just to exist.

Rather than requiring the seller to inject 68 CKB at match time (which would be a terrible UX — the seller is providing a service, why should they pay?), the buyer pre-funds this reserve on the Order cell. The constant `ORDER_TO_MATCH_CAPACITY_RESERVE = 68 CKB` is baked into the calculator and added to every Order's capacity. When the Order→Match transition happens with `CapacityAdjustment::Keep`, the reserve flows through automatically.

This is possible because CKB's capacity model separates "total capacity" (the cell's CKB balance) from "occupied capacity" (the bytes the cell uses). The unoccupied portion is free for any use — in our case, it becomes the rent pool.

## On-Chain Verification

### The Root Verifier Pattern

All five verifiers share a common `Context` populated by the root verifier. The root does all I/O upfront — parsing args, loading capacity, detecting xUDT — and stores the results. Branch verifiers only compare pre-computed values. This avoids redundant syscalls (which cost VM cycles) and keeps each verifier focused on its specific rules.

The root verifier dispatches by args-length comparison:

```
Root (runs first, populates Context)
├── Order(65) + None           → "order_cancel"   (Burn)
├── Order(65) + Match(133)     → "order_match"    (Transfer)
├── Match(133) + Match(133)    → "match_update"   (Transfer)
└── Match(133) + None          → "match_destroy"  (Burn)
```

### match_update: One Verifier, Two Paths

`match_update` handles both seller extraction and buyer inject/withdraw — two very different operations that share the same state transition pattern (Match→Match). Since the root can't distinguish them by args length alone, `match_update` internally branches on authorization:

- **seller_lock_hash in inputs** → extraction path (withdraw rent, advance `last_extraction_block`)
- **buyer_lock_hash in inputs** → inject/withdraw path (adjust capacity, preserve data fields)
- **neither/both** → error

A shared check — `shannons_per_block` must not change — is hoisted above the branch because it applies to both paths. This is a small optimization that also serves as documentation: "this invariant always holds, regardless of who's acting."

## Development

### Prerequisites

- **Rust** — nightly or stable with `riscv64imac-unknown-none-elf` target
- **Clang** — for CKB-VM syscall linking (the build script auto-discovers via `scripts/find_clang`)
- **llvm-objcopy** — strips debug symbols from the RISC-V binary (part of the LLVM toolchain)
- **[ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar)** — must be cloned as a sibling of this repo

Your directory layout should look like:

```
~/Freelancer/fiber/
├── opticrum/          # this repo
└── ckb-cinnabar/      # framework dependency
```

### Setup

```bash
# 1. Install the RISC-V target
make prepare
# Equivalent to: rustup target add riscv64imac-unknown-none-elf

# 2. Verify the toolchain is available
rustup target list | grep riscv64imac

# 3. Check clang is discoverable
./scripts/find_clang
```

### Build

```bash
# Compile just the RISC-V contract (fastest for iteration)
make build CONTRACT=opticrum
# → build/release/opticrum

# Build everything (contract + calculator + CLI binaries)
cargo build -p runner -p opticrum-calculator

# Build with verbose output to debug syscall issues
make build CONTRACT=opticrum CARGO_ARGS="--verbose"
```

The contract cross-compiles to `riscv64imac-unknown-none-elf` with B-extension features (`+zba,+zbb,+zbc,+zbs`) and is stripped via `llvm-objcopy --strip-debug`.

### Test

```bash
# Full integration test suite (12 tests, requires compiled contract binary)
make build CONTRACT=opticrum && cargo test -p opticrum-tests -- --nocapture

# Run a single test
cargo test -p opticrum-tests -- test_match_order --nocapture

# Sequential execution (avoids test interference from shared RPC state)
cargo test -p opticrum-tests -- --nocapture --test-threads=1
```

The integration tests use ckb-cinnabar's `TransactionSimulator` which spawns the real RISC-V binary in a CKB-VM instance. No mocked contract — tests verify the exact binary that would be deployed on-chain.

Tests seed fake cells, headers, and channels via `FakeRpcClient`, then execute calculator-built instructions and assert VM verification results. The test faker in `tests/src/faker.rs` must stay in sync with `FIBER_FUNDING_TYPE_ID_MOCK` in the contract.

### Validity Checks

```bash
# Type-check all crates
make check
# Equivalent to: cargo check --workspace

# Lint
make clippy
# Equivalent to: cargo clippy --workspace

# Format
make fmt
# Equivalent to: cargo fmt --all

# Run all three in sequence
make check && make clippy && make fmt
```

These are the gates before committing. CI would run the same sequence.

### Reproducible Build

```bash
# Build inside Docker for a byte-identical binary
./scripts/reproducible_build_docker

# Verify the checksum matches a known-good build
make checksum
# → shasum -a 256 build/release/*
```

The Docker build pins the Rust toolchain, clang version, and OS image to ensure the output binary is bit-for-bit reproducible. This is critical for on-chain deployment — anyone can verify the deployed contract matches the source.

### Project Layout

```
opticrum/
├── contracts/opticrum/       # On-chain RISC-V verification (no_std)
│   ├── src/main.rs           # Entry: cinnabar_main! + funding type IDs
│   ├── src/state.rs          # Branch, OpticrumState, Context + convenience methods
│   ├── src/verifiers/        # Five verifiers: root, order_cancel, order_match,
│   │                         #   match_update, match_destroy
│   ├── src/utils.rs          # Shared helpers (auth, headers, channels, xUDT)
│   └── src/error.rs          # OpticrumError (20 variants)
├── calculator/opticrum/      # Off-chain transaction assembly
│   ├── src/calculator.rs     # Six instruction builders + yield helpers
│   ├── src/reader.rs         # scan_orders, scan_matches (shared generic scan_cells)
│   ├── src/config.rs         # CKB_DECIMAL, BLOCKS_PER_YEAR, type_id, reserve
│   └── src/types.rs          # Xudt, OrderInfo, MatchInfo + protocol re-exports
├── opticrum-protocol/        # Canonical types shared on/off chain (no_std)
├── src/bin/                  # Seven CLI binaries
│   ├── create_order.rs       # Buyer creates Order
│   ├── match_order.rs        # Seller matches Order with channel
│   ├── extract_liquidity_rent.rs  # Seller extracts vested rent
│   ├── topup_rent.rs         # Buyer injects more capacity
│   ├── decline_rent.rs       # Buyer withdraws capacity
│   ├── scan_orders.rs        # List live Orders
│   └── scan_matches.rs       # List live Matches
├── tests/                    # Integration tests (CKB simulator + FakeRpcClient)
│   └── src/
│       ├── integration.rs    # 12 full-lifecycle tests
│       └── faker.rs          # Cell seeding, constants, helpers
├── scripts/                  # find_clang, reproducible_build_docker
├── deployment/               # On-chain deployment records (testnet)
└── Makefile                  # Top-level build orchestration
```

## CLI Binaries

| Binary | Actor | Signing | Purpose |
|--------|-------|---------|---------|
| `create_order` | Buyer | ckb-cli | Create Order cell |
| `match_order` | Seller | privkey | Match Order with channel |
| `extract_liquidity_rent` | Seller | privkey | Withdraw vested rent |
| `topup_rent` | Buyer | privkey | Inject more rent capacity |
| `decline_rent` | Buyer | privkey | Withdraw rent capacity |
| `scan_orders` | — | — | List live Orders |
| `scan_matches` | — | — | List live Matches |

## Type Reference

| Type | Fields | Size |
|------|--------|------|
| `OrderArgs` | `fiber_pubkey` (CompressedPubkey), `buyer_lock_hash` ([u8; 32]) | 65 |
| `OrderData` | `xudt_amount` (u128), `channel_capacity` (u64), `shannons_per_block` (u64) | 32 |
| `MatchArgs` | `order_args` (OrderArgs), `channel_outpoint` (OutPoint), `seller_lock_hash` ([u8; 32]) | 133 |
| `MatchData` | `xudt_amount` (u128), `shannons_per_block` (u64), `last_extraction_block` (u64) | 32 |
| `OutPoint` | `tx_hash` ([u8; 32]), `index` (u32) | 36 |
| `OrderInfo` | `order_args`, `order_data`, `xudt`?, `ckb_capacity`, `order_outpoint` | — |
| `MatchInfo` | `match_args`, `match_data`, `xudt`?, `ckb_capacity`, `match_outpoint`, `match_current_block` | — |

## Error Codes

All errors start from `CUSTOM_ERROR_START` (20) with sequential offsets:

| Offset | Variant | Trigger |
|--------|---------|---------|
| +0 | `BadOrderCancel` | Cancel verification failed |
| +1 | `BadOrderMatch` | Match verification failed |
| +2 | `ChannelCellNotInDep` | Channel CellDep missing or wrong type |
| +3 | `ChannelCapacityMismatch` | Unoccupied capacity changed Order→Match |
| +4 | `ChannelCreatedBeforeOrder` | Channel block ≤ Order block |
| +5 | `OrderDataNotSet` | Order data missing/malformed |
| +6 | `BadXudtAmount` | xUDT amount changed Order→Match |
| +7 | `BadExtractionAmount` | Extracted ≠ rate × elapsed |
| +8 | `MatchDataNotSet` | Match data missing/malformed |
| +9 | `HeaderNotSet` | Required header dep missing |
| +10 | `BadMatchDataUpdate` | Match data fields changed incorrectly |
| +11 | `BadMatchUpdate` | Update verification failed |
| +12 | `MatchNotExhausted` | Destroy before exhaustion |
| +13 | `RentPerBlockMismatch` | Rate changed during update |
| +15 | `BadArgsLength` | Args not 65 or 133 bytes |
| +16 | `BuyerAuthMissing` | Buyer lock not in inputs |
| +17 | `SellerAuthMissing` | Seller lock not in inputs |
| +18 | `AuthorizationMissing` | Neither party authorized |
| +19 | `UnexpectedBranch` | Wrong branch for transition |
| +20 | `UnknownState` | Can't determine transition |

Offset +14 (`MatchNotViable`) is reserved to preserve error code indices.

## Encoding Conventions

- All integers: little-endian
- Capacity values: shannons (1 CKB = 10⁸ shannons)
- `shannons_per_block`: u64, direct integer comparison is deterministic everywhere
- `CKB_DECIMAL = 100_000_000`
- `BLOCKS_PER_YEAR ≈ 2,629,800` (~12s block interval)
- `ORDER_TO_MATCH_CAPACITY_RESERVE = 68 CKB`
