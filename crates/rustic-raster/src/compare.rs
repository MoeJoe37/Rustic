//! Pixel comparison for reftests (WPT-style fuzzy matching).

use crate::Surface;

/// How two same-sized surfaces differ.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Difference {
    /// Pixels where any channel differs.
    pub pixels: u64,
    /// Largest absolute difference of any channel, 0–255.
    pub max_channel: u8,
}

/// Compare two surfaces channel by channel; `None` if their sizes differ.
pub fn compare(a: &Surface, b: &Surface) -> Option<Difference> {
    if (a.width(), a.height()) != (b.width(), b.height()) {
        return None;
    }
    let mut difference = Difference::default();
    for (p, q) in a
        .premultiplied()
        .chunks_exact(4)
        .zip(b.premultiplied().chunks_exact(4))
    {
        let max = p
            .iter()
            .zip(q)
            .map(|(x, y)| x.abs_diff(*y))
            .max()
            .unwrap_or(0);
        if max > 0 {
            difference.pixels += 1;
            difference.max_channel = difference.max_channel.max(max);
        }
    }
    Some(difference)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RasterLimits;

    #[test]
    fn counts_pixels_and_largest_channel_difference() {
        let a = Surface::new(2, 2, RasterLimits::default()).unwrap();
        let mut b = a.clone();
        assert_eq!(compare(&a, &b), Some(Difference::default()));
        b.pixels_mut()[0] = 3;
        b.pixels_mut()[1] = 9;
        b.pixels_mut()[12] = 1;
        assert_eq!(
            compare(&a, &b),
            Some(Difference {
                pixels: 2,
                max_channel: 9
            })
        );
        let c = Surface::new(1, 2, RasterLimits::default()).unwrap();
        assert_eq!(compare(&a, &c), None);
    }
}
