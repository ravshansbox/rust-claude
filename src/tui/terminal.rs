use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
};
use std::{
    backtrace::{Backtrace, BacktraceStatus},
    io::{self, Write},
    panic::PanicHookInfo,
    sync::{Arc, Mutex, PoisonError},
};

/// Mouse capture, bracketed paste and keyboard flags, turned off when dropped
/// so a panic does not leave them on in the user's shell.
pub(super) struct InputModes<W: Write> {
    output: W,
    keyboard_enhanced: bool,
}

impl<W: Write> InputModes<W> {
    pub(super) fn enable(mut output: W) -> io::Result<Self> {
        execute!(
            output,
            EnableMouseCapture,
            EnableBracketedPaste,
            EnableFocusChange
        )?;
        let keyboard_enhanced = execute!(
            output,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
        Ok(Self {
            output,
            keyboard_enhanced,
        })
    }
}

impl<W: Write> Drop for InputModes<W> {
    fn drop(&mut self) {
        if self.keyboard_enhanced {
            let _ = execute!(self.output, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(
            self.output,
            DisableMouseCapture,
            DisableBracketedPaste,
            DisableFocusChange
        );
    }
}

type PanicHook = dyn Fn(&PanicHookInfo<'_>) + Send + Sync;

/// Holds back panic messages from other threads while the interface owns the
/// terminal. ratatui's hook would restore the terminal from the panicking
/// thread, and the interface could then draw over the shell and the message.
/// The interface notices a crashed agent, restores the terminal itself, and
/// then drops this, which writes the held messages to `output`. A panic on the
/// interface thread goes to ratatui's hook, which restores the terminal before
/// unwinding drops this.
pub(super) struct HeldPanics<W: Write> {
    /// `None` once released, so later panics go to the previous hook.
    messages: Arc<Mutex<Option<Vec<String>>>>,
    previous: Arc<PanicHook>,
    output: W,
}

impl<W: Write> HeldPanics<W> {
    /// Panics on the calling thread still go to the previous hook.
    pub(super) fn install(output: W) -> Self {
        let messages = Arc::new(Mutex::new(Some(Vec::new())));
        let previous: Arc<PanicHook> = Arc::from(std::panic::take_hook());
        let interface = std::thread::current().id();
        let held = messages.clone();
        let hook = previous.clone();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            if thread.id() != interface {
                let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(held) = held.as_mut() {
                    held.push(panic_message(&thread, info));
                    return;
                }
            }
            hook(info);
        }));
        Self {
            messages,
            previous,
            output,
        }
    }
}

impl<W: Write> Drop for HeldPanics<W> {
    /// Writes the held messages and passes later panics to the previous hook.
    fn drop(&mut self) {
        let messages = self
            .messages
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .unwrap_or_default();
        // The hook can't be changed while this thread unwinds; the installed
        // one passes panics on once the messages are taken.
        if !std::thread::panicking() {
            let previous = self.previous.clone();
            drop(std::panic::take_hook());
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
        for message in messages {
            let _ = writeln!(self.output, "{message}");
        }
    }
}

/// The message the default hook prints.
fn panic_message(thread: &std::thread::Thread, info: &PanicHookInfo<'_>) -> String {
    let name = thread.name().unwrap_or("<unnamed>");
    let backtrace = Backtrace::capture();
    match backtrace.status() {
        BacktraceStatus::Captured => {
            format!("thread '{name}' {info}\nstack backtrace:\n{backtrace}")
        }
        _ => format!("thread '{name}' {info}"),
    }
}

pub(super) fn notify(output: &mut impl Write, message: &str, tmux: bool) -> io::Result<()> {
    let message: String = message.chars().filter(|c| !c.is_control()).collect();
    if tmux {
        write!(output, "\x1bPtmux;\x1b\x1b]9;{message}\x07\x1b\\")?;
    } else {
        write!(output, "\x1b]9;{message}\x07")?;
    }
    output.flush()
}

#[cfg(test)]
mod tests {
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
        output.contains("thread 'agent' panicked at src/tui/terminal.rs:")
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
}
