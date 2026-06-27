use std::ffi::{c_char, c_int, c_void, CStr};
use std::slice::from_raw_parts;

use crate::{
    calc_witness, gw_status_t, GW_ERROR_CODE, GW_ERROR_CODE_ERROR, GW_ERROR_CODE_OK,
};

// The caller owns status.error_msg and must free it before reusing status.
// On allocation failure, code is still set but error_msg remains NULL.
fn prepare_status(status: *mut gw_status_t, code: GW_ERROR_CODE, error_msg: &str) {
    if !status.is_null() {
        let bs = error_msg.as_bytes();
        unsafe {
            (*status).code = code;
            (*status).error_msg = std::ptr::null_mut();
            let error_msg_ptr = libc::malloc(bs.len()+1) as *mut c_char;
            if error_msg_ptr.is_null() {
                return;
            }
            libc::memcpy(error_msg_ptr as *mut c_void, bs.as_ptr() as *mut c_void, bs.len());
            *(error_msg_ptr.add(bs.len())) = 0;
            (*status).error_msg = error_msg_ptr;
        }
    }
}

fn prepare_success_status(status: *mut gw_status_t) {
    if !status.is_null() {
        unsafe {
            (*status).code = GW_ERROR_CODE_OK;
            (*status).error_msg = std::ptr::null_mut();
        }
    }
}

/// # Safety
///
/// This function is unsafe because it dereferences raw pointers and can cause
/// undefined behavior if misused.
#[no_mangle]
pub unsafe extern "C" fn gw_calc_witness(
    inputs: *const c_char,
    graph_data: *const c_void, graph_data_len: usize,
    wtns_data: *mut *mut c_void, wtns_len: *mut usize,
    status: *mut gw_status_t) -> c_int {

    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        gw_calc_witness_inner(inputs, graph_data, graph_data_len, wtns_data, wtns_len, status)
    })) {
        Ok(result) => result,
        Err(_) => {
            prepare_status(status, GW_ERROR_CODE_ERROR, "panic while calculating witness");
            1
        }
    }
}

/// # Safety
///
/// `wtns_data` must be NULL or a pointer returned by gw_calc_witness that has
/// not already been freed.
#[no_mangle]
pub unsafe extern "C" fn gw_free_wtns_data(wtns_data: *mut c_void) {
    if !wtns_data.is_null() {
        unsafe {
            libc::free(wtns_data);
        }
    }
}

unsafe fn gw_calc_witness_inner(
    inputs: *const c_char,
    graph_data: *const c_void, graph_data_len: usize,
    wtns_data: *mut *mut c_void, wtns_len: *mut usize,
    status: *mut gw_status_t) -> c_int {

    if wtns_data.is_null() {
        prepare_status(status, GW_ERROR_CODE_ERROR, "wtns_data is null");
        return 1;
    }

    if wtns_len.is_null() {
        prepare_status(status, GW_ERROR_CODE_ERROR, "wtns_len is null");
        return 1;
    }

    unsafe {
        *wtns_data = std::ptr::null_mut();
        *wtns_len = 0;
    }

    if inputs.is_null() {
        prepare_status(status, GW_ERROR_CODE_ERROR, "inputs is null");
        return 1;
    }

    if graph_data.is_null() {
        prepare_status(status, GW_ERROR_CODE_ERROR, "graph_data is null");
        return 1;
    }

    if graph_data_len == 0 {
        prepare_status(status, GW_ERROR_CODE_ERROR, "graph_data_len is 0");
        return 1;
    }

    let graph_data_r: &[u8];
    unsafe {
        graph_data_r = from_raw_parts(graph_data as *const u8, graph_data_len);
    }


    let inputs_str: &str;
    unsafe {
        let c = CStr::from_ptr(inputs);
        match c.to_str() {
            Ok(x) => {
                inputs_str = x;
            }
            Err(e) => {
                prepare_status(
                    status, GW_ERROR_CODE_ERROR,
                    format!(
                        "Failed to parse inputs as UTF-8 string: {}",
                        e).as_str());
                return 1;
            }
        }
    }

    let witness_data = match calc_witness(inputs_str, graph_data_r) {
        Ok(witness) => witness,
        Err(e) => {
            prepare_status(status, GW_ERROR_CODE_ERROR, format!("Failed to calculate witness: {:?}", e).as_str());
            return 1;
        }
    };

    unsafe {
        let witness_ptr = libc::malloc(witness_data.len());
        if witness_ptr.is_null() && !witness_data.is_empty() {
            prepare_status(status, GW_ERROR_CODE_ERROR, "Failed to allocate memory for wtns_data");
            return 1;
        }
        if !witness_data.is_empty() {
            libc::memcpy(witness_ptr, witness_data.as_ptr() as *const c_void, witness_data.len());
        }
        *wtns_data = witness_ptr;
        *wtns_len = witness_data.len();
    }

    prepare_success_status(status);

    0
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::ffi::{CStr, CString, c_void};
    use std::ptr;
    use std::process::Command;

    fn empty_status(code: super::GW_ERROR_CODE) -> super::gw_status_t {
        super::gw_status_t {
            code,
            error_msg: ptr::null_mut(),
        }
    }

    fn take_status_message(status: &mut super::gw_status_t) -> String {
        unsafe {
            let message = CStr::from_ptr(status.error_msg).to_string_lossy().into_owned();
            libc::free(status.error_msg as *mut c_void);
            status.error_msg = ptr::null_mut();
            message
        }
    }

    fn stale_witness_data() -> *mut c_void {
        ptr::NonNull::<u8>::dangling().as_ptr() as *mut c_void
    }

    fn assert_witness_outputs_empty(wtns_data: *mut c_void, wtns_len: usize) {
        assert!(wtns_data.is_null());
        assert_eq!(wtns_len, 0);
    }

    #[test]
    fn ffi_success_sets_ok_status_and_writes_witness() {
        let inputs = CString::new(
            include_str!("../tests/vm2_setup/data/test_init_signals__inputs.json"),
        ).unwrap();
        let graph_data = include_bytes!("../tests/vm2_setup/data/test_init_signals__bc2.wcd");
        let mut wtns_data = ptr::null_mut();
        let mut wtns_len = 0;
        let mut status = empty_status(super::GW_ERROR_CODE_ERROR);

        let result = unsafe {
            super::gw_calc_witness(
                inputs.as_ptr(),
                graph_data.as_ptr() as *const c_void,
                graph_data.len(),
                &mut wtns_data,
                &mut wtns_len,
                &mut status,
            )
        };

        assert_eq!(result, 0);
        assert_eq!(status.code, super::GW_ERROR_CODE_OK);
        assert!(status.error_msg.is_null());
        assert!(!wtns_data.is_null());
        assert!(wtns_len > 0);

        let witness = unsafe {
            std::slice::from_raw_parts(wtns_data as *const u8, wtns_len)
        };
        assert!(witness.starts_with(b"wtns"));
        unsafe {
            super::gw_free_wtns_data(wtns_data);
        }
    }

    #[test]
    fn ffi_free_wtns_data_accepts_null() {
        unsafe {
            super::gw_free_wtns_data(ptr::null_mut());
        }
    }

    #[test]
    fn ffi_error_resets_outputs_for_invalid_graph_data() {
        let inputs = CString::new("{}").unwrap();
        let graph_data = [0_u8];
        let mut wtns_data = stale_witness_data();
        let mut wtns_len = usize::MAX;
        let mut status = empty_status(super::GW_ERROR_CODE_OK);

        let result = unsafe {
            super::gw_calc_witness(
                inputs.as_ptr(),
                graph_data.as_ptr() as *const c_void,
                graph_data.len(),
                &mut wtns_data,
                &mut wtns_len,
                &mut status,
            )
        };

        assert_eq!(result, 1);
        assert_eq!(status.code, super::GW_ERROR_CODE_ERROR);
        let message = take_status_message(&mut status);
        assert!(message.contains("Failed to calculate witness"));
        assert_witness_outputs_empty(wtns_data, wtns_len);
    }

    #[test]
    fn ffi_error_resets_outputs_for_invalid_input_json() {
        let inputs = CString::new("{").unwrap();
        let graph_data = include_bytes!("../tests/vm2_setup/data/test_init_signals__bc2.wcd");
        let mut wtns_data = stale_witness_data();
        let mut wtns_len = usize::MAX;
        let mut status = empty_status(super::GW_ERROR_CODE_OK);

        let result = unsafe {
            super::gw_calc_witness(
                inputs.as_ptr(),
                graph_data.as_ptr() as *const c_void,
                graph_data.len(),
                &mut wtns_data,
                &mut wtns_len,
                &mut status,
            )
        };

        assert_eq!(result, 1);
        assert_eq!(status.code, super::GW_ERROR_CODE_ERROR);
        let message = take_status_message(&mut status);
        assert!(message.contains("Failed to calculate witness"));
        assert_witness_outputs_empty(wtns_data, wtns_len);
    }

    #[test]
    fn ffi_error_resets_outputs_for_invalid_arguments() {
        let inputs = CString::new("{}").unwrap();
        let graph_data = [0_u8];
        let graph_data_ptr = graph_data.as_ptr() as *const c_void;

        let cases = [
            (
                ptr::null(),
                graph_data_ptr,
                graph_data.len(),
                "inputs is null",
            ),
            (
                inputs.as_ptr(),
                ptr::null(),
                graph_data.len(),
                "graph_data is null",
            ),
            (
                inputs.as_ptr(),
                graph_data_ptr,
                0,
                "graph_data_len is 0",
            ),
        ];

        for (inputs_ptr, graph_data_ptr, graph_data_len, expected_message) in cases {
            let mut wtns_data = stale_witness_data();
            let mut wtns_len = usize::MAX;
            let mut status = empty_status(super::GW_ERROR_CODE_OK);

            let result = unsafe {
                super::gw_calc_witness(
                    inputs_ptr,
                    graph_data_ptr,
                    graph_data_len,
                    &mut wtns_data,
                    &mut wtns_len,
                    &mut status,
                )
            };

            assert_eq!(result, 1);
            assert_eq!(status.code, super::GW_ERROR_CODE_ERROR);
            let message = take_status_message(&mut status);
            assert_eq!(message, expected_message);
            assert_witness_outputs_empty(wtns_data, wtns_len);
        }
    }

    #[test]
    fn c_example_compiles_against_public_header() {
        let cc = env::var("CC").unwrap_or_else(|_| "cc".to_string());
        let mut cc_parts = cc.split_whitespace();
        let compiler = cc_parts.next().unwrap_or("cc");
        let compiler_args: Vec<&str> = cc_parts.collect();

        match Command::new(compiler).args(&compiler_args).arg("--version").output() {
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                println!("skipping C example compile smoke test: `{}` not found", cc);
                return;
            }
            Err(err) => panic!("failed to probe C compiler `{}`: {}", cc, err),
        }

        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let object_file = tempfile::Builder::new()
            .suffix(".o")
            .tempfile()
            .expect("failed to create temporary object file");

        // Compile-only coverage keeps this focused on the public header/example
        // C surface; Rust FFI tests exercise the exported symbol itself.
        let output = Command::new(compiler)
            .args(&compiler_args)
            .arg("-std=c11")
            .arg("-Wall")
            .arg("-Wextra")
            .arg("-Werror")
            .arg("-I")
            .arg(manifest_dir.join("include"))
            .arg("-c")
            .arg(manifest_dir.join("examples/calc_witness.c"))
            .arg("-o")
            .arg(object_file.path())
            .output()
            .expect("failed to compile C example");

        assert!(
            output.status.success(),
            "failed to compile C example:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn ffi_rejects_null_wtns_data() {
        let inputs = CString::new("{}").unwrap();
        let graph_data = [0_u8];
        let mut wtns_len = usize::MAX;
        let mut status = empty_status(super::GW_ERROR_CODE_OK);

        let result = unsafe {
            super::gw_calc_witness(
                inputs.as_ptr(),
                graph_data.as_ptr() as *const c_void,
                graph_data.len(),
                ptr::null_mut(),
                &mut wtns_len,
                &mut status,
            )
        };

        assert_eq!(result, 1);
        assert_eq!(status.code, super::GW_ERROR_CODE_ERROR);
        let message = take_status_message(&mut status);
        assert_eq!(message, "wtns_data is null");
        assert_eq!(wtns_len, usize::MAX);
    }

    #[test]
    fn ffi_rejects_null_wtns_len() {
        let inputs = CString::new("{}").unwrap();
        let graph_data = [0_u8];
        let mut wtns_data = stale_witness_data();
        let mut status = empty_status(super::GW_ERROR_CODE_OK);

        let result = unsafe {
            super::gw_calc_witness(
                inputs.as_ptr(),
                graph_data.as_ptr() as *const c_void,
                graph_data.len(),
                &mut wtns_data,
                ptr::null_mut(),
                &mut status,
            )
        };

        assert_eq!(result, 1);
        assert_eq!(status.code, super::GW_ERROR_CODE_ERROR);
        let message = take_status_message(&mut status);
        assert_eq!(message, "wtns_len is null");
        assert_eq!(wtns_data, stale_witness_data());
    }

    #[test]
    fn ffi_converts_internal_panic_to_error_status() {
        let inputs = CString::new("{}").unwrap();
        let graph_data = b"wtns.graph.002";
        let mut wtns_data = stale_witness_data();
        let mut wtns_len = usize::MAX;
        let mut status = empty_status(super::GW_ERROR_CODE_OK);

        let result = unsafe {
            super::gw_calc_witness(
                inputs.as_ptr(),
                graph_data.as_ptr() as *const c_void,
                graph_data.len(),
                &mut wtns_data,
                &mut wtns_len,
                &mut status,
            )
        };

        assert_eq!(result, 1);
        assert_eq!(status.code, super::GW_ERROR_CODE_ERROR);
        let message = take_status_message(&mut status);
        assert_eq!(message, "panic while calculating witness");
        assert_witness_outputs_empty(wtns_data, wtns_len);
    }
}
