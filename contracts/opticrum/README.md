# Opticrum Contract

On-chain RISC-V verification for the Opticrum liquidity marketplace. Runs inside the
CKB-VM via [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar)'s verification tree.

## Cell Model

Two cell states discriminated by lock script `args` length. No type script is used — the
lock itself carries all identity and authorization data.

### Order Cell (65-byte args)

```
fiber_pubkey[33] | buyer_lock_hash[32]
```

**Cell data** (32 bytes): `xudt_amount` (u128 LE) | `channel_capacity` (u64 LE) | `escrow_blocks` (u64 LE)

`channel_capacity` is the minimum the seller's channel must have. It is verified at match
time from the real channel CellDep and then discarded — the Match cell does not carry it.
`escrow_blocks` defines the vesting duration.

### Match Cell (166-byte args)

```
Order args[65] | channel_outpoint[36] | seller_lock_hash[32] | seller_fiber_pubkey[33]
```

The first 65 bytes are the original Order args (buyer identity). The seller appends their
own `fiber_pubkey` and the channel outpoint, enabling on-chain MuSig2 verification.

**Cell data** (40 bytes): `xudt_amount` (u128 LE) | `rent_per_block` (f64 LE) | `escrow_blocks` (u64 LE) | `last_extraction_block` (u64 LE)

`rent_per_block` is pre-computed at match time as `total_rent / escrow_blocks` and never
changes. It is intentionally not compared during extraction updates — f64 equality is
unreliable across hardware FPU vs RISC-V soft-float. `escrow_blocks` is stored so expiry
can be computed without loading the original Order cell.

## Verification Tree

The `Root` verifier inspects `args` length and `ScriptPattern` (how the cell is consumed)
to route to the correct branch:

```
Root
├── args_len == 65 (Order)
│   ├── Burn     → order_cancel
│   └── Transfer → order_match
└── args_len == 166 (Match)
    ├── Transfer → match_extract
    └── Burn     → match_destroy
```

**Burn**: cell consumed as input, no matching Opticrum output.
**Transfer**: cell appears in both inputs and outputs with matching Opticrum lock.
**Create**: cell only in outputs — the lock does not execute; creation is unchecked.

## Operations

### Cancel Order

Buyer reclaims an unmatched Order cell. The verifier checks that the buyer's lock hash
(from Order args) appears in the transaction inputs — only the original buyer can cancel.

### Match Order

Seller matches an Order by referencing a pre-created Fiber channel as a CellDep (the
channel is NOT consumed). The verifier enforces:

1. **Channel exists and satisfies the order.** A CellDep must match `channel_outpoint`,
   have a recognized Fiber funding type ID, and have sufficient capacity (and xUDT amount,
   for token orders).

2. **Channel is bound to both parties.** Fiber channels are funded by a 2-of-2 MuSig2 key.
   The contract recomputes the aggregated x-only key from the buyer's `fiber_pubkey` (in
   Order args) and the seller's `fiber_pubkey` (in Match args), hashes it with blake2b-256,
   and compares the first 20 bytes against the channel's lock args. This proves the channel
   was created for exactly this buyer-seller pair.

3. **Seller authorizes.** The seller's lock hash must appear in transaction inputs.

4. **State integrity.** Match data is correctly initialized (`rent_per_block > 0`,
   `escrow_blocks > 0`, `last_extraction_block == 0`), capacity transfers intact, and
   xUDT amount is preserved.

### Extract Rent

Seller withdraws linearly-vested rent. The verifier checks:

1. Channel cell still exists in CellDeps (existence only — capacity was verified at match).
2. Seller authorizes.
3. Extraction amount equals `rent_per_block × (tip_block - last_extraction_block)`.
4. Only `last_extraction_block` changes; all other data fields and args stay the same.
5. Output cell remains viable (capacity ≥ occupied).
6. On first extraction, a HeaderDep at the match creation block proves the match's age.

If `accumulated_rent ≥ remaining_capacity`, the match is **exhausted** — extraction
delegates to destroy internally.

### Destroy Match

After exhaustion or expiry, the seller or buyer can sweep remaining funds. The verifier
checks that the match is genuinely exhausted and that one of the authorized parties signs.
This is the safety valve — no funds can be permanently locked.

## MuSig2 Key Aggregation

The aggregated key is computed on-chain by calling into a **C function** compiled from
Bitcoin Core's libsecp256k1. The algorithm is BIP-327 MuSig2\* for 2-of-2:

1. Sort the two 33-byte compressed pubkeys ascending (Fiber's deterministic order).
2. `L = tagged_hash("KeyAgg list", pk1 || pk2)`
3. `a1 = int(tagged_hash("KeyAgg coefficient", L || pk1)) mod n`
4. `Q = a1·P1 + P2` (second distinct key gets coefficient 1)
5. Extract the 32-byte x-coordinate, hash, compare first 20 bytes.

The 1 MB secp256k1 pre-context table (needed for efficient EC multiplication) is **not**
embedded in the binary. It is loaded at runtime from a shared CKB CellDep, identified by
its blake2b data hash. This keeps the contract binary small while retaining full on-chain
verification.

The FFI is exposed through the `secp256k1` crate (`secp256k1/src/lib.rs`) via a single
function:

```rust
pub fn compute_musig2_key_aggregation_xonly(
    pk_a: &[u8; 33],
    pk_b: &[u8; 33],
) -> Result<[u8; 32], i32>
```

## Fiber Funding Type IDs

The contract recognizes channel cells by their type script hash:

| Constant | Purpose |
|----------|---------|
| `FIBER_FUNDING_TYPE_ID_MAINNET` | Production Fiber funding cells |
| `FIBER_FUNDING_TYPE_ID_TESTNET` | Testnet Fiber funding cells |
| `FIBER_FUNDING_TYPE_ID_MOCK` | Integration-test mock (`code_hash=[0xCC;32]`, `hash_type=Data1`) |

## Error Codes

| Error | Trigger |
|-------|---------|
| `ChannelCellNotInDep` | Channel CellDep not found or wrong funding type |
| `ChannelFundingPubkeyMismatch` | Lock args ≠ blake160(aggregated buyer+seller key) |
| `ChannelCapacityMismatch` | Order → Match capacity changed |
| `BadExtractionAmount` | Extracted ≠ rent_per_block × elapsed |
| `MatchAlreadyExhausted` | Extract on exhausted match |
| `MatchNotExhausted` | Destroy before exhaustion |
| `BuyerAuthMissing` / `SellerAuthMissing` | Required signer not in inputs |
| `BadArgsLength` | Lock args not 65 or 166 bytes |

## Build

```bash
make build          # RISC-V binary → build/release/opticrum
make prepare        # rustup target add riscv64imac-unknown-none-elf
```

Requires `riscv64imac-unknown-none-elf` target and a RISC-V GCC toolchain for the C
secp256k1 library. The pre-context data is built separately in `secp256k1/ckb-lib-secp256k1/`.
