#![no_std]

extern crate alloc;

#[link(name = "ckb-lib-secp256k1", kind = "static")]
extern "C" {
    #[link_name = "compute_musig2_key_aggregation_xonly"]
    fn musig2_key_aggregation_xonly_ffi(
        pk_a: *const u8,
        pk_b: *const u8,
        xonly_out: *mut u8,
    ) -> i32;
}

/// Compute the 32-byte BIP-327 MuSig2* x-only aggregated key from two
/// 33-byte compressed secp256k1 pubkeys.  Order-independent (keys are
/// sorted internally as Fiber does).
///
/// Returns `Ok([u8; 32])` on success, `Err(error_code)` on failure.
/// Error codes are negative: -10 parse, -11 tweak mul, -12 combine, -13 context init.
pub fn compute_musig2_key_aggregation_xonly(
    pk_a: &[u8; 33],
    pk_b: &[u8; 33],
) -> Result<[u8; 32], i32> {
    let mut xonly = [0u8; 32];
    let ret = unsafe {
        musig2_key_aggregation_xonly_ffi(pk_a.as_ptr(), pk_b.as_ptr(), xonly.as_mut_ptr())
    };
    if ret == 0 {
        Ok(xonly)
    } else {
        Err(ret)
    }
}
