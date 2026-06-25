#ifndef CKB_MUSIG2_KEYAGG_H_
#define CKB_MUSIG2_KEYAGG_H_

/*
 * MuSig2* key aggregation (BIP-327, 2-of-2) for Fiber channel funding.
 *
 * Replaces the Rust `keyagg` module:
 *   - Does NOT depend on the `secp256k1` crate (avoids embedding ~1 MB of
 *     pre-context tables in the on-chain binary).
 *   - Reuses the existing CKB-VM pre-context data CellDep + libsecp256k1
 *     already linked into the contract.
 *
 * Algorithm (mirrors Fiber's `get_deterministic_musig2_agg_context`):
 *   1. Sort two compressed pubkeys ascending (by 33-byte serialization).
 *   2. L  = tagged_hash("KeyAgg list",        pk1 || pk2)
 *   3. a1 = tagged_hash("KeyAgg coefficient", L   || pk1) mod n
 *   4. Q  = a1 * P1 + P2     (MuSig2* assigns coefficient 1 to pk2)
 *   5. Output the 32-byte x-coordinate of Q (drop parity prefix).
 *
 * Dependencies (all available in the translation unit via secp256k1_helper.h
 * which #includes <secp256k1.c>):
 *   - secp256k1_sha256_*   (hash.h          – BIP-327 tagged hashes)
 *   - secp256k1_ec_pubkey_* (secp256k1.h    – point ops)
 *   - ckb_secp256k1_custom_verify_only_initialize (pre-context from CellDep)
 */

#include "secp256k1_data_info.h"

#define MUSIG2_KEYAGG_ERROR_PUBKEY_PARSE   -10
#define MUSIG2_KEYAGG_ERROR_TWEAK_MUL      -11
#define MUSIG2_KEYAGG_ERROR_COMBINE        -12
#define MUSIG2_KEYAGG_ERROR_CONTEXT_INIT   -13

/* ---------------------------------------------------------------------------
 * Curve order n (big-endian)
 * ------------------------------------------------------------------------- */
static const uint8_t SECP256K1_ORDER[32] = {
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFE,
    0xBA, 0xAE, 0xDC, 0xE6, 0xAF, 0x48, 0xA0, 0x3B,
    0xBF, 0xD2, 0x5E, 0x8C, 0xD0, 0x36, 0x41, 0x41,
};

/* ---------------------------------------------------------------------------
 * reduce_mod_n – conditional subtraction of n
 *
 * Since 2n > 2^256, a single conditional subtraction is sufficient.
 * Modifies `bytes` in place.  Mirrors Rust `keyagg::reduce_mod_n`.
 * ------------------------------------------------------------------------- */
static void reduce_mod_n(uint8_t bytes[32]) {
    /* Compare bytes >= SECP256K1_ORDER (big-endian unsigned) */
    int ge = 0;
    for (int i = 0; i < 32; i++) {
        if (bytes[i] > SECP256K1_ORDER[i]) {
            ge = 1;
            break;
        }
        if (bytes[i] < SECP256K1_ORDER[i]) {
            break;
        }
    }
    if (!ge && memcmp(bytes, SECP256K1_ORDER, 32) < 0) {
        return; /* bytes < n, nothing to do */
    }

    /* bytes -= SECP256K1_ORDER  (borrow-propagating subtraction) */
    int16_t borrow = 0;
    for (int i = 31; i >= 0; i--) {
        int16_t diff = (int16_t)bytes[i] - (int16_t)SECP256K1_ORDER[i] - borrow;
        if (diff < 0) {
            bytes[i] = (uint8_t)(diff + 256);
            borrow = 1;
        } else {
            bytes[i] = (uint8_t)diff;
            borrow = 0;
        }
    }
}

/* ---------------------------------------------------------------------------
 * tagged_sha256 – BIP-327 tagged hash
 *
 * tag_hash = SHA256(tag)
 * result   = SHA256(tag_hash || tag_hash || parts...)
 *
 * `parts` is an array of (ptr, len) pairs packed linearly:
 *   parts[2*i]   = pointer
 *   parts[2*i+1] = length
 * ------------------------------------------------------------------------- */
static void tagged_sha256(const uint8_t *tag,
                          size_t tag_len,
                          const uint8_t * const *parts,
                          const size_t *part_lens,
                          size_t num_parts,
                          uint8_t out[32]) {
    secp256k1_sha256 hasher;
    uint8_t tag_hash[32];

    /* tag_hash = SHA256(tag) */
    secp256k1_sha256_initialize(&hasher);
    secp256k1_sha256_write(&hasher, tag, tag_len);
    secp256k1_sha256_finalize(&hasher, tag_hash);

    /* SHA256(tag_hash || tag_hash || parts...) */
    secp256k1_sha256_initialize(&hasher);
    secp256k1_sha256_write(&hasher, tag_hash, 32);
    secp256k1_sha256_write(&hasher, tag_hash, 32);
    for (size_t i = 0; i < num_parts; i++) {
        secp256k1_sha256_write(&hasher, parts[i], part_lens[i]);
    }
    secp256k1_sha256_finalize(&hasher, out);
}

/* ---------------------------------------------------------------------------
 * sorted_key_pair – parse + deterministic sort of two compressed pubkeys
 *
 * Returns: 0 on success, MUSIG2_KEYAGG_ERROR_PUBKEY_PARSE on failure.
 * On success: *pk1 <= *pk2 by compressed-byte ordering.
 * ------------------------------------------------------------------------- */
static int sorted_key_pair(const uint8_t pk_a[33],
                           const uint8_t pk_b[33],
                           secp256k1_pubkey *pk1,
                           secp256k1_pubkey *pk2,
                           const secp256k1_context *ctx) {
    /* Parse both keys */
    if (!secp256k1_ec_pubkey_parse(ctx, pk1, pk_a, 33)) {
        return MUSIG2_KEYAGG_ERROR_PUBKEY_PARSE;
    }
    if (!secp256k1_ec_pubkey_parse(ctx, pk2, pk_b, 33)) {
        return MUSIG2_KEYAGG_ERROR_PUBKEY_PARSE;
    }

    /* Sort: smaller compressed serialization first */
    if (memcmp(pk_a, pk_b, 33) > 0) {
        /* Swap pk1 <-> pk2 */
        secp256k1_pubkey tmp = *pk1;
        *pk1 = *pk2;
        *pk2 = tmp;
    }
    return 0;
}

/* ---------------------------------------------------------------------------
 * compute_musig2_key_aggregation_xonly  (public API)
 *
 * Computes the 32-byte x-only MuSig2* aggregated key from two 33-byte
 * compressed secp256k1 pubkeys.  Order-independent (keys are sorted
 * internally as Fiber does).
 *
 * Returns 0 on success, negative on error (see error codes above).
 *
 *   pk_a[33]      – first  compressed pubkey (any order)
 *   pk_b[33]      – second compressed pubkey (any order)
 *   xonly_out[32] – output: 32-byte x-only aggregated key
 * ------------------------------------------------------------------------- */
int compute_musig2_key_aggregation_xonly(const uint8_t *pk_a,
                                         const uint8_t *pk_b,
                                         uint8_t *xonly_out) {
    uint8_t secp_data[CKB_SECP256K1_DATA_SIZE];
    secp256k1_context context;
    int ret;

    /* 1. Initialize secp256k1 context (loads pre-context from CellDep) */
    ret = ckb_secp256k1_custom_verify_only_initialize(&context, secp_data);
    if (ret != 0) {
        return MUSIG2_KEYAGG_ERROR_CONTEXT_INIT;
    }

    /* 2. Parse and sort pubkeys */
    secp256k1_pubkey pk1, pk2;
    ret = sorted_key_pair(pk_a, pk_b, &pk1, &pk2, &context);
    if (ret != 0) {
        return ret;
    }

    /* 3. Serialize the (sorted) keys for tagged hash inputs */
    uint8_t pk1_ser[33], pk2_ser[33];
    size_t pk1_len = 33, pk2_len = 33;
    secp256k1_ec_pubkey_serialize(&context, pk1_ser, &pk1_len, &pk1,
                                  SECP256K1_EC_COMPRESSED);
    secp256k1_ec_pubkey_serialize(&context, pk2_ser, &pk2_len, &pk2,
                                  SECP256K1_EC_COMPRESSED);

    /* 4. L = tagged_hash("KeyAgg list", pk1 || pk2) */
    const uint8_t tag_list[] = "KeyAgg list";
    const uint8_t *list_parts[2] = {pk1_ser, pk2_ser};
    const size_t list_lens[2]   = {33, 33};
    uint8_t l[32];
    tagged_sha256(tag_list, sizeof(tag_list) - 1, list_parts, list_lens, 2, l);

    /* 5. a1 = tagged_hash("KeyAgg coefficient", L || pk1) mod n */
    const uint8_t tag_coeff[] = "KeyAgg coefficient";
    const uint8_t *coeff_parts[2] = {l, pk1_ser};
    const size_t coeff_lens[2]   = {32, 33};
    uint8_t a1[32];
    tagged_sha256(tag_coeff, sizeof(tag_coeff) - 1, coeff_parts, coeff_lens, 2,
                  a1);
    reduce_mod_n(a1);

    /* 6. effective1 = a1 * P1 */
    secp256k1_pubkey effective1 = pk1;
    if (!secp256k1_ec_pubkey_tweak_mul(&context, &effective1, a1)) {
        return MUSIG2_KEYAGG_ERROR_TWEAK_MUL;
    }

    /* 7. Q = effective1 + P2 */
    secp256k1_pubkey agg;
    const secp256k1_pubkey *ins[2] = {&effective1, &pk2};
    if (!secp256k1_ec_pubkey_combine(&context, &agg, ins, 2)) {
        return MUSIG2_KEYAGG_ERROR_COMBINE;
    }

    /* 8. Serialize Q as compressed (33 bytes), drop parity prefix -> x-only */
    uint8_t agg_ser[33];
    size_t agg_len = 33;
    secp256k1_ec_pubkey_serialize(&context, agg_ser, &agg_len, &agg,
                                  SECP256K1_EC_COMPRESSED);

    /* x-only = bytes 1..32 (skip 0x02/0x03 parity prefix) */
    memcpy(xonly_out, &agg_ser[1], 32);

    return 0;
}

#endif /* CKB_MUSIG2_KEYAGG_H_ */
