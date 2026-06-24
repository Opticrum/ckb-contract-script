## Learned User Preferences

- When implementing attached plans, do not edit the plan file; use existing todos and mark them in_progress/completed rather than creating new ones.
- Put unit/reference tests inline in the source module (`#[cfg(test)] mod tests`) rather than separate `tests/` integration files when testing internal algorithm modules.
- Run and pass `cargo clippy` for all relevant feature combinations before finishing work.
- Integration tests should use real secp256k1 keypairs when on-chain checks (e.g. channel funding pubkey verification) are no longer skipped in mock mode.

## Learned Workspace Facts

- CKB `load_input_out_point(index, Source::CellDep)` does not work — it only works for inputs; read cell-dep outpoints via `load_transaction()` and `tx.raw().cell_deps()[i].out_point()`, then use the same index `i` with `load_cell_*` syscalls.
- BIP-327 MuSig2 key aggregation lives in `opticrum-protocol/src/keyagg.rs`, gated behind the optional default-off `musig2` feature so the RISC-V contract build pulls in zero extra deps unless enabled.
- Fiber's funding-key reference stack is `musig2 = 0.2.4` + `secp256k1 = 0.30` with deterministic sorted compressed-pubkey ordering (see Fiber `channel.rs`).
- On-chain channel identity check in `order_match`: `blake160(aggregate_funding_keys_xonly(buyer_pk, seller_pk))` must equal the channel CellDep lock args (first 20 bytes).
- MatchArgs `fiber_pubkey` is the seller's Fiber funding pubkey; the buyer's stays in embedded `order_args.fiber_pubkey`.
- `OutPoint::matches` compares raw 36-byte molecule slices (tx_hash + index).
