use crate::{agent::Agent, images::Image, tools, tui};
use std::io::Write;

mod printer;

pub use printer::Printer;

/// Answers `prompt`, then each background command as it finishes. While a
/// turn runs, `checkpoint` holds where it started, so a stop can cancel it.
pub async fn run<Out: Write, Err: Write>(
    agent: &mut Agent,
    printer: &mut Printer<Out, Err>,
    prompt: &str,
    images: &[Image],
    checkpoint: &mut Option<usize>,
) -> anyhow::Result<()> {
    turn(agent, printer, prompt, images, checkpoint).await?;
    while let Some(message) = agent.background.next().await {
        printer.flush_reads();
        if let Some((summary, output)) = tools::background::display(&message) {
            printer.tool(&tui::tool_message(
                "background",
                &summary,
                output.as_deref(),
            ));
        }
        turn(agent, printer, &message, &[], checkpoint).await?;
    }
    Ok(())
}

async fn turn<Out: Write, Err: Write>(
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

#[cfg(test)]
mod tests;
