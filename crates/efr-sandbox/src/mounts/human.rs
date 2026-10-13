//! The number format of GNU `df`: gnulib's `human_readable`, with its default rounding
//! up, in exact integer arithmetic.

/// The letters of the powers: none, kilo, mega and on.
const POWER_LETTERS: &[u8] = b"\0KMGTPEZYRQ";

/// How a number prints, as gnulib's `human_*` option bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Human {
    /// Picks the power of the base that keeps the number small (`-h`, `-H`).
    pub(crate) autoscale: bool,
    /// Appends the letter of the power.
    pub(crate) si: bool,
    /// Powers of 1024, not of 1000.
    pub(crate) base_1024: bool,
    /// Appends `B` (and `iB` for powers of 1024) after the letter.
    pub(crate) bytes: bool,
    /// Drops a `.0` after a number under 10.
    pub(crate) suppress_point_zero: bool,
}

/// `n` units of `from` bytes in units of `to` bytes, rounded up, as `df` prints it.
pub(crate) fn human_readable(n: u64, opts: Human, from: u64, to: u64) -> String {
    let base: u128 = if opts.base_1024 { 1024 } else { 1000 };
    let max_exponent = POWER_LETTERS.len() - 1;
    let to = u128::from(to.max(1));
    let bytes = u128::from(n) * u128::from(from);
    let mut amount = bytes / to;
    let remainder = bytes % to;
    // tenths and rounding as gnulib keeps them: 0 means exact, 1 below a half tenth,
    // 2 exactly a half tenth, 3 above.
    let mut tenths = remainder * 10 / to;
    let twice = (remainder * 10 % to) * 2;
    let mut rounding = if twice < to { u128::from(twice > 0) } else { 2 + u128::from(to < twice) };
    let mut exponent: Option<usize> = None;
    let mut fraction = String::new();

    if opts.autoscale {
        let mut scaled = 0;
        if base <= amount {
            loop {
                let r10 = (amount % base) * 10 + tenths;
                let r2 = (r10 % base) * 2 + (rounding >> 1);
                amount /= base;
                tenths = r10 / base;
                rounding = if r2 < base {
                    u128::from(r2 + rounding != 0)
                } else {
                    2 + u128::from(base < r2 + rounding)
                };
                scaled += 1;
                if !(base <= amount && scaled < max_exponent) {
                    break;
                }
            }
            if amount < 10 {
                if rounding > 0 {
                    tenths += 1;
                    rounding = 0;
                    if tenths == 10 {
                        amount += 1;
                        tenths = 0;
                    }
                }
                if amount < 10 && (tenths != 0 || !opts.suppress_point_zero) {
                    fraction = format!(".{tenths}");
                    tenths = 0;
                    rounding = 0;
                }
            }
        }
        exponent = Some(scaled);
    }

    if tenths + rounding > 0 {
        amount += 1;
        if let Some(scaled) = exponent
            && amount == base
            && scaled < max_exponent
        {
            exponent = Some(scaled + 1);
            if !opts.suppress_point_zero {
                fraction = ".0".to_owned();
            }
            amount = 1;
        }
    }

    let mut text = format!("{amount}{fraction}");
    if opts.si {
        let exponent = exponent.unwrap_or_else(|| {
            let mut exponent = 0;
            let mut power: u128 = 1;
            while power < to && exponent < max_exponent {
                power *= base;
                exponent += 1;
            }
            exponent
        });
        if exponent > 0 {
            let letter = if !opts.base_1024 && exponent == 1 {
                'k'
            } else {
                char::from(POWER_LETTERS[exponent])
            };
            text.push(letter);
        }
        if opts.bytes {
            if opts.base_1024 && exponent > 0 {
                text.push('i');
            }
            text.push('B');
        }
    }
    text
}

/// `used` as a percent of `used + available`, rounded up, as `df` prints it: `-` when
/// both are 0.
pub(crate) fn percent(used: u64, available: u64) -> String {
    let total = u128::from(used) + u128::from(available);
    if total == 0 {
        return "-".to_owned();
    }
    let hundred = u128::from(used) * 100;
    let pct = hundred / total + u128::from(hundred % total != 0);
    format!("{pct}%")
}

/// A size as util-linux prints it (`size_to_human_string` with a one letter suffix):
/// powers of 1024, one decimal rounded to nearest, no `.0`, `B` under 1 KiB.
pub(crate) fn util_linux_size(bytes: u64) -> String {
    let mut exp = 0_u32;
    for shift in (10..=60).step_by(10) {
        exp = shift - 10;
        if u128::from(bytes) < 1_u128 << shift {
            break;
        }
        exp = shift;
    }
    let letters = b"BKMGTPE";
    let letter = char::from(letters[(exp / 10) as usize]);
    let (mut whole, mut frac) = if exp == 0 {
        (u128::from(bytes), 0)
    } else {
        let unit = 1_u128 << exp;
        (u128::from(bytes) / unit, u128::from(bytes) % unit)
    };
    if frac > 0 {
        frac = frac * 1000 / (1_u128 << exp);
        frac = (frac + 50) / 100 * 10;
        if frac == 100 {
            whole += 1;
            frac = 0;
        }
    }
    if frac > 0 {
        let mut digits = format!("{frac:02}");
        if digits.ends_with('0') {
            digits.pop();
        }
        format!("{whole}.{digits}{letter}")
    } else {
        format!("{whole}{letter}")
    }
}

#[cfg(test)]
mod tests;
