# Opticrum

A decentralized liquidity marketplace for the [Fiber Network](https://github.com/nervosnetwork/fiber) on [CKB](https://github.com/nervosnetwork/ckb).

## Overview

Opticrum is the fully decentralized version of [Amboss](https://amboss.tech/) — a liquidity marketplace where:

- **Buyers** create on-chain Order Cells offering rent for inbound channel liquidity
- **Sellers** match orders by opening Fiber channels, earning rent proportionally over time

Built with the [ckb-cinnabar](https://github.com/ashuralyk/ckb-cinnabar) framework.

## Architecture

```
Order Cell (buyer creates)
    │
    ├── Cancel (buyer reclaims)
    │
    └── Match (seller opens channel, creates Match Cell)
            │
            ├── Extract Rent (seller periodically withdraws)
            │
            └── Destroy (expired, buyer/seller sweeps remaining)
```

Two Cell states discriminated by lock script `args` length:
- **Order** (68 bytes): Fiber Pubkey + Buyer Pubkey Hash + Channel Capacity + Escrow Blocks
- **Match** (120 bytes): Order fields + Channel Lock Hash + Seller Pubkey Hash

## Project Structure

```
├── contracts/opticrum/    # On-chain verification (no_std, RISC-V)
├── calculator/opticrum/   # Off-chain transaction assembly
├── tests/                 # Integration tests
└── src/                   # CLI runner (deploy/migrate/consume)
```

## Build

```bash
make build    # Compile RISC-V contract binary
make test     # Run integration tests
```
