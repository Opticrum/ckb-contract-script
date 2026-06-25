# Opticrum

A decentralized liquidity marketplace for the [Fiber Network](https://github.com/nervosnetwork/fiber) on [CKB](https://github.com/nervosnetwork/ckb). Fully decentralized version of [Amboss](https://amboss.tech/).

Built with [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar).

## How It Works

Opticrum connects two parties through on-chain escrow cells:

- **Buyers** lock CKB (or xUDT) into an Order Cell, offering rent for inbound Fiber channel liquidity.
- **Sellers** already have a pre-created 2-of-2 Fiber channel. They match an order by referencing their channel and locking the rent into a Match Cell.
- **Sellers** withdraw rent linearly over the escrow period. If the rent pool is exhausted, the match is destroyed.

The entire lifecycle is enforced by a RISC-V contract running inside the CKB-VM — no trusted third party, no oracle, no off-chain server.

### Lifecycle

```
                    ┌─ Cancel → buyer reclaims
Order Cell ─────────┤
                    └─ Match  → Match Cell ─┬─ Extract → seller withdraws rent
                                            └─ Destroy → expired, sweep remainder
```

### Cell State Discrimination

The contract identifies cell type purely by lock script `args` length:

| State | Args | Identifies |
|-------|------|-----------|
| Order | 65 bytes | `fiber_pubkey` (33) + `buyer_lock_hash` (32) |
| Match | 166 bytes | Order args (65) + `channel_outpoint` (36) + `seller_lock_hash` (32) + seller `fiber_pubkey` (33) |

Both states carry 32–40 bytes of cell data encoding the economic parameters (amounts, rent rate, escrow duration).

## Economic Model

### Order Creation

The buyer specifies three parameters in the Order cell data:

- **xUDT amount** — tokens to pay as rent (0 for CKB-denominated orders)
- **Channel capacity** — minimum capacity the seller's channel must have
- **Escrow blocks** — how long the rent vests

The buyer also pre-funds the order with the total rent amount. An `AnnualYield` percentage converts the escrow duration into a rent capacity that the buyer must provide upfront.

### Linear Rent

Rent vests linearly from the moment of matching. The key insight: instead of storing a "remaining at last extraction" numerator and computing a proportional share each time, Opticrum pre-computes a **rent per block** at match time:

```
rent_per_block = total_rent / escrow_blocks
```

Extraction then becomes a single multiplication:

```
extractable = rent_per_block × (tip_block - last_extraction_block)
```

When `accumulated_rent ≥ remaining_capacity`, the match is **exhausted** — all remaining funds go to the seller and the Match cell is destroyed.

This design eliminates proportional arithmetic from on-chain verification, replacing it with one `f64 × u64` multiply.

## On-Chain Verification

### create_order / cancel_order

Order creation is unchecked on-chain (the lock is passive on `ScriptPattern::Create`). Cancellation checks that the consuming input is signed by `buyer_lock_hash` from the Order args — only the buyer can cancel.

### match_order

When a seller matches, the contract verifies:

1. **Channel exists** — a CellDep matches `channel_outpoint` and has a Fiber funding type ID. Its capacity (and xUDT amount, for token orders) satisfies the order's requirements.

2. **MuSig2 key binds both parties** — Fiber channels are funded by a 2-of-2 MuSig2 aggregated key. The channel's lock args store `blake160(x_only_aggregated_key)`. At match time the contract recomputes the x-only aggregated key from the buyer's pubkey (in Order args) and the seller's pubkey (in Match args), hashes it, and compares against the channel lock. This proves the referenced channel was created for exactly these two parties.

3. **Seller authorizes** — the seller's lock hash appears in transaction inputs.

4. **State integrity** — Match cell data is correctly initialized, capacity transfers intact, and xUDT amount is preserved.

### extract_rent

The seller periodically withdraws vested rent:

1. Channel cell still exists in CellDeps (existence only — capacity was verified at match time).
2. Seller authorizes the transaction.
3. Extraction amount matches `rent_per_block × (tip_block - last_extraction_block)`.
4. Match cell data is updated (only `last_extraction_block` advances).
5. Output cell remains viable (capacity ≥ occupied).
6. On first extraction, a HeaderDep at the match creation block is required to prove the match's age.

If the accumulated rent exhausts the remaining capacity, extraction delegates to destroy internally.

### destroy_match

After expiry or exhaustion, anyone can sweep the remainder. The verifier checks that either the buyer or seller authorizes, and that the match is genuinely exhausted (accumulated rent ≥ remaining capacity).

## MuSig2 Key Aggregation

Fiber creates every channel with a 2-of-2 MuSig2 multisig. The individual party keys are never stored on-chain — only `blake160(x_only_aggregated_pubkey)` appears in the channel lock args (and the full x-only key in the witness).

To verify a channel belongs to a specific buyer-seller pair, Opticrum recomputes the aggregated key from the two compressed pubkeys using BIP-327 MuSig2\*:

1. Sort the two 33-byte compressed pubkeys ascending (Fiber's deterministic ordering).
2. Compute `L = tagged_hash("KeyAgg list", pk1 || pk2)`.
3. Compute coefficient `a1 = int(tagged_hash("KeyAgg coefficient", L || pk1)) mod n`.
4. Aggregate: `Q = a1·P1 + P2` (second distinct key gets coefficient 1).
5. Extract the 32-byte x-coordinate, hash with blake2b-256, and compare the first 20 bytes against the channel lock args.

The algorithm is implemented in **C** using Bitcoin Core's libsecp256k1 and compiled into the RISC-V binary. The 1 MB secp256k1 pre-context table (needed for efficient EC multiplication) is loaded from a shared CKB CellDep at runtime rather than embedded in the contract binary — keeping the on-chain footprint minimal.

## CKB-VM Integration

Opticrum uses ckb-cinnabar's verification tree pattern. The `Root` verifier inspects `args_len` and `ScriptPattern` (how the cell is consumed) to route to the correct branch:

```
Root
├── args_len == 65 (Order)
│   ├── Burn     → order_cancel   (consumed, no matching output)
│   └── Transfer → order_match    (consumed, Match output produced)
└── args_len == 166 (Match)
    ├── Transfer → match_extract  (consumed, updated Match output produced)
    └── Burn     → match_destroy  (consumed, no matching output)
```

All verifiers run inside the CKB-VM (RISC-V, `no_std`). They access on-chain state exclusively through CKB syscalls: loading cells, headers, witnesses, and transaction data.

## Build & Test

```bash
make build          # Compile RISC-V contract → build/release/opticrum
make test           # Integration tests (CKB transaction simulator)
make check          # cargo check
make clippy         # cargo clippy
make fmt            # cargo fmt
```

**Dependencies**: [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) (sibling repo), Rust nightly for `riscv64imac-unknown-none-elf`, and a RISC-V GCC toolchain for the C secp256k1 library.

## Error Reference

| Error | Trigger |
|-------|---------|
| `ChannelCellNotInDep` | Channel CellDep not found or wrong funding type |
| `ChannelFundingPubkeyMismatch` | Channel lock args ≠ blake160(aggregated buyer+seller key) |
| `ChannelCapacityMismatch` | Order → Match capacity changed |
| `BadExtractionAmount` | Extracted amount ≠ rent_per_block × elapsed |
| `MatchAlreadyExhausted` | Extract called on exhausted match |
| `MatchNotExhausted` | Destroy called before match is exhausted |
| `BuyerAuthMissing` / `SellerAuthMissing` | Required signer not found in inputs |
| `BadArgsLength` | Lock args not 65 or 166 bytes |

## Encoding Conventions

- All integers: little-endian
- Capacity values: shannons (1 CKB = 10⁸ shannons)
- `f64`: IEEE 754 little-endian (RISC-V soft-float compatible)
- `rent_per_block` is intentionally not compared during extraction updates — f64 equality is unreliable across hardware FPU vs RISC-V soft-float
- `ABOUT_ONE_DAY_BLOCKS ≈ 10,000`
