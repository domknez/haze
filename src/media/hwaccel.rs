//! Decoding on the GPU where the platform has a decoder ffmpeg can use
//! (VideoToolbox on macOS, D3D11 on Windows, VA-API on Linux). It takes most of the
//! work off the CPU; pictures come back to memory as NV12 for the screen.

use ffmpeg::ffi;
use ffmpeg_next as ffmpeg;

/// Set up `ctx` to decode on the GPU; false when the platform can't (software then).
pub fn attach(ctx: &mut ffmpeg::codec::Context) -> bool {
    if std::env::var_os("HAZE_NO_HWACCEL").is_some() {
        return false;
    }
    let kind = if cfg!(target_os = "macos") {
        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX
    } else if cfg!(windows) {
        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA
    } else {
        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VAAPI
    };
    // SAFETY: the device reference is created here and handed to the codec context,
    // which owns it from then on.
    unsafe {
        let mut device: *mut ffi::AVBufferRef = std::ptr::null_mut();
        if ffi::av_hwdevice_ctx_create(&mut device, kind, std::ptr::null(), std::ptr::null_mut(), 0) < 0 || device.is_null() {
            log_info!("no hardware video decoder, decoding in software");
            return false;
        }
        (*ctx.as_mut_ptr()).hw_device_ctx = device;
    }
    true
}

/// A frame decoded on the GPU, copied into `out`: Some(true) when copied, Some(false)
/// when the copy failed, None when `frame` is in memory already.
pub fn download(frame: &ffmpeg::frame::Video, out: &mut ffmpeg::frame::Video) -> Option<bool> {
    // SAFETY: reading the format and, for hardware frames, copying into our frame.
    unsafe {
        let f = frame.as_ptr();
        if (*f).hw_frames_ctx.is_null() {
            return None;
        }
        ffi::av_frame_unref(out.as_mut_ptr());
        let ok = ffi::av_hwframe_transfer_data(out.as_mut_ptr(), f, 0) >= 0;
        if ok {
            ffi::av_frame_copy_props(out.as_mut_ptr(), f);
        }
        Some(ok)
    }
}
