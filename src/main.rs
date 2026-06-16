//! Opticrum CLI runner.
//!
//! Provides `deploy`, `migrate`, and `consume` subcommands for managing
//! the Opticrum contract on-chain, powered by ckb-cinnabar.

use ckb_cinnabar::dispatch;

pub fn main() {
    dispatch();
}
