use base58::ToBase58;

// TODO: use something similar from separate crate?
pub const HASH_512_LEN: usize = 64;
pub const BASE58_ID: &[u8] = b"SS58PRE";

pub fn ss58hash(data: &[u8]) -> [u8; HASH_512_LEN] {
    let mut blake2b_state = blake2b_simd::Params::new()
        .hash_length(HASH_512_LEN)
        .to_state();
    blake2b_state.update(BASE58_ID);
    blake2b_state.update(data);
    #[expect(
        clippy::expect_used,
        reason = "the state is built with `hash_length(HASH_512_LEN)`, so the digest is always exactly HASH_512_LEN bytes"
    )]
    let hash = blake2b_state
        .finalize()
        .as_bytes()
        .try_into()
        .expect("static length, always fits");

    hash
}

// Same as `to_ss58check_with_version()` method for `Ss58Codec` from `sp_core`,
// comments from `sp_core`.
pub fn to_base58_string(
    bytes: [u8; 32],
    base58prefix: u16,
) -> String {
    // We mask out the upper two bits of the ident - SS58 Prefix currently only
    // supports 14-bits
    let ident: u16 = base58prefix & 0b0011_1111_1111_1111;
    let mut v = match ident {
        #[expect(clippy::cast_possible_truncation)]
        0..=63 => vec![ident as u8],
        64..=16_383 => {
            // upper six bits of the lower byte(!)
            let first = ((ident & 0b0000_0000_1111_1100) as u8) >> 2;
            // lower two bits of the lower byte in the high pos,
            // lower bits of the upper byte in the low pos
            let second = ((ident >> 8) as u8) | ((ident & 0b0000_0000_0000_0011) as u8) << 6;
            vec![first | 0b0100_0000, second]
        },
        #[expect(
            clippy::unreachable,
            reason = "`ident` is masked to 14 bits above, so it cannot exceed 16_383"
        )]
        _ => unreachable!("masked out the upper two bits; qed"),
    };
    v.extend(bytes);
    let r = ss58hash(&v);
    v.extend(&r[0..2]);
    v.to_base58()
}
