#[test]
fn admission_diagnostics_are_stable_compile_artifacts() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/admission_ok.rs");
    tests.compile_fail("tests/ui/invalid_deadline.rs");
    tests.compile_fail("tests/ui/all_codes.rs");
}
