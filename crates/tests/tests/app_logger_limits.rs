use tests::app_record;
use tracing::Level;

/// Cyclic references stop at the reference to an earlier value and do not produce a PHP diagnostic.
#[test]
fn cycles_are_broken_without_a_diagnostic() {
    let (level, _, ctx) = app_record("app_logger/limits-cycles.php");

    assert_eq!(level, Level::ERROR);
    assert!(
        ctx.contains(r#""objects":{"bar":{"foo":null}}"#),
        "object cycle must be cut at the back-edge: {ctx:?}"
    );
    assert!(
        ctx.contains(r#""arrays":{"x":{"foo":"bar","y":null}}"#),
        "reference cycle must be cut at the back-edge: {ctx:?}"
    );
    assert!(
        ctx.contains(r#""keep":"visible""#),
        "siblings of a cycle must survive: {ctx:?}"
    );
    let phpdiag: Vec<_> = tests::captured()
        .iter()
        .filter(|c| c.target == "php")
        .map(|c| c.message.clone())
        .collect();
    assert!(phpdiag.is_empty(), "cycles must raise nothing: {phpdiag:?}");
}

/// A large array retains its last item.
#[test]
fn a_large_array_is_complete() {
    let (_, _, ctx) = app_record("app_logger/limits-large-array.php");

    assert!(
        ctx.contains(",1000]"),
        "the last item must survive: {ctx:?}"
    );
}
