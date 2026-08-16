#![cfg(feature = "cuda")]

use pathhydra_cuda::{embedded_ptx, validate_embedded_ptx};

#[test]
fn embedded_rust_ptx_has_the_audited_target_and_arithmetic() {
    validate_embedded_ptx().unwrap();
    let ptx = embedded_ptx();
    assert!(ptx.contains(".version 7.1"));
    assert!(!ptx.contains("fma.rn.f64"));
    assert!(!ptx.contains(".extern .func"));
    assert!(ptx.contains("atom.global.cas.b64"));
    assert!(ptx.contains("pathhydra_frontier_phase"));
    assert!(ptx.contains("pathhydra_delta_phase"));
}
