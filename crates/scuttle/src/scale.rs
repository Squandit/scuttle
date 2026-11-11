//! The interface size setting, for widget code.
//!
//! Sizes in the stylesheet are scaled when it is generated. Sizes set
//! from code (spacing, widths, icon sizes) go through [`px`] instead.

use std::cell::Cell;

thread_local! {
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
}

/// Sets the scale used by [`px`]. Called when the config is loaded.
pub fn set(scale: f32) {
    SCALE.with(|s| s.set(scale));
}

/// A base size in pixels at the current scale.
pub fn px(base: i32) -> i32 {
    scuttle_core::css::scaled(base as f32, SCALE.with(Cell::get))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_follow_the_scale() {
        set(1.0);
        assert_eq!(px(40), 40);
        set(1.5);
        assert_eq!(px(40), 60);
        assert_eq!(px(1), 1, "hairlines stay one pixel");
        assert_eq!(px(0), 0);
        set(1.0);
    }
}
