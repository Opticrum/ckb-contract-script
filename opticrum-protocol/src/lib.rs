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

use ckb_cinnabar_verifier::re_exports::ckb_std::ckb_types::{packed, prelude::Entity};

pub const FIBER_PUBKEY_LEN: usize = 33; // 1-byte prefix + 32-byte x-coordinate
pub const LOCK_HASH_LEN: usize = 32;

pub const ORDER_ARGS_LEN: usize = FIBER_PUBKEY_LEN + LOCK_HASH_LEN; // 65
pub const CHANNEL_OUTPOINT_LEN: usize = 32 + 4; // 36: tx_hash[32] + index[4] (u32 LE)
pub const MATCH_ARGS_LEN: usize =
    ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN + LOCK_HASH_LEN + FIBER_PUBKEY_LEN; // 166
/// Fiber funding lock args: blake160(x-only aggregated MuSig2 pubkey).
pub const FIBER_FUNDING_LOCK_ARGS_LEN: usize = 20;

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

    pub fn matches(&self, other: &packed::OutPoint) -> bool {
        other.as_slice() == self.to_bytes().as_slice()
    }
}

// ---------------------------------------------------------------------------
// CompressedPubkey — 33-byte compressed secp256k1 public key
// ---------------------------------------------------------------------------
// Layout: prefix[1] | x_coordinate[32]
//
// The prefix byte encodes y-coordinate parity:
//   - 0x02 — Y is even
//   - 0x03 — Y is odd

#[derive(Clone, PartialEq, Eq)]
pub struct CompressedPubkey([u8; FIBER_PUBKEY_LEN]);

impl CompressedPubkey {
    /// Wrap a raw 33-byte array as a CompressedPubkey.
    pub fn new(bytes: [u8; FIBER_PUBKEY_LEN]) -> Self {
        Self(bytes)
    }

    /// Parse from a byte slice. Returns an error if the slice length != 33.
    pub fn from_slice(data: &[u8]) -> Result<Self, &'static str> {
        if data.len() != FIBER_PUBKEY_LEN {
            return Err("Bad compressed pubkey length");
        }
        let mut bytes = [0u8; FIBER_PUBKEY_LEN];
        bytes.copy_from_slice(data);
        Ok(Self(bytes))
    }

    /// Return the raw 33-byte array.
    pub fn to_bytes(&self) -> [u8; FIBER_PUBKEY_LEN] {
        self.0
    }

    /// Borrow as a reference to the raw 33-byte array.
    pub fn as_bytes(&self) -> &[u8; FIBER_PUBKEY_LEN] {
        &self.0
    }

    /// Compression prefix byte: `0x02` (even Y) or `0x03` (odd Y).
    pub fn prefix(&self) -> u8 {
        self.0[0]
    }

    /// Returns `true` if the prefix byte matches a valid compressed pubkey prefix.
    pub fn is_compressed(&self) -> bool {
        matches!(self.0[0], 0x02 | 0x03)
    }
}

impl core::fmt::Debug for CompressedPubkey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "CompressedPubkey(0x")?;
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, ")")
    }
}

impl core::ops::Deref for CompressedPubkey {
    type Target = [u8; FIBER_PUBKEY_LEN];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// ---------------------------------------------------------------------------
// OrderArgs — 65-byte Order Cell lock args
// ---------------------------------------------------------------------------
// Layout: fiber_pubkey[33] | buyer_lock_hash[32]

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderArgs {
    pub fiber_pubkey: CompressedPubkey,
    pub buyer_lock_hash: [u8; LOCK_HASH_LEN],
}

impl OrderArgs {
    pub fn new(fiber_pubkey: CompressedPubkey, buyer_lock_hash: [u8; LOCK_HASH_LEN]) -> Self {
        Self {
            fiber_pubkey,
            buyer_lock_hash,
        }
    }

    pub fn from_slice(args: &[u8]) -> Result<Self, &'static str> {
        if args.len() != ORDER_ARGS_LEN {
            return Err("Bad Order args length");
        }
        let fiber_pubkey = CompressedPubkey::from_slice(&args[0..FIBER_PUBKEY_LEN])?;
        let mut buyer_lock_hash = [0u8; LOCK_HASH_LEN];
        buyer_lock_hash.copy_from_slice(&args[FIBER_PUBKEY_LEN..ORDER_ARGS_LEN]);
        Ok(Self {
            fiber_pubkey,
            buyer_lock_hash,
        })
    }

    pub fn to_bytes(&self) -> [u8; ORDER_ARGS_LEN] {
        let mut buf = [0u8; ORDER_ARGS_LEN];
        buf[0..FIBER_PUBKEY_LEN].copy_from_slice(&self.fiber_pubkey.to_bytes());
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
// MatchArgs — 166-byte Match Cell lock args
// ---------------------------------------------------------------------------
// Layout: OrderArgs[65] | channel_outpoint[36] | seller_lock_hash[32] | fiber_pubkey[33]
//
// `order_args.fiber_pubkey` is the buyer's Fiber channel funding pubkey.
// `fiber_pubkey` is the seller's Fiber channel funding pubkey (added at match time).

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchArgs {
    pub order_args: OrderArgs,
    pub channel_outpoint: OutPoint,
    pub seller_lock_hash: [u8; LOCK_HASH_LEN],
    /// Seller's Fiber channel funding pubkey (33-byte compressed secp256k1).
    pub fiber_pubkey: CompressedPubkey,
}

impl MatchArgs {
    pub fn new(
        order_args: OrderArgs,
        channel_outpoint: OutPoint,
        seller_lock_hash: [u8; LOCK_HASH_LEN],
        fiber_pubkey: CompressedPubkey,
    ) -> Self {
        Self {
            order_args,
            channel_outpoint,
            seller_lock_hash,
            fiber_pubkey,
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
        let fiber_pubkey = CompressedPubkey::from_slice(
            &args[seller_offset + LOCK_HASH_LEN..seller_offset + LOCK_HASH_LEN + FIBER_PUBKEY_LEN],
        )?;
        Ok(Self {
            order_args,
            channel_outpoint,
            seller_lock_hash,
            fiber_pubkey,
        })
    }

    pub fn to_bytes(&self) -> [u8; MATCH_ARGS_LEN] {
        let mut buf = [0u8; MATCH_ARGS_LEN];
        buf[0..ORDER_ARGS_LEN].copy_from_slice(&self.order_args.to_bytes());
        buf[ORDER_ARGS_LEN..ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN]
            .copy_from_slice(&self.channel_outpoint.to_bytes());
        let seller_offset = ORDER_ARGS_LEN + CHANNEL_OUTPOINT_LEN;
        buf[seller_offset..seller_offset + LOCK_HASH_LEN].copy_from_slice(&self.seller_lock_hash);
        buf[seller_offset + LOCK_HASH_LEN..MATCH_ARGS_LEN]
            .copy_from_slice(&self.fiber_pubkey.to_bytes());
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
        // Note: rent_per_block is intentionally not compared — f64 equality
        // is unreliable across platforms (hardware vs RISC-V soft-float).
        // It is an invariant set at match time and never changes.
        if new_match_data.escrow_blocks != self.escrow_blocks
            || new_match_data.last_extraction_block != tip_block
            || new_match_data.xudt_amount.saturating_add(xudt_extraction) > self.xudt_amount
        {
            return false;
        }
        true
    }
}
