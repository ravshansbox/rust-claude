use super::{HeldPanics, InputModes, notify};
use std::sync::{Mutex, PoisonError};

/// Tests that change the process-wide panic hook take turns.
static PANIC_HOOK: Mutex<()> = Mutex::new(());

#[test]
fn turns_input_modes_off_after_a_panic() {
    let mut output = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _modes = InputModes::enable(&mut output).unwrap();
        panic!("drawing failed");
    }));
    assert!(result.is_err());
    let written = String::from_utf8_lossy(&output);
    let (enabled, disabled) = written.split_at(written.find("\x1b[<1u").unwrap());
    assert!(enabled.contains("\x1b[?1000h") && enabled.contains("\x1b[?2004h"));
    assert!(enabled.contains("\x1b[?1004h"), "{written:?}");
    assert!(
        disabled.contains("\x1b[?1004l"),
        "focus reporting still on: {written:?}"
    );
    assert!(
        disabled.contains("\x1b[?1000l"),
        "mouse capture still on: {written:?}"
    );
    assert!(
        disabled.contains("\x1b[?2004l"),
        "bracketed paste still on: {written:?}"
    );
}

#[test]
fn notifies_with_an_osc_9_sequence_without_control_characters() {
    let mut output = Vec::new();
    notify(&mut output, "Failed: \x1b]52;c;aGk=\x07bad\u{9c}", false).unwrap();
    assert_eq!(output, b"\x1b]9;Failed: ]52;c;aGk=bad\x07");
}

#[test]
fn wraps_the_notification_for_tmux_passthrough() {
    let mut output = Vec::new();
    notify(&mut output, "Done", true).unwrap();
    assert_eq!(output, b"\x1bPtmux;\x1b\x1b]9;Done\x07\x1b\\");
}

/// Panics on a thread named "agent" while the interface holds panics.
fn agent_panics() {
    let result = std::thread::Builder::new()
        .name("agent".into())
        .spawn(|| panic!("the agent broke"))
        .unwrap()
        .join();
    assert!(result.is_err());
}

fn shows_agent_panic(output: &[u8]) -> bool {
    // Other tests may panic on their own threads meanwhile.
    let output = String::from_utf8_lossy(output);
    output.contains("thread 'agent' panicked at src/tui/terminal/tests.rs:")
        && output.contains("the agent broke\n")
}

#[test]
fn holds_panics_from_other_threads_until_released() {
    let _turn = PANIC_HOOK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut output = Vec::new();
    let held = HeldPanics::install(&mut output);
    agent_panics();
    drop(held);
    assert!(
        shows_agent_panic(&output),
        "{}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn shows_held_panics_and_passes_on_later_ones_after_the_interface_panics() {
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let _turn = PANIC_HOOK.lock().unwrap_or_else(PoisonError::into_inner);
    std::panic::set_hook(Box::new(|info| {
        SEEN.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(info.to_string());
    }));
    let mut output = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _held = HeldPanics::install(&mut output);
        agent_panics();
        panic!("the interface broke");
    }));
    assert!(result.is_err());
    let _ = std::thread::spawn(|| panic!("a later crash")).join();
    drop(std::panic::take_hook());
    assert!(
        shows_agent_panic(&output),
        "{}",
        String::from_utf8_lossy(&output)
    );
    let seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(
        seen.iter()
            .any(|message| message.contains("the interface broke")),
        "{seen:?}"
    );
    assert!(
        seen.iter().any(|message| message.contains("a later crash")),
        "{seen:?}"
    );
}
