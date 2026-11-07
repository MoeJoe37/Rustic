use std::process::Command;

#[test]
fn reports_foundation_status_without_claiming_rendering() {
    let output = Command::new(env!("CARGO_BIN_EXE_rustic"))
        .arg("info")
        .output()
        .expect("run rustic");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("not implemented"));
}

#[test]
fn rejects_navigation_and_extra_arguments() {
    for args in [vec!["https://example.com"], vec!["info", "unexpected"]] {
        let output = Command::new(env!("CARGO_BIN_EXE_rustic"))
            .args(args)
            .output()
            .expect("run rustic");
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("not implemented"));
    }
}
