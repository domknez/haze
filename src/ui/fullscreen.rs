//! Full screen on macOS without the menu bar and the Dock. The system only auto-hides
//! them by default, so a pointer near the top of the screen brings the menu bar and the
//! window's title bar back over the picture. AppKit asks the window's delegate which
//! options to use, however full screen was entered (the green button too), and the
//! delegate is winit's, with no setting for this: its answer gets replaced here.

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{ffi, sel};

// NSApplicationPresentationOptions. Auto-hide and hide exclude each other.
const AUTO_HIDE_DOCK: usize = 1 << 0;
const HIDE_DOCK: usize = 1 << 1;
const AUTO_HIDE_MENU_BAR: usize = 1 << 2;
const HIDE_MENU_BAR: usize = 1 << 3;

fn without_bars(proposed: usize) -> usize {
    proposed & !(AUTO_HIDE_DOCK | AUTO_HIDE_MENU_BAR) | HIDE_DOCK | HIDE_MENU_BAR
}

type Answer = unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, usize) -> usize;

unsafe extern "C-unwind" fn presentation_options(_: *mut AnyObject, _: Sel, _: *mut AnyObject, proposed: usize) -> usize {
    without_bars(proposed)
}

/// Call once the window exists: winit registers its delegate class with the first one.
pub fn hide_bars() {
    let Some(delegate) = AnyClass::get(c"WinitWindowDelegate") else {
        log_warn!("full screen: no window delegate, the menu bar will only auto-hide");
        return;
    };
    // SAFETY: the signature is that of window:willUseFullScreenPresentationOptions:,
    // and "Q@:@Q" spells it out. The runtime takes any IMP as a plain function pointer.
    unsafe {
        let answer: Imp = std::mem::transmute(presentation_options as Answer);
        let selector = sel!(window:willUseFullScreenPresentationOptions:);
        ffi::class_replaceMethod(std::ptr::from_ref(delegate).cast_mut(), selector, answer, c"Q@:@Q".as_ptr());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bars_hide_instead_of_auto_hiding() {
        const FULL_SCREEN: usize = 1 << 10;
        const AUTO_HIDE_TOOLBAR: usize = 1 << 11;
        let proposed = FULL_SCREEN | AUTO_HIDE_DOCK | AUTO_HIDE_MENU_BAR | AUTO_HIDE_TOOLBAR;
        assert_eq!(without_bars(proposed), FULL_SCREEN | HIDE_DOCK | HIDE_MENU_BAR | AUTO_HIDE_TOOLBAR);
    }
}
