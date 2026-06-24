//! Standalone `no_std` BIP-327 MuSig2* key aggregation for the 2-of-2 funding key.
//!
//! Fiber funds every channel with a 2-of-2 MuSig2 multisig. The on-chain
//! funding cell only stores `blake160(x_only_aggregated_pubkey)` in its lock
//! args (and the full x-only key in the witness), so the individual party keys
//! cannot be recovered from chain data. They *can*, however, be verified in the
//! forward direction: given both parties' compressed `funding_pubkey`s, recompute
//! the aggregated key and compare.
//!
//! This module reproduces exactly what Fiber does, but with no `std` and no
//! `musig2`/`secp` crate (both require `std`). It relies only on:
//!   - `sha2` (`no_std`) for the BIP-327 tagged hashes, and
//!   - `secp256k1` (`alloc`) for point (de)serialization, scalar*point and
//!     point+point.
//!
//! Reference (Fiber `crates/fiber-lib/src/fiber/channel.rs`):
//!   - `get_deterministic_musig2_agg_context` — builds `KeyAggContext` from the
//!     two ordered funding pubkeys.
//!   - `order_things_for_musig2` / `should_local_go_first_in_musig2` — orders the
//!     two keys ascending by their 33-byte compressed serialization.
//!   - `get_funding_lock_script_xonly_key` — takes `aggregated_pubkey()` and
//!     drops the parity to an x-only key.
//!
//! Because BIP-327's `KeyAggContext` does not reorder its input, Fiber's manual
//! sort is what makes aggregation deterministic regardless of which party is
//! "local". We replicate that sort here.

use crate::CompressedPubkey;
use secp256k1::{PublicKey, Scalar, Secp256k1};
use sha2::{Digest, Sha256};

/// Length of an x-only (BIP-340) public key.
pub const XONLY_PUBKEY_LEN: usize = 32;

/// The secp256k1 group order `n`, big-endian.
const CURVE_ORDER: [u8; 32] = [
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B, 0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
];

/// BIP-327 "KeyAgg list" tag.
const TAG_KEYAGG_LIST: &[u8] = b"KeyAgg list";
/// BIP-327 "KeyAgg coefficient" tag.
const TAG_KEYAGG_COEFF: &[u8] = b"KeyAgg coefficient";

/// Errors surfaced by key aggregation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAggError {
    /// One of the inputs was not a valid compressed secp256k1 point.
    InvalidPubkey,
    /// The aggregation step failed (degenerate coefficient or points cancelled).
    Aggregation,
}

/// BIP-327 tagged hash: `SHA256(SHA256(tag) || SHA256(tag) || parts...)`.
///
/// `parts` are streamed into the hasher so no heap buffer is needed.
fn tagged_hash(tag: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let tag_hash = Sha256::digest(tag);
    let mut hasher = Sha256::new();
    hasher.update(tag_hash);
    hasher.update(tag_hash);
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// Reduce a 32-byte big-endian value into `[0, n)`.
///
/// A 256-bit integer is always `< 2n` (since `2n > 2^256`), so at most one
/// conditional subtraction of `n` is required — no general bignum division.
fn reduce_mod_n(bytes: &[u8; 32]) -> [u8; 32] {
    let mut out = *bytes;
    if out >= CURVE_ORDER {
        let mut borrow = 0i16;
        for i in (0..32).rev() {
            let diff = out[i] as i16 - CURVE_ORDER[i] as i16 - borrow;
            if diff < 0 {
                out[i] = (diff + 256) as u8;
                borrow = 1;
            } else {
                out[i] = diff as u8;
                borrow = 0;
            }
        }
    }
    out
}

/// Aggregate two pubkeys that are already sorted ascending (`pk1 <= pk2` by
/// compressed bytes), per BIP-327 MuSig2*.
///
/// MuSig2* assigns coefficient `1` to the "second distinct" key. After sorting
/// two distinct keys ascending, that key is `pk2`. So:
///   `Q = a1 * P1 + P2` where `a1 = H_coeff(L || pk1) mod n`.
fn aggregate_sorted(pk1: &PublicKey, pk2: &PublicKey) -> Result<PublicKey, KeyAggError> {
    let pk1_bytes = pk1.serialize();
    let pk2_bytes = pk2.serialize();

    // L = tagged_hash("KeyAgg list", pk1 || pk2)
    let l = tagged_hash(TAG_KEYAGG_LIST, &[&pk1_bytes, &pk2_bytes]);

    // a1 = int(tagged_hash("KeyAgg coefficient", L || pk1)) mod n
    let a1_hash = tagged_hash(TAG_KEYAGG_COEFF, &[&l, &pk1_bytes]);
    let a1 = Scalar::from_be_bytes(reduce_mod_n(&a1_hash)).map_err(|_| KeyAggError::Aggregation)?;

    let secp = Secp256k1::new();
    // effective1 = a1 * P1; effective2 = 1 * P2 = P2.
    let effective1 = pk1
        .mul_tweak(&secp, &a1)
        .map_err(|_| KeyAggError::Aggregation)?;
    PublicKey::combine_keys(&[&effective1, pk2]).map_err(|_| KeyAggError::Aggregation)
}

/// Order two compressed pubkeys ascending and parse them.
///
/// Mirrors Fiber's `order_things_for_musig2`: smaller 33-byte compressed
/// serialization first. Returns `(pk1, pk2)` with `pk1 <= pk2`.
fn ordered_pair(
    pk_a: &CompressedPubkey,
    pk_b: &CompressedPubkey,
) -> Result<(PublicKey, PublicKey), KeyAggError> {
    let key_a = PublicKey::from_slice(pk_a.as_bytes()).map_err(|_| KeyAggError::InvalidPubkey)?;
    let key_b = PublicKey::from_slice(pk_b.as_bytes()).map_err(|_| KeyAggError::InvalidPubkey)?;
    if pk_a.as_bytes() <= pk_b.as_bytes() {
        Ok((key_a, key_b))
    } else {
        Ok((key_b, key_a))
    }
}

/// Aggregate two funding pubkeys into the compressed (33-byte) MuSig2* key.
///
/// Inputs may be in any order; they are sorted internally exactly as Fiber does,
/// so `aggregate_funding_keys(a, b) == aggregate_funding_keys(b, a)`.
pub fn aggregate_funding_keys(
    pk_a: &CompressedPubkey,
    pk_b: &CompressedPubkey,
) -> Result<CompressedPubkey, KeyAggError> {
    let (pk1, pk2) = ordered_pair(pk_a, pk_b)?;
    Ok(CompressedPubkey::new(aggregate_sorted(&pk1, &pk2)?.serialize()))
}

/// Aggregate two funding pubkeys into the x-only (32-byte) MuSig2* key.
///
/// This is the key used to build the Fiber funding lock script
/// (`get_funding_lock_script_xonly_key`): the aggregated point with its parity
/// dropped.
pub fn aggregate_funding_keys_xonly(
    pk_a: &CompressedPubkey,
    pk_b: &CompressedPubkey,
) -> Result<[u8; XONLY_PUBKEY_LEN], KeyAggError> {
    let (pk1, pk2) = ordered_pair(pk_a, pk_b)?;
    let agg = aggregate_sorted(&pk1, &pk2)?;
    Ok(agg.x_only_public_key().0.serialize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CompressedPubkey;
    use musig2::KeyAggContext;
    use secp256k1::{PublicKey, Secp256k1, SecretKey};

    /// Fiber's deterministic aggregation, using the real `musig2` crate as oracle.
    /// Returns `(compressed_33, xonly_32)`.
    fn fiber_reference(pk_a: &PublicKey, pk_b: &PublicKey) -> ([u8; 33], [u8; 32]) {
        // order_things_for_musig2: smaller compressed serialization goes first.
        let (k1, k2) = if pk_a.serialize() <= pk_b.serialize() {
            (*pk_a, *pk_b)
        } else {
            (*pk_b, *pk_a)
        };
        let ctx = KeyAggContext::new([k1, k2]).expect("valid pubkeys");
        let agg: PublicKey = ctx.aggregated_pubkey();
        (agg.serialize(), agg.x_only_public_key().0.serialize())
    }

    fn keypair_from(secret: [u8; 32]) -> PublicKey {
        let secp = Secp256k1::new();
        let sk = SecretKey::from_slice(&secret).expect("valid secret");
        PublicKey::from_secret_key(&secp, &sk)
    }

    #[test]
    fn matches_fiber_reference_random() {
        let secp = Secp256k1::new();
        let mut rng = rand::thread_rng();

        for _ in 0..256 {
            let (_, pk_a) = secp.generate_keypair(&mut rng);
            let (_, pk_b) = secp.generate_keypair(&mut rng);
            let a = CompressedPubkey::new(pk_a.serialize());
            let b = CompressedPubkey::new(pk_b.serialize());

            let (ref_compressed, ref_xonly) = fiber_reference(&pk_a, &pk_b);

            for (x, y) in [(&a, &b), (&b, &a)] {
                assert_eq!(
                    aggregate_funding_keys(x, y).expect("aggregation").to_bytes(),
                    ref_compressed,
                    "compressed aggregate mismatch vs Fiber reference"
                );
                assert_eq!(
                    aggregate_funding_keys_xonly(x, y).expect("aggregation"),
                    ref_xonly,
                    "x-only aggregate mismatch vs Fiber reference"
                );
            }
        }
    }

    #[test]
    fn sorting_is_deterministic() {
        let pk_a = CompressedPubkey::new(keypair_from([0x11; 32]).serialize());
        let pk_b = CompressedPubkey::new(keypair_from([0x22; 32]).serialize());

        assert_eq!(
            aggregate_funding_keys(&pk_a, &pk_b).unwrap(),
            aggregate_funding_keys(&pk_b, &pk_a).unwrap(),
            "aggregation must be independent of input order"
        );
        assert_eq!(
            aggregate_funding_keys_xonly(&pk_a, &pk_b).unwrap(),
            aggregate_funding_keys_xonly(&pk_b, &pk_a).unwrap(),
        );
    }

    #[test]
    fn known_answer_vector() {
        let pk_a = keypair_from([0x11; 32]);
        let pk_b = keypair_from([0x22; 32]);

        let (ref_compressed, ref_xonly) = fiber_reference(&pk_a, &pk_b);
        let ours_compressed =
            aggregate_funding_keys(&CompressedPubkey::new(pk_a.serialize()), &CompressedPubkey::new(pk_b.serialize()))
                .unwrap();
        let ours_xonly = aggregate_funding_keys_xonly(
            &CompressedPubkey::new(pk_a.serialize()),
            &CompressedPubkey::new(pk_b.serialize()),
        )
        .unwrap();

        assert_eq!(ours_compressed.to_bytes(), ref_compressed);
        assert_eq!(ours_xonly, ref_xonly);

        const EXPECTED_XONLY: [u8; 32] = [
            0xfc, 0x23, 0x18, 0x2b, 0x2e, 0xe6, 0xd2, 0x54, 0x38, 0xf1, 0x48, 0xc2, 0x83, 0x6a,
            0x5c, 0xa0, 0x41, 0xb4, 0xca, 0xca, 0x3d, 0x99, 0x20, 0x22, 0x94, 0xae, 0x89, 0xc7,
            0x90, 0x33, 0x2f, 0xb3,
        ];
        assert_eq!(ours_xonly, EXPECTED_XONLY);
    }
}
