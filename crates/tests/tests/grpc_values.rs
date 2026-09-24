use php_sys::{Mode, Rapira};
use tests::{assert_case_records, captured, fixture, init_log_capture, php_lock};

// Expected values come from the Rapira\Grpc contract classes; a text metadata value must be printable ASCII (0x20-0x7E).
const CASES: &[(&str, &str)] = &[
    ("metadata rejects an empty key", "ValueError"),
    ("metadata rejects an upper-case key", "ValueError"),
    ("metadata rejects a non-ASCII key", "ValueError"),
    ("metadata rejects a non-ASCII text value", "ValueError"),
    (
        "metadata rejects a control byte in a text value",
        "ValueError",
    ),
    ("metadata accepts an empty text value", "ok"),
    ("metadata keeps any bytes under a -bin key", "ok"),
    ("metadata rejects an entry that is not a list", "TypeError"),
    ("metadata rejects a value that is not a string", "TypeError"),
    ("metadata rejects a map of values", "TypeError"),
    ("metadata rejects a sparse list", "TypeError"),
    ("values() is case-insensitive and ordered", r#"["1","2"]"#),
    ("values() of an absent key", "[]"),
    ("values() finds a numeric key", r#"["a"]"#),
    ("count() counts keys", "2"),
    ("getIterator() walks the entries", "x-a,x-b-bin"),
    (
        "method kind axes",
        "unary:00 server-streaming:01 client-streaming:10 bidi-streaming:11",
    ),
    ("grpc exception carries its status", "NotFound|nf|nf|0"),
    ("grpc exception subclass calls the parent", "Aborted"),
    (
        "context rejects a remote that is not an address",
        "TypeError",
    ),
    ("context keeps a null tls", "NULL"),
];

/// The fixture runs one PHP case per row and logs `case {name, result}`; a thrown case logs its class.
#[test]
fn value_types_follow_the_contract() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();

    let r = Rapira::start(Mode::Dispatcher(fixture("grpc/values.php")))?;
    drop(r);

    assert_case_records(CASES);
    Ok(())
}
