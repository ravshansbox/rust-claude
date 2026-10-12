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
mod tests;
