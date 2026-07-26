use super::*;

#[test]
fn lsa_reports_a_missing_api_key_without_starting_a_request() {
    let fixture = SourceFixture::new("(module sample)\n");
    let output = Command::new(env!("CARGO_BIN_EXE_osr"))
        .args(["lsa", "Explain reduce"])
        .current_dir(&fixture.directory)
        .env_remove("OSR_API_KEY")
        .env_remove("OSR_BASE_URL")
        .env_remove("OSR_MODEL")
        .env_remove("OSR_WIRE_API")
        .output()
        .expect("osr lsa should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("OSR_API_KEY is not set"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
