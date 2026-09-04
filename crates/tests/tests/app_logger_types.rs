use tests::app_record;
use tracing::Level;

/// Nonfinite floats must convert to "INF", "-INF", or "NaN" and must not encode as `0`.
#[test]
#[ignore = "needs the context normalizer (Monolog NormalizerFormatter parity)"]
fn special_floats_keep_their_meaning() {
    let (_, _, ctx) = app_record("app_logger/types-scalars.php");

    for fragment in [r#""inf":"INF""#, r#""ninf":"-INF""#, r#""nan":"NaN""#] {
        assert!(
            ctx.contains(fragment),
            "a non-finite float must not become a number: no {fragment} in {ctx:?}"
        );
    }
}

/// Invalid encoded bytes become U+FFFD without replacing the complete value with null.
#[test]
#[ignore = "needs the context normalizer (Monolog NormalizerFormatter parity)"]
fn invalid_utf8_is_substituted_not_dropped() {
    let (_, _, ctx) = app_record("app_logger/types-scalars.php");

    assert!(
        ctx.contains(r#""bad_utf8":"\u{fffd}1""#) || ctx.contains(r#""bad_utf8":"\\ufffd1""#),
        "bad bytes must degrade to U+FFFD, keeping the rest of the string: {ctx:?}"
    );
}

/// An object includes its class name and properties in the log.
#[test]
#[ignore = "needs the context normalizer (Monolog NormalizerFormatter parity)"]
fn objects_keep_their_class_name() {
    let (_, _, ctx) = app_record("app_logger/types-objects.php");

    assert!(
        ctx.contains(r#""plain":{"PlainNorm":{"foo":"fooValue"}}"#),
        "an object must carry its class: {ctx:?}"
    );
}

/// An object with only __toString converts to that string.
#[test]
#[ignore = "needs the context normalizer (Monolog NormalizerFormatter parity)"]
fn stringable_objects_use_their_string_form() {
    let (_, _, ctx) = app_record("app_logger/types-objects.php");

    assert!(
        ctx.contains(r#""stringable":"bar""#),
        "__toString is the only meaningful view of such an object: {ctx:?}"
    );
}

/// A resource is formatted as `[resource(stream)]` so it is different from null.
#[test]
#[ignore = "needs the context normalizer (Monolog NormalizerFormatter parity)"]
fn resources_render_as_a_type_marker() {
    let (_, _, ctx) = app_record("app_logger/types-objects.php");

    assert!(
        ctx.contains(r#""res":"[resource(stream)]""#),
        "a resource must be distinguishable from null: {ctx:?}"
    );
}

/// An exception from __toString must not remove adjacent keys or add the exception to the record.
#[test]
fn a_throwing_tostring_cannot_break_logging() {
    let (level, msg, ctx) = app_record("app_logger/types-objects.php");

    assert_eq!(level, Level::ERROR);
    assert_eq!(msg, "objects");
    assert!(
        ctx.contains(r#""boom""#),
        "the throwing key must still be present: {ctx:?}"
    );
    assert!(
        ctx.contains(r#""keep":"visible""#),
        "a throwing __toString must not cost its siblings: {ctx:?}"
    );
    assert!(
        !ctx.contains("Could not convert to string"),
        "the exception must not escape into the record: {ctx:?}"
    );
}
