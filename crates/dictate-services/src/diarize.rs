//! Speaker separation: who speaks when, worked out offline with sherpa-onnx (pyannote segmentation, then a speaker
//! embedding per segment, then clustering).

#[cfg(test)]
mod tests {
    use std::ffi::CStr;

    #[test]
    fn the_linked_sherpa_onnx_is_the_pinned_version() {
        // Reads the version of the native library that was linked, not of the crate: a library dir that holds
        // another release (SHERPA_ONNX_LIB_DIR in CI) would be caught here.
        let version = unsafe { CStr::from_ptr(sherpa_onnx_sys::SherpaOnnxGetVersionStr()) };
        assert_eq!(version.to_str().unwrap(), "1.13.8");
    }
}
