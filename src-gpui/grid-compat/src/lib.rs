//! Re-export the fixed upstream implementation for Taffy's grid 0.18 requirement.
//! No upstream grid 0.18 code is included. grid 1.0.1 fixes CVE-2026-42199.

pub use grid_fixed::*;

#[cfg(test)]
mod tests {
    #[test]
    fn overflow_is_rejected_before_corrupting_dimensions() {
        let result = std::panic::catch_unwind(|| {
            let mut grid = super::Grid::from_vec(vec![1_u8, 2], 2);
            grid.expand_rows(usize::MAX / 2);
        });
        assert!(result.is_err());
    }
}
