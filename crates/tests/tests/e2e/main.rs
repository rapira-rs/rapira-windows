mod concurrency;
mod examples;
mod harness;
mod ini;
mod lifecycle;
mod logging;
mod static_files;
mod streaming;
mod timeout;

#[test]
fn a_console_control_event_reaches_the_child_process_group() {
    harness::assert_console_delivery();
}
