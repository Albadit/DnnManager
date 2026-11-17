//! Whether the system wants animations: Windows' "Animation effects" (Settings > Accessibility > Visual effects).

/// False when Windows' animation effects are off; true elsewhere and when it can't be read.
#[cfg(windows)]
pub fn system_animations() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    };
    let mut on = windows::core::BOOL(1);
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL into the pointer passed.
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some(&mut on as *mut _ as *mut std::ffi::c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_err() || on.as_bool()
}

#[cfg(not(windows))]
pub fn system_animations() -> bool {
    true
}
