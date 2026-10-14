use super::super::login::parse_pasted_code;

#[test]
fn accepts_a_pasted_code_with_the_state_we_sent() {
    assert_eq!(parse_pasted_code(" abc#xyz\n", "xyz").unwrap(), "abc");
}

#[test]
fn rejects_a_pasted_code_with_another_state() {
    assert_eq!(
        parse_pasted_code("abc#other", "xyz")
            .unwrap_err()
            .to_string(),
        "OAuth state mismatch"
    );
}

#[test]
fn rejects_a_pasted_code_without_its_state() {
    let error = parse_pasted_code("abc\n", "xyz").unwrap_err();
    assert!(error.to_string().contains("whole code"), "{error}");
}
