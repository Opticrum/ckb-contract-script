#![no_std]

//! Canonical on-chain data layouts for Opticrum cells.
//!
//! This crate defines the byte-level protocol types shared between the
//! on-chain RISC-V verification scripts (contracts/) and the off-chain
//! transaction assembly tools (calculator/).
//!
//! All `from_slice` constructors return `Result<T, &'static str>` so
//! each consumer can map errors into its own error type.

// ---------------------------------------------------------------------------
// Length constants
// ---------------------------------------------------------------------------

use ckb_cinnabar_verifier::re_exports::ckb_std::ckb_types::{packed, prelude::Unpack};

pub const FIBER_PUBKEY_LEN: usize = 32;
pub const LOCK_HASH_LEN: usize = 32;

pub const ORDER_ARGS_LEN: usize = FIBER_PUBKEY_LEN + LOCK_HASH_LEN; // 64
pub const CHANNEL_OUTPOINT_LEN: usize = 32 + 4; // 36: tx_hash[32] + index[4] (u32 LE)
pub const MATCH_ARGS_LEN: usize = ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN + LOCK_HASH_LEN; // 132

pub const XUDT_AMOUNT_LEN: usize = 16;
pub const CHANNEL_CAPACITY_LEN: usize = 8;
pub const ESCROW_BLOCKS_LEN: usize = 8;
pub const RENT_PER_BLOCK_LEN: usize = 8; // f64
pub const BLOCKNUMBER_LEN: usize = 8;

pub const ORDER_DATA_LEN: usize = XUDT_AMOUNT_LEN + CHANNEL_CAPACITY_LEN + ESCROW_BLOCKS_LEN; // 32
pub const MATCH_DATA_LEN: usize =
    XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN + ESCROW_BLOCKS_LEN + BLOCKNUMBER_LEN; // 40

// ---------------------------------------------------------------------------
// ChannelOutpoint — raw 36-byte outpoint (no ckb-types dependency)
// ---------------------------------------------------------------------------
// Layout: tx_hash[32] | index[4] (u32 LE)
//
// Stored as raw bytes so this crate remains free of CKB-specific
// dependencies and works in both no_std (contract) and std (calculator)
// contexts. Consumers convert to/from their preferred OutPoint type.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutPoint {
    pub tx_hash: [u8; 32],
    pub index: u32,
}

impl OutPoint {
    pub fn new(tx_hash: [u8; 32], index: u32) -> Self {
        Self { tx_hash, index }
    }

    pub fn from_slice(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() != CHANNEL_OUTPOINT_LEN {
            return Err("Bad Channel outpoint length");
        }
        let mut tx_hash = [0u8; 32];
        tx_hash.copy_from_slice(&data[0..32]);
        let index = u32::from_le_bytes(data[32..36].try_into().unwrap());
        Ok(Self { tx_hash, index })
    }

    pub fn to_bytes(&self) -> [u8; CHANNEL_OUTPOINT_LEN] {
        let mut buf = [0u8; CHANNEL_OUTPOINT_LEN];
        buf[0..32].copy_from_slice(&self.tx_hash);
        buf[32..36].copy_from_slice(&self.index.to_le_bytes());
        buf
    }

    pub fn eq(&self, other: &packed::OutPoint) -> bool {
        self.tx_hash == other.tx_hash().unpack() && self.index == other.index().unpack()
    }
}

// ---------------------------------------------------------------------------
// OrderArgs — 64-byte Order Cell lock args
// ---------------------------------------------------------------------------
// Layout: fiber_pubkey[32] | buyer_lock_hash[32]

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderArgs {
    pub fiber_pubkey: [u8; FIBER_PUBKEY_LEN],
    pub buyer_lock_hash: [u8; LOCK_HASH_LEN],
}

impl OrderArgs {
    pub fn new(fiber_pubkey: [u8; FIBER_PUBKEY_LEN], buyer_lock_hash: [u8; LOCK_HASH_LEN]) -> Self {
        Self {
            fiber_pubkey,
            buyer_lock_hash,
        }
    }

    pub fn from_slice(args: &[u8]) -> Result<Self, &'static str> {
        if args.len() != ORDER_ARGS_LEN {
            return Err("Bad Order args length");
        }
        let mut fiber_pubkey = [0u8; FIBER_PUBKEY_LEN];
        fiber_pubkey.copy_from_slice(&args[0..FIBER_PUBKEY_LEN]);
        let mut buyer_lock_hash = [0u8; LOCK_HASH_LEN];
        buyer_lock_hash.copy_from_slice(&args[FIBER_PUBKEY_LEN..ORDER_ARGS_LEN]);
        Ok(Self {
            fiber_pubkey,
            buyer_lock_hash,
        })
    }

    pub fn to_bytes(&self) -> [u8; ORDER_ARGS_LEN] {
        let mut buf = [0u8; ORDER_ARGS_LEN];
        buf[0..FIBER_PUBKEY_LEN].copy_from_slice(&self.fiber_pubkey);
        buf[FIBER_PUBKEY_LEN..ORDER_ARGS_LEN].copy_from_slice(&self.buyer_lock_hash);
        buf
    }
}

// ---------------------------------------------------------------------------
// OrderData — 32-byte Order Cell data
// ---------------------------------------------------------------------------
// Layout: xudt_amount[16] | channel_capacity[8] | escrow_blocks[8]
//
// Stored as the cell's `data` field, separately from OrderArgs in the lock.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderData {
    pub xudt_amount: u128,
    pub channel_capacity: u64,
    pub escrow_blocks: u64,
}

impl OrderData {
    pub fn new(xudt_amount: u128, channel_capacity: u64, escrow_blocks: u64) -> Self {
        Self {
            xudt_amount,
            channel_capacity,
            escrow_blocks,
        }
    }

    pub fn from_slice(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() != ORDER_DATA_LEN {
            return Err("Bad Order data length");
        }
        Ok(Self {
            xudt_amount: u128::from_le_bytes(data[0..XUDT_AMOUNT_LEN].try_into().unwrap()),
            channel_capacity: u64::from_le_bytes(
                data[XUDT_AMOUNT_LEN..XUDT_AMOUNT_LEN + CHANNEL_CAPACITY_LEN]
                    .try_into()
                    .unwrap(),
            ),
            escrow_blocks: u64::from_le_bytes(
                data[XUDT_AMOUNT_LEN + CHANNEL_CAPACITY_LEN..ORDER_DATA_LEN]
                    .try_into()
                    .unwrap(),
            ),
        })
    }

    pub fn to_bytes(&self) -> [u8; ORDER_DATA_LEN] {
        let mut buf = [0u8; ORDER_DATA_LEN];
        buf[0..XUDT_AMOUNT_LEN].copy_from_slice(&self.xudt_amount.to_le_bytes());
        buf[XUDT_AMOUNT_LEN..XUDT_AMOUNT_LEN + CHANNEL_CAPACITY_LEN]
            .copy_from_slice(&self.channel_capacity.to_le_bytes());
        buf[XUDT_AMOUNT_LEN + CHANNEL_CAPACITY_LEN..ORDER_DATA_LEN]
            .copy_from_slice(&self.escrow_blocks.to_le_bytes());
        buf
    }
}

// ---------------------------------------------------------------------------
// MatchArgs — 132-byte Match Cell lock args
// ---------------------------------------------------------------------------
// Layout: OrderArgs[64] | channel_outpoint[36] | seller_lock_hash[32]

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArgs {
    pub order_args: OrderArgs,
    pub channel_outpoint: OutPoint,
    pub seller_lock_hash: [u8; LOCK_HASH_LEN],
}

impl MatchArgs {
    pub fn new(
        order_args: OrderArgs,
        channel_outpoint: OutPoint,
        seller_lock_hash: [u8; LOCK_HASH_LEN],
    ) -> Self {
        Self {
            order_args,
            channel_outpoint,
            seller_lock_hash,
        }
    }

    pub fn from_slice(args: &[u8]) -> Result<Self, &'static str> {
        if args.len() != MATCH_ARGS_LEN {
            return Err("Bad Match args length");
        }
        let order_args = OrderArgs::from_slice(&args[..ORDER_ARGS_LEN])?;
        let channel_outpoint =
            OutPoint::from_slice(&args[ORDER_ARGS_LEN..ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN])
                .map_err(|_| "Bad Channel outpoint")?;
        let seller_offset = ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN;
        let mut seller_lock_hash = [0u8; LOCK_HASH_LEN];
        seller_lock_hash.copy_from_slice(&args[seller_offset..seller_offset + LOCK_HASH_LEN]);
        Ok(Self {
            order_args,
            channel_outpoint,
            seller_lock_hash,
        })
    }

    pub fn to_bytes(&self) -> [u8; MATCH_ARGS_LEN] {
        let mut buf = [0u8; MATCH_ARGS_LEN];
        buf[0..ORDER_ARGS_LEN].copy_from_slice(&self.order_args.to_bytes());
        buf[ORDER_ARGS_LEN..ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN]
            .copy_from_slice(&self.channel_outpoint.to_bytes());
        buf[ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN..MATCH_ARGS_LEN]
            .copy_from_slice(&self.seller_lock_hash);
        buf
    }
}

// ---------------------------------------------------------------------------
// MatchData — 40-byte Match Cell data
// ---------------------------------------------------------------------------
// Layout: xudt_amount[16] | rent_per_block[8] | escrow_blocks[8] |
//         last_extraction_block[8]
//
// `rent_per_block` is pre-computed at match time as total_rent / escrow_blocks.
// `escrow_blocks` is stored here so Match verifiers can compute expiry
// without loading the original Order cell.

#[derive(Clone, Debug, PartialEq)]
pub struct MatchData {
    pub xudt_amount: u128,
    pub rent_per_block: f64,
    pub escrow_blocks: u64,
    pub last_extraction_block: u64,
}

impl Eq for MatchData {}

impl MatchData {
    pub fn new(xudt_amount: u128, rent_per_block: f64, escrow_blocks: u64) -> Self {
        Self {
            xudt_amount,
            rent_per_block,
            escrow_blocks,
            last_extraction_block: 0,
        }
    }

    pub fn from_slice(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() < MATCH_DATA_LEN {
            return Err("Bad Match data length");
        }
        Ok(Self {
            xudt_amount: u128::from_le_bytes(data[0..XUDT_AMOUNT_LEN].try_into().unwrap()),
            rent_per_block: f64::from_le_bytes(
                data[XUDT_AMOUNT_LEN..XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN]
                    .try_into()
                    .unwrap(),
            ),
            escrow_blocks: u64::from_le_bytes(
                data[XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN
                    ..XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN + ESCROW_BLOCKS_LEN]
                    .try_into()
                    .unwrap(),
            ),
            last_extraction_block: u64::from_le_bytes(
                data[XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN + ESCROW_BLOCKS_LEN..MATCH_DATA_LEN]
                    .try_into()
                    .unwrap(),
            ),
        })
    }

    pub fn to_bytes(&self) -> [u8; MATCH_DATA_LEN] {
        let mut buf = [0u8; MATCH_DATA_LEN];
        buf[0..XUDT_AMOUNT_LEN].copy_from_slice(&self.xudt_amount.to_le_bytes());
        buf[XUDT_AMOUNT_LEN..XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN]
            .copy_from_slice(&self.rent_per_block.to_le_bytes());
        buf[XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN
            ..XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN + ESCROW_BLOCKS_LEN]
            .copy_from_slice(&self.escrow_blocks.to_le_bytes());
        buf[XUDT_AMOUNT_LEN + RENT_PER_BLOCK_LEN + ESCROW_BLOCKS_LEN..MATCH_DATA_LEN]
            .copy_from_slice(&self.last_extraction_block.to_le_bytes());
        buf
    }

    pub fn good_extraction(
        &self,
        new_match_data: &Self,
        tip_block: u64,
        xudt_extraction: u128,
    ) -> bool {
        if new_match_data.rent_per_block != self.rent_per_block
            || new_match_data.escrow_blocks != self.escrow_blocks
            || new_match_data.last_extraction_block != tip_block
            || new_match_data.xudt_amount.saturating_add(xudt_extraction) > self.xudt_amount
        {
            return false;
        }
        true
    }
}
