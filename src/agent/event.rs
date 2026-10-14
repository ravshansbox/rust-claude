use super::Stats;

pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ToolStart {
        name: String,
        summary: String,
        diff: Option<String>,
    },
    ToolDone {
        name: String,
        error: Option<String>,
        note: Option<String>,
    },
    Stats(Stats),
    Notice(String),
    Queued(String),
    /// A subagent started on task `task`, counting from 0.
    TaskStart {
        task: usize,
        description: String,
    },
    /// The subagent on `task` has made `count` tool calls so far.
    TaskToolCalls {
        task: usize,
        count: usize,
    },
    /// The latest stats of the subagent on `task`.
    TaskStats {
        task: usize,
        stats: Stats,
    },
    /// The subagent on `task` finished with `reply` after `tool_calls` tool
    /// calls, or failed or did not run, as the error tells the main agent.
    TaskDone {
        task: usize,
        reply: Result<String, String>,
        tool_calls: usize,
    },
}
