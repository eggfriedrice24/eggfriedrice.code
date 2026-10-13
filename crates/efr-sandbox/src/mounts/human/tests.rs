//! The number formats of `df` and `findmnt`.

use pretty_assertions::assert_eq;

use super::*;

const H: Human =
    Human { autoscale: true, si: true, base_1024: true, bytes: false, suppress_point_zero: false };
const SI: Human =
    Human { autoscale: true, si: true, base_1024: false, bytes: false, suppress_point_zero: false };

#[test]
fn human_sizes_round_up_with_one_decimal_under_ten() {
    let cases: &[(u64, &str)] = &[
        (0, "0"),
        (1023, "1023"),
        (1024, "1.0K"),
        (1025, "1.1K"),
        (1536, "1.5K"),
        (10 * 1024, "10K"),
        (10 * 1024 + 1, "11K"),
        // 9.95K rounds up past 9.9 to 10.
        (10_189, "10K"),
        (1024 * 1024 - 1, "1.0M"),
        (1024 * 1024, "1.0M"),
        (1023 * 1024 * 1024, "1023M"),
        (1023 * 1024 * 1024 + 1, "1.0G"),
        (1_048_575 * 1024 + 1, "1.0G"),
        (u64::MAX, "16E"),
    ];
    for (bytes, text) in cases {
        assert_eq!(human_readable(*bytes, H, 1, 1), *text, "{bytes}");
    }
    assert_eq!(human_readable(1000, SI, 1, 1), "1.0k");
    assert_eq!(human_readable(1_500_000, SI, 1, 1), "1.5M");
    // Blocks of one size in units of another, rounded up.
    assert_eq!(human_readable(3, Human::default(), 4096, 1024), "12");
    assert_eq!(human_readable(3, Human::default(), 512, 1024), "2");
    assert_eq!(human_readable(3, Human::default(), 4096, 1000), "13");
}

#[test]
fn a_unit_with_no_number_prints_its_suffix() {
    let mega = Human { si: true, base_1024: true, ..Human::default() };
    assert_eq!(human_readable(256, mega, 4096, 1024 * 1024), "1M");
    let kilo_bytes = Human { si: true, bytes: true, ..Human::default() };
    assert_eq!(human_readable(1, kilo_bytes, 4096, 1000), "5kB");
    let kibi = Human { si: true, bytes: true, base_1024: true, ..Human::default() };
    assert_eq!(human_readable(1, kibi, 4096, 1024), "4KiB");
}

#[test]
fn percent_rounds_up_and_is_a_dash_with_nothing() {
    assert_eq!(percent(0, 0), "-");
    assert_eq!(percent(0, 10), "0%");
    assert_eq!(percent(1, 999), "1%");
    assert_eq!(percent(1, 1), "50%");
    assert_eq!(percent(10, 0), "100%");
}

#[test]
fn util_linux_sizes_have_one_decimal_rounded_to_nearest() {
    let cases: &[(u64, &str)] = &[
        (1, "1B"),
        (1023, "1023B"),
        (1024, "1K"),
        (1536, "1.5K"),
        (1024 * 1024 * 1024 * 31, "31G"),
        (1_073_741_824 * 31 - 1, "31G"),
        (5_000_000, "4.8M"),
        (1 << 60, "1E"),
    ];
    for (bytes, text) in cases {
        assert_eq!(util_linux_size(*bytes), *text, "{bytes}");
    }
}
