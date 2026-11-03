//! The order-preserving event key (`brauser_events.order_key`, cache
//! format 9).
//!
//! The key is a byte string whose `memcmp` order equals the
//! replay tuple order `(instant, position.is_none(), position,
//! sequence or u64::MAX, id)`, so SQLite can order and compare events by an
//! indexed BLOB column. It is concatenated from:
//!
//! 1. the instant as i128 nanoseconds since the Unix epoch, big-endian with
//!    the sign bit flipped (the offset is ignored, as `OffsetDateTime`'s own
//!    order ignores it);
//! 2. one byte: `0` when there is a position, `1` when there is none;
//! 3. when there is a position, its path components, each with `0x00`
//!    escaped as `0x00 0xFF`, separated by `0x00 0x01` and terminated by
//!    `0x00 0x00`, then the line as u64 big-endian;
//! 4. the sequence as u64 big-endian (`u64::MAX` when absent);
//! 5. the event ID bytes.
//!
//! Positions are log-dir-relative, so their components are normal names.
//! A non-normal component (`.` leading, `..`, a root or a Windows prefix)
//! is written as a reserved `0x00 tag` pair, which sorts before every normal
//! name exactly as `std::path::Component`'s order puts it, and keeps the
//! encoding injective. (Two different Windows prefixes are ordered by their
//! bytes, not by `Prefix`'s variant order; positions never carry one.) Names are compared through
//! `OsStr::as_encoded_bytes`, whose byte order is `OsStr`'s order on every
//! platform (raw bytes on Unix, WTF-8 on Windows).

// Consumed by the SQL store (T8); until then only the tests call it.
#![allow(dead_code)]

use std::path::{Component, Path};

use time::OffsetDateTime;

const ESCAPE: u8 = 0x00;
const ESCAPED_NUL: u8 = 0xFF;
const SEPARATOR: u8 = 0x01;
const TERMINATOR: u8 = 0x00;
// Reserved tags for non-normal components, ordered like `Component`:
// Prefix < RootDir < CurDir < ParentDir < Normal.
const PREFIX: u8 = 0x02;
const ROOT_DIR: u8 = 0x03;
const CUR_DIR: u8 = 0x04;
const PARENT_DIR: u8 = 0x05;

/// The memcmp-ordered key of one event; see the module documentation.
pub(super) fn order_key(
    instant: OffsetDateTime,
    position: Option<(&Path, u64)>,
    sequence: Option<u64>,
    id: &str,
) -> Vec<u8> {
    let mut key = Vec::with_capacity(16 + 1 + 64 + 8 + 8 + id.len());
    let nanos = instant.unix_timestamp_nanos() as u128 ^ (1 << 127);
    key.extend_from_slice(&nanos.to_be_bytes());
    match position {
        Some((path, line)) => {
            key.push(0);
            push_path(&mut key, path);
            key.extend_from_slice(&line.to_be_bytes());
        }
        None => key.push(1),
    }
    key.extend_from_slice(&sequence.unwrap_or(u64::MAX).to_be_bytes());
    key.extend_from_slice(id.as_bytes());
    key
}

fn push_path(key: &mut Vec<u8>, path: &Path) {
    for (index, component) in path.components().enumerate() {
        if index > 0 {
            key.extend_from_slice(&[ESCAPE, SEPARATOR]);
        }
        let (tag, bytes) = match component {
            Component::Normal(name) => (None, name.as_encoded_bytes()),
            Component::Prefix(prefix) => (Some(PREFIX), prefix.as_os_str().as_encoded_bytes()),
            Component::RootDir => (Some(ROOT_DIR), &[][..]),
            Component::CurDir => (Some(CUR_DIR), &[][..]),
            Component::ParentDir => (Some(PARENT_DIR), &[][..]),
        };
        if let Some(tag) = tag {
            key.extend_from_slice(&[ESCAPE, tag]);
        }
        for &byte in bytes {
            if byte == ESCAPE {
                key.extend_from_slice(&[ESCAPE, ESCAPED_NUL]);
            } else {
                key.push(byte);
            }
        }
    }
    key.extend_from_slice(&[ESCAPE, TERMINATOR]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::path::PathBuf;
    use time::UtcOffset;

    /// The replay sort key the JSON cache used, field for field.
    type Tuple<'a> = (
        OffsetDateTime,
        bool,
        Option<&'a (PathBuf, u64)>,
        u64,
        &'a String,
    );

    struct Sample {
        instant: OffsetDateTime,
        position: Option<(PathBuf, u64)>,
        sequence: Option<u64>,
        id: String,
    }

    impl Sample {
        fn tuple(&self) -> Tuple<'_> {
            (
                self.instant,
                self.position.is_none(),
                self.position.as_ref(),
                self.sequence.unwrap_or(u64::MAX),
                &self.id,
            )
        }

        fn key(&self) -> Vec<u8> {
            order_key(
                self.instant,
                self.position
                    .as_ref()
                    .map(|(path, line)| (path.as_path(), *line)),
                self.sequence,
                &self.id,
            )
        }
    }

    /// xorshift64*: deterministic, so a failure reproduces from its seed.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    // Small pools force ties, so every later field is exercised.
    const NAME_PARTS: &[&str] = &[
        "",
        "-",
        ".",
        "..",
        "/",
        "\0",
        "\u{1}",
        "a",
        "b",
        "2026-11-02.md",
        "a.md",
        "a-b",
        "a/b",
        "é",
        "\u{ffff}",
        "\0\u{1}",
        "0",
        "z",
    ];
    const IDS: &[&str] = &["", "a", "a\0", "ab", "b", "\0", "é", "event-1", "event-10"];

    fn random_path(rng: &mut Rng) -> PathBuf {
        let mut text = String::new();
        for _ in 0..rng.below(5) {
            text.push_str(rng.pick(NAME_PARTS));
        }
        PathBuf::from(text)
    }

    fn random_instant(rng: &mut Rng, pool: &[i128]) -> OffsetDateTime {
        let nanos = if rng.below(4) == 0 {
            // Anywhere in the representable range, negative included.
            let span = 253_402_300_799_i128 * 1_000_000_000;
            (i128::from(rng.next()) << 64 | i128::from(rng.next())) % span
        } else {
            *rng.pick(pool)
        };
        let offset = UtcOffset::from_whole_seconds(rng.below(2 * 50_400) as i32 - 50_400).unwrap();
        OffsetDateTime::from_unix_timestamp_nanos(nanos)
            .unwrap()
            .to_offset(offset)
    }

    fn random_sample(rng: &mut Rng, instants: &[i128]) -> Sample {
        let position = (rng.below(3) != 0).then(|| {
            let line = *rng.pick(&[0, 1, 2, 255, 256, u64::MAX - 1, u64::MAX]);
            (random_path(rng), line)
        });
        let sequence = (rng.below(4) != 0).then(|| *rng.pick(&[0, 1, 7, 256, u64::MAX - 1]));
        Sample {
            instant: random_instant(rng, instants),
            position,
            sequence,
            id: (*rng.pick(IDS)).to_owned(),
        }
    }

    #[test]
    fn sorting_by_key_equals_the_ordered_events_tuple_sort() {
        let instants = [
            -1_000_000_000_000,
            -1,
            0,
            1,
            1_793_577_600_000_000_000,
            1_793_577_600_000_000_001,
        ];
        for seed in 1..=200_u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let samples = (0..60)
                .map(|_| random_sample(&mut rng, &instants))
                .collect::<Vec<_>>();
            let keys = samples.iter().map(Sample::key).collect::<Vec<_>>();
            for (i, left) in samples.iter().enumerate() {
                for (j, right) in samples.iter().enumerate() {
                    assert_eq!(
                        keys[i].cmp(&keys[j]),
                        left.tuple().cmp(&right.tuple()),
                        "seed {seed}: {:?} vs {:?}",
                        left.tuple(),
                        right.tuple(),
                    );
                }
            }
            let mut by_tuple = (0..samples.len()).collect::<Vec<_>>();
            by_tuple.sort_by(|&a, &b| samples[a].tuple().cmp(&samples[b].tuple()));
            let mut by_key = (0..samples.len()).collect::<Vec<_>>();
            by_key.sort_by(|&a, &b| keys[a].cmp(&keys[b]));
            let sorted =
                |order: &[usize]| order.iter().map(|&i| keys[i].clone()).collect::<Vec<_>>();
            assert_eq!(sorted(&by_key), sorted(&by_tuple), "seed {seed}");
        }
    }

    #[test]
    fn equal_tuples_give_equal_keys_and_different_tuples_different_keys() {
        let mut rng = Rng(0x5EED);
        let instants = [0, 1];
        let samples = (0..400)
            .map(|_| random_sample(&mut rng, &instants))
            .collect::<Vec<_>>();
        for left in &samples {
            for right in &samples {
                assert_eq!(
                    left.key() == right.key(),
                    left.tuple() == right.tuple(),
                    "{:?} vs {:?}",
                    left.tuple(),
                    right.tuple(),
                );
            }
        }
    }

    #[test]
    fn negative_instants_sort_before_the_epoch_and_by_magnitude() {
        let key = |nanos: i128| {
            order_key(
                OffsetDateTime::from_unix_timestamp_nanos(nanos).unwrap(),
                None,
                None,
                "id",
            )
        };
        let ordered = [
            OffsetDateTime::new_utc(time::Date::MIN, time::Time::MIDNIGHT).unix_timestamp_nanos(),
            -86_400_000_000_000,
            -1_000_000_000,
            -2,
            -1,
            0,
            1,
            1_000_000_000,
            OffsetDateTime::new_utc(time::Date::MAX, time::Time::MIDNIGHT).unix_timestamp_nanos(),
        ];
        for pair in ordered.windows(2) {
            assert_eq!(key(pair[0]).cmp(&key(pair[1])), Ordering::Less, "{pair:?}");
        }
        // The sign bit is flipped: -1 ns starts 0x7F..., 0 ns starts 0x80 00....
        assert_eq!(key(-1)[..16], [&[0x7F_u8][..], &[0xFF; 15]].concat()[..]);
        assert_eq!(key(0)[..16], [&[0x80_u8][..], &[0x00; 15]].concat()[..]);
    }

    #[test]
    fn the_same_instant_at_another_offset_gives_the_same_key() {
        let utc = OffsetDateTime::from_unix_timestamp(1_793_577_600).unwrap();
        let shifted = utc.to_offset(UtcOffset::from_hms(-7, 0, 0).unwrap());
        let path = Path::new("2026/2026-11-02.md");
        assert_eq!(
            order_key(utc, Some((path, 3)), Some(9), "e"),
            order_key(shifted, Some((path, 3)), Some(9), "e"),
        );
    }

    #[test]
    fn the_layout_is_the_documented_one() {
        let at = OffsetDateTime::UNIX_EPOCH;
        let instant = [&[0x80_u8][..], &[0x00; 15]].concat();
        let with_position = order_key(at, Some((Path::new("a\0b/c"), 2)), None, "id");
        let expected = [
            &instant[..],
            &[0],
            b"a\x00\xFFb\x00\x01c\x00\x00",
            &2_u64.to_be_bytes(),
            &u64::MAX.to_be_bytes(),
            b"id",
        ]
        .concat();
        assert_eq!(with_position, expected);
        let without = order_key(at, None, Some(5), "id");
        let expected = [&instant[..], &[1], &5_u64.to_be_bytes(), b"id"].concat();
        assert_eq!(without, expected);
    }
}
