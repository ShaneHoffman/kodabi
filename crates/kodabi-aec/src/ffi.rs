//! Hand-written bindings to the vendored speexdsp echo canceller and
//! preprocessor (`vendor/speexdsp/speex/speex_echo.h`,
//! `vendor/speexdsp/speex/speex_preprocess.h`). Only the ~10 functions
//! [`crate::EchoCanceller`] calls are declared — small enough that bindgen
//! (and the libclang/CMake toolchain it needs, see `.claude/rules` on the
//! `whisper` feature) would be more machinery than the surface warrants.

#![allow(non_camel_case_types)]

use std::ffi::{c_int, c_void};

#[repr(C)]
pub struct SpeexEchoState {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SpeexPreprocessState {
    _private: [u8; 0],
}

/// `speex_echo.h`: set the AEC's internal sampling rate (`ctl` request id).
pub const SPEEX_ECHO_SET_SAMPLING_RATE: c_int = 24;

/// `speex_preprocess.h`: `ctl` request ids this crate uses.
pub const SPEEX_PREPROCESS_SET_DENOISE: c_int = 0;
pub const SPEEX_PREPROCESS_SET_ECHO_SUPPRESS: c_int = 20;
pub const SPEEX_PREPROCESS_SET_ECHO_SUPPRESS_ACTIVE: c_int = 22;
pub const SPEEX_PREPROCESS_SET_ECHO_STATE: c_int = 24;

unsafe extern "C" {
    pub fn speex_echo_state_init(frame_size: c_int, filter_length: c_int) -> *mut SpeexEchoState;
    pub fn speex_echo_state_destroy(st: *mut SpeexEchoState);
    pub fn speex_echo_cancellation(
        st: *mut SpeexEchoState,
        rec: *const i16,
        play: *const i16,
        out: *mut i16,
    );
    pub fn speex_echo_ctl(st: *mut SpeexEchoState, request: c_int, ptr: *mut c_void) -> c_int;

    pub fn speex_preprocess_state_init(
        frame_size: c_int,
        sampling_rate: c_int,
    ) -> *mut SpeexPreprocessState;
    pub fn speex_preprocess_state_destroy(st: *mut SpeexPreprocessState);
    pub fn speex_preprocess_run(st: *mut SpeexPreprocessState, x: *mut i16) -> c_int;
    pub fn speex_preprocess_ctl(
        st: *mut SpeexPreprocessState,
        request: c_int,
        ptr: *mut c_void,
    ) -> c_int;
}
