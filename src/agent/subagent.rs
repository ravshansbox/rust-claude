use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::{Agent, AgentEvent, Queue, Usage, text_of};
use crate::{
    session::Session,
    skills::Skills,
    tools::agent::{self, Mode},
};

const NOT_RUN: &str = "This task did not run, because an earlier task failed.";
const FAILED: &str = "This task failed: ";

pub(crate) fn is_failure(reply: &str) -> bool {
    reply.starts_with(FAILED) || reply == NOT_RUN
}

type Callback = Box<dyn FnMut(AgentEvent) + Send>;

impl Agent {
    fn subagent(&self) -> Result<Agent> {
        Ok(Agent {
            http: self.http.clone(),
            api_base: self.api_base.clone(),
            credentials: self.credentials.clone(),
            model: self.model.clone(),
            effort: self.effort,
            messages: Vec::new(),
            pending_usage: Usage::default(),
            tool_results: Vec::new(),
            quota: self.quota,
            instructions: self.instructions.clone(),
            skills: Skills {
                skills: self.skills.skills.clone(),
                diagnostics: Vec::new(),
            },
            mcp: self.mcp.clone(),
            session: Session::new()?,
            queue: Queue::default(),
            missing_programs: self.missing_programs.clone(),
            subagent_usage: Arc::default(),
            subagent: true,
            background: Default::default(),
        })
    }

    /// Runs the tasks of an `agent` tool call, each in its own subagent, and
    /// gives one text block per task with its final reply.
    pub(super) async fn run_tasks(
        &self,
        input: &Value,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<Value, String> {
        let (mode, tasks) = agent::parse(input)?;
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let work = async move {
            match mode {
                Mode::Parallel => {
                    futures::future::join_all(tasks.into_iter().enumerate().map(|(index, task)| {
                        self.run_task(index, task.description, task.prompt, sender.clone())
                    }))
                    .await
                }
                Mode::Sequential => {
                    let mut replies: Vec<Result<String, String>> = Vec::new();
                    for (index, task) in tasks.into_iter().enumerate() {
                        let prompt = match replies.last() {
                            Some(Err(_)) => {
                                let _ = sender.send(AgentEvent::TaskStart {
                                    task: index,
                                    description: task.description,
                                });
                                let _ = sender.send(AgentEvent::TaskDone {
                                    task: index,
                                    reply: Err(NOT_RUN.into()),
                                    tool_calls: 0,
                                });
                                replies.push(Err(NOT_RUN.into()));
                                continue;
                            }
                            Some(Ok(previous)) => format!(
                                "{}\n\n<previous-task-result>\n{previous}\n</previous-task-result>",
                                task.prompt
                            ),
                            None => task.prompt,
                        };
                        let reply = self
                            .run_task(index, task.description, prompt, sender.clone())
                            .await;
                        replies.push(reply);
                    }
                    replies
                }
            }
        };
        let forward = async {
            while let Some(event) = receiver.recv().await {
                match event {
                    AgentEvent::Stats(_) => on_event(AgentEvent::Stats(self.stats())),
                    event => on_event(event),
                }
            }
        };
        let (replies, ()) = futures::join!(work, forward);
        Ok(Value::Array(
            replies
                .into_iter()
                .map(|reply| json!({ "type": "text", "text": reply.unwrap_or_else(|error| error) }))
                .collect(),
        ))
    }

    /// Boxed, because a subagent's run holds this future's type in turn.
    fn run_task(
        &self,
        task: usize,
        description: String,
        prompt: String,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>> {
        Box::pin(async move {
            let _ = events.send(AgentEvent::TaskStart { task, description });
            let tool_calls = Arc::new(AtomicUsize::new(0));
            let reply = self
                .run_subagent(task, prompt, events.clone(), tool_calls.clone())
                .await;
            let _ = events.send(AgentEvent::TaskDone {
                task,
                reply: reply.clone(),
                tool_calls: tool_calls.load(Ordering::Relaxed),
            });
            reply
        })
    }

    async fn run_subagent(
        &self,
        task: usize,
        prompt: String,
        events: mpsc::UnboundedSender<AgentEvent>,
        tool_calls: Arc<AtomicUsize>,
    ) -> Result<String, String> {
        let failed = |error: anyhow::Error| format!("{FAILED}{error:#}");
        let mut child = self.subagent().map_err(failed)?;
        let usage = self.subagent_usage.clone();
        let slot = usage.lock().map_or(0, |mut slots| {
            slots.push(Usage::default());
            slots.len() - 1
        });
        let count = move |counted: Usage| {
            if let Ok(mut slots) = usage.lock()
                && let Some(slot) = slots.get_mut(slot)
            {
                *slot = counted;
            }
        };
        let count_failed = count.clone();
        let callback: Callback = Box::new(move |event| match &event {
            AgentEvent::Stats(stats) => {
                count(stats.usage);
                let _ = events.send(AgentEvent::TaskStats {
                    task,
                    stats: *stats,
                });
                let _ = events.send(event);
            }
            AgentEvent::ToolStart { .. } => {
                let count = tool_calls.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = events.send(AgentEvent::TaskToolCalls { task, count });
            }
            _ => {}
        });
        if let Err(error) = child.run(&prompt, &[], callback).await {
            child.discard_from(child.messages.len(), "error", false);
            count_failed(child.stats().usage);
            return Err(failed(error));
        }
        Ok(final_reply(&child.messages))
    }
}

fn final_reply(messages: &[Value]) -> String {
    messages
        .iter()
        .rfind(|message| message["role"] == "assistant" && message.get("stop_reason").is_none())
        .and_then(|message| message["content"].as_array())
        .map_or_else(String::new, |content| text_of(content))
}

#[cfg(test)]
mod tests {
    use super::super::{
        AgentEvent,
        test_support::{self, MockApi, Reply, text_reply, tool_reply},
    };
    use serde_json::{Value, json};
    use std::time::Duration;

    fn tool_names(request: &Value) -> Vec<String> {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect()
    }

    fn tool_result(request: &Value) -> Value {
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .find(|block| block["type"] == "tool_result")
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn runs_a_task_in_a_subagent_and_returns_its_final_reply() {
        let prompt = format!("what is 6 x 7? {}", std::process::id());
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "sequential",
                    "tasks": [{ "description": "work out the answer", "prompt": prompt }],
                }),
            ),
            text_reply("42"),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let result = agent.prompt("ask a subagent", &[], |_| {}).await;
        let sessions = crate::config::dir().unwrap().join("sessions");
        let saved_with_prompt = std::fs::read_dir(&sessions)
            .unwrap()
            .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
            .filter(|text| text.contains(&prompt))
            .count();
        test_support::remove_session(&agent);
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 3);
        let child = &requests[1];
        assert!(tool_names(&requests[0]).contains(&"agent".to_string()));
        assert!(!tool_names(child).contains(&"agent".to_string()));
        assert!(tool_names(child).contains(&"bash".to_string()));
        assert!(tool_names(child).contains(&"web_search".to_string()));
        assert!(tool_names(&requests[0]).contains(&"bash_stop".to_string()));
        assert!(!tool_names(child).contains(&"bash_stop".to_string()));
        let background = |request: &Value| {
            request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tool| tool["name"] == "bash")
                .unwrap()["input_schema"]["properties"]
                .get("run_in_background")
                .is_some()
        };
        assert!(background(&requests[0]));
        assert!(!background(child));
        assert_eq!(child["system"], requests[0]["system"]);
        assert_eq!(child["model"], requests[0]["model"]);
        assert_eq!(child["thinking"], requests[0]["thinking"]);
        assert_eq!(child["messages"].as_array().unwrap().len(), 1);
        assert!(child["messages"].to_string().contains(&prompt));
        assert!(!child["messages"].to_string().contains("ask a subagent"));
        let result = tool_result(&requests[2]);
        assert_eq!(result["is_error"], false);
        assert_eq!(result["content"], json!([{ "type": "text", "text": "42" }]));
        assert_eq!(saved_with_prompt, 1);
    }

    fn task(description: &str, prompt: &str) -> Value {
        json!({ "description": description, "prompt": prompt })
    }

    #[tokio::test]
    async fn gives_each_sequential_task_the_previous_final_reply() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "sequential",
                    "tasks": [task("list files", "list the files"), task("pick one", "pick a file")],
                }),
            ),
            text_reply("a.rs and b.rs"),
            text_reply("b.rs"),
            text_reply("done"),
        ])
        .await;
        let (_, result) = test_support::run_prompt(&api, "chain", |_| {}).await;
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[2]["messages"][0]["content"],
            json!([{
                "type": "text",
                "text": "pick a file\n\n<previous-task-result>\na.rs and b.rs\n</previous-task-result>",
                "cache_control": { "type": "ephemeral" },
            }])
        );
        assert_eq!(
            tool_result(&requests[3])["content"],
            json!([
                { "type": "text", "text": "a.rs and b.rs" },
                { "type": "text", "text": "b.rs" },
            ])
        );
    }

    #[tokio::test]
    async fn stops_a_sequence_at_the_task_that_failed() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "sequential",
                    "tasks": [task("first", "one"), task("second", "two")],
                }),
            ),
            Reply::BadRequest("prompt is too long".into()),
            text_reply("done"),
        ])
        .await;
        let (_, result) = test_support::run_prompt(&api, "chain", |_| {}).await;
        result.unwrap();
        let requests = api.requests().await;
        assert_eq!(requests.len(), 3);
        let content = tool_result(&requests[2])["content"].clone();
        let first = content[0]["text"].as_str().unwrap();
        assert!(first.starts_with("This task failed: "), "{first}");
        assert!(first.contains("prompt is too long"), "{first}");
        assert_eq!(
            content[1]["text"],
            "This task did not run, because an earlier task failed."
        );
    }

    #[tokio::test]
    async fn runs_parallel_tasks_at_the_same_time() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "parallel",
                    "tasks": [task("first", "one"), task("second", "two")],
                }),
            ),
            Reply::Stall,
            Reply::Stall,
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut prompt = Box::pin(agent.prompt("both", &[], |_| {}));
        let both_sent = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _ = &mut prompt => return false,
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if api.requests().await.len() == 3 {
                            return true;
                        }
                    }
                }
            }
        })
        .await;
        drop(prompt);
        test_support::remove_session(&agent);
        assert_eq!(both_sent, Ok(true));
    }

    #[tokio::test]
    async fn rejects_tasks_it_cannot_run() {
        let cases = [
            (
                json!({ "mode": "parallel", "tasks": [] }),
                "tasks must not be empty",
            ),
            (
                json!({ "mode": "random", "tasks": [task("a", "b")] }),
                "unknown mode: random",
            ),
            (
                json!({ "mode": "parallel", "tasks": [{ "prompt": "b" }] }),
                "missing argument: description",
            ),
            (
                json!({ "tasks": [task("a", "b")] }),
                "missing argument: mode",
            ),
        ];
        for (input, error) in cases {
            let api = MockApi::start(vec![
                tool_reply("call-1", "agent", input.clone()),
                text_reply("done"),
            ])
            .await;
            let (_, result) = test_support::run_prompt(&api, "go", |_| {}).await;
            result.unwrap();
            let requests = api.requests().await;
            assert_eq!(requests.len(), 2, "{input}");
            let result = tool_result(&requests[1]);
            assert_eq!(result["is_error"], true, "{input}");
            assert_eq!(result["content"], error, "{input}");
        }
    }

    fn describe(event: AgentEvent) -> Option<String> {
        Some(match event {
            AgentEvent::TaskStart { task, description } => format!("{task} start {description}"),
            AgentEvent::TaskToolCalls { task, count } => format!("{task} tool calls {count}"),
            AgentEvent::TaskDone {
                task,
                reply,
                tool_calls,
            } => format!(
                "{task} done {} after {tool_calls}",
                reply.unwrap_or_else(|error| error)
            ),
            AgentEvent::Text(text) => format!("text {text}"),
            AgentEvent::Thinking(text) => format!("thinking {text}"),
            AgentEvent::ToolStart { name, .. } => format!("tool {name}"),
            AgentEvent::ToolDone { name, error, .. } => format!("tool done {name} {error:?}"),
            AgentEvent::Notice(text) => format!("notice {text}"),
            AgentEvent::Stats(_) | AgentEvent::TaskStats { .. } => "stats".into(),
            AgentEvent::Queued(prompt) => format!("queued {prompt}"),
        })
    }

    #[tokio::test]
    async fn reports_when_each_subagent_starts_calls_tools_and_finishes() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "sequential",
                    "tasks": [task("say hi", "say hi"), task("say bye", "say bye")],
                }),
            ),
            tool_reply("call-2", "bash", json!({ "command": "true" })),
            text_reply("hi"),
            text_reply("bye"),
            text_reply("done"),
        ])
        .await;
        let mut events = Vec::new();
        let (_, result) =
            test_support::run_prompt(&api, "go", |event| events.extend(describe(event))).await;
        result.unwrap();
        let tasks: Vec<_> = events
            .iter()
            .skip_while(|event| *event != "tool agent")
            .take_while(|event| !event.starts_with("tool done agent"))
            .filter(|event| *event != "stats")
            .cloned()
            .collect();
        assert_eq!(
            tasks,
            [
                "tool agent",
                "0 start say hi",
                "0 tool calls 1",
                "0 done hi after 1",
                "1 start say bye",
                "1 done bye after 0",
            ]
        );
    }

    #[tokio::test]
    async fn reports_the_stats_of_each_subagent() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "sequential",
                    "tasks": [task("first", "one"), task("second", "two")],
                }),
            ),
            tool_reply("call-2", "bash", json!({ "command": "true" })),
            text_reply("one done"),
            text_reply("two done"),
            text_reply("done"),
        ])
        .await;
        let mut last = std::collections::BTreeMap::new();
        let (_, result) = test_support::run_prompt(&api, "go", |event| {
            if let AgentEvent::TaskStats { task, stats } = event {
                last.insert(task, (stats.usage.output, stats.context_tokens.is_some()));
            }
        })
        .await;
        result.unwrap();
        assert_eq!(
            last.into_iter().collect::<Vec<_>>(),
            [(0, (2, true)), (1, (1, true))]
        );
    }

    #[tokio::test]
    async fn counts_the_tokens_subagents_use_in_the_totals() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "parallel",
                    "tasks": [task("first", "one"), task("second", "two")],
                }),
            ),
            text_reply("one done"),
            text_reply("two done"),
            text_reply("done"),
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let mut inputs = Vec::new();
        let result = agent
            .prompt("go", &[], |event| {
                if let AgentEvent::Stats(stats) = event {
                    inputs.push(stats.usage.input);
                }
            })
            .await;
        let context_tokens = agent.stats().context_tokens;
        let id = agent.session.id.clone();
        drop(agent);
        let mut resumed = test_support::agent(&api, reqwest::Client::new());
        let loaded = resumed.resume(&id).await;
        test_support::remove_session(&resumed);
        result.unwrap();
        loaded.unwrap();
        assert!(inputs.contains(&3), "{inputs:?}");
        assert_eq!(inputs.last(), Some(&4));
        let usage = resumed.stats().usage;
        assert_eq!((usage.input, usage.output), (4, 4));
        assert_eq!(resumed.stats().context_tokens, context_tokens);
        let requests = api.requests().await;
        assert!(!requests[3]["messages"].to_string().contains("usage"));
    }

    #[tokio::test]
    async fn counts_the_tokens_of_a_subagent_that_failed() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({ "mode": "parallel", "tasks": [task("first", "one")] }),
            ),
            test_support::stopped_reply("partial", "refusal"),
            text_reply("done"),
        ])
        .await;
        let (agent, result) = test_support::run_prompt(&api, "go", |_| {}).await;
        result.unwrap();
        assert_eq!(agent.stats().usage.input, 3);
    }

    #[tokio::test]
    async fn counts_finished_subagent_replies_when_cancelled() {
        let api = MockApi::start(vec![
            tool_reply(
                "call-1",
                "agent",
                json!({
                    "mode": "parallel",
                    "tasks": [task("first", "one"), task("second", "two")],
                }),
            ),
            text_reply("one done"),
            Reply::Stall,
        ])
        .await;
        let mut agent = test_support::agent(&api, reqwest::Client::new());
        let one_done = std::cell::Cell::new(false);
        let mut prompt = Box::pin(agent.prompt("go", &[], |event| {
            if matches!(event, AgentEvent::TaskDone { reply: Ok(_), .. }) {
                one_done.set(true);
            }
        }));
        let ready = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                tokio::select! {
                    _ = &mut prompt => return false,
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if one_done.get() && api.requests().await.len() == 3 {
                            return true;
                        }
                    }
                }
            }
        })
        .await;
        drop(prompt);
        let cancelled = agent.cancel(0);
        test_support::remove_session(&agent);
        cancelled.unwrap();
        assert_eq!(ready, Ok(true));
        assert_eq!(agent.stats().usage.input, 2);
    }
}
