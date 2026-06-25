// C FFI type bindings for `include/graph_witness.h`.
//
// The binding surface is a single enum and struct; keep them in sync with the
// header by hand. (Originally produced by rust-bindgen 0.72.0.)

pub const GW_ERROR_CODE_OK: GW_ERROR_CODE = 0;
pub const GW_ERROR_CODE_ERROR: GW_ERROR_CODE = 1;
pub type GW_ERROR_CODE = ::std::os::raw::c_uint;
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct gw_status_t {
    pub code: GW_ERROR_CODE,
    pub error_msg: *mut ::std::os::raw::c_char,
}

#[cfg(test)]
mod binding_layout_tests {
    use super::{gw_status_t, GW_ERROR_CODE};
    use std::mem::{align_of, offset_of, size_of};

    #[test]
    fn gw_status_t_layout_matches_c_header() {
        assert_eq!(offset_of!(gw_status_t, code), 0);
        assert_eq!(size_of::<GW_ERROR_CODE>(), 4);
        assert_eq!(offset_of!(gw_status_t, error_msg), size_of::<usize>());
        assert_eq!(align_of::<gw_status_t>(), align_of::<usize>());
        assert_eq!(size_of::<gw_status_t>(), size_of::<usize>() * 2);
    }
}
