use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

/// Convert an unsigned integer string in token base units to an exact decimal
/// amount at the asset's scale.
pub(crate) fn decimal_from_base_units(
    value: &str,
    decimals: u32,
) -> Option<Decimal> {
    if value.bytes().all(|digit| digit == b'0') {
        return Some(Decimal::ZERO)
    }

    let trailing_zeroes = value
        .bytes()
        .rev()
        .take_while(|digit| *digit == b'0')
        .count();
    let trailing_zeroes = u32::try_from(trailing_zeroes).ok()?;
    let significant_len = value
        .len()
        .checked_sub(usize::try_from(trailing_zeroes).ok()?)?;
    let significant = value.get(..significant_len)?;

    if decimals <= trailing_zeroes {
        let integer_zeroes = usize::try_from(trailing_zeroes.checked_sub(decimals)?).ok()?;
        let mut normalized = String::with_capacity(significant_len.checked_add(integer_zeroes)?);
        normalized.push_str(significant);
        normalized.extend(std::iter::repeat_n('0', integer_zeroes));
        return Decimal::from_str_exact(&normalized).ok()
    }

    let scale = decimals.checked_sub(trailing_zeroes)?;
    if scale > Decimal::MAX_SCALE {
        return None
    }

    let scale = usize::try_from(scale).ok()?;
    let normalized = if significant_len > scale {
        let decimal_point = significant_len.checked_sub(scale)?;
        format!(
            "{}.{}",
            significant.get(..decimal_point)?,
            significant.get(decimal_point..)?
        )
    } else {
        let leading_zeroes = scale.checked_sub(significant_len)?;
        format!(
            "0.{}{}",
            "0".repeat(leading_zeroes),
            significant
        )
    };

    Decimal::from_str_exact(&normalized).ok()
}

/// Convert a decimal token amount to integer base units without rounding or
/// truncating sub-base-unit dust.
pub(crate) fn decimal_to_base_units(
    value: Decimal,
    decimals: u32,
) -> Option<u128> {
    let scaled = Decimal::try_new(1, decimals)
        .ok()
        .and_then(|unit| value.checked_div(unit))?;

    if !scaled.fract().is_zero() {
        return None
    }

    scaled.to_u128()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scale_compatible_trailing_zeroes() {
        assert_eq!(
            decimal_from_base_units("1000000000000000000000000000000", 18),
            Some(Decimal::from(1_000_000_000_000_u64))
        );
    }

    #[test]
    fn rejects_fractional_scale_beyond_decimal_maximum() {
        assert_eq!(decimal_from_base_units("1", 29), None);
    }

    #[test]
    fn base_unit_conversion_requires_an_exact_integer() {
        assert_eq!(
            decimal_to_base_units(
                Decimal::from_str_exact("1.000001").unwrap(),
                6
            ),
            Some(1_000_001)
        );
        assert_eq!(
            decimal_to_base_units(
                Decimal::from_str_exact("1.0000001").unwrap(),
                6
            ),
            None
        );
    }
}
