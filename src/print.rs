use crate::{
    agent::{Agent, AgentEvent},
    images::Image,
};
use std::{
    io::Write,
    sync::atomic::{AtomicBool, Ordering},
};

mod printer;

pub use printer::Printer;

/// Answers `prompt`. While it runs, `checkpoint` holds where it started, so
/// a stop can cancel it.
pub async fn run<Out: Write, Err: Write>(
    agent: &mut Agent,
    printer: &mut Printer<Out, Err>,
    prompt: &str,
    images: &[Image],
    checkpoint: &mut Option<usize>,
) -> anyhow::Result<()> {
    *checkpoint = Some(agent.messages().len());
    agent
        .prompt(prompt, images, |event| printer.event(event))
        .await?;
    *checkpoint = None;
    Ok(())
}

/// Runs `prompt` again and again, each time in a new session, until a run
/// fails or `stop_after_run` is set when a run finishes.
pub async fn run_loop<Out: Write, Err: Write>(
    agent: &mut Agent,
    printer: &mut Printer<Out, Err>,
    prompt: &str,
    images: &[Image],
    checkpoint: &mut Option<usize>,
    stop_after_run: &AtomicBool,
) -> anyhow::Result<()> {
    for round in 1.. {
        if round > 1 {
            agent.new_session()?;
        }
        printer.event(AgentEvent::Notice(format!("loop {round}")));
        run(agent, printer, prompt, images, checkpoint).await?;
        if stop_after_run.load(Ordering::Relaxed) {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
