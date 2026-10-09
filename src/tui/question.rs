use crate::ask::Question;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    style::Stylize,
    text::Line,
    widgets::{Paragraph, Wrap},
};
use tokio::sync::oneshot;

pub struct QuestionPrompt {
    questions: Vec<Question>,
    answers: Vec<Vec<String>>,
    selected: usize,
    checked: Vec<bool>,
    other: String,
    reply: Option<oneshot::Sender<Vec<Vec<String>>>>,
}

impl QuestionPrompt {
    pub fn new(questions: Vec<Question>, reply: oneshot::Sender<Vec<Vec<String>>>) -> Self {
        let checked = vec![false; questions[0].options.len()];
        Self {
            questions,
            answers: Vec::new(),
            selected: 0,
            checked,
            other: String::new(),
            reply: Some(reply),
        }
    }

    fn current(&self) -> &Question {
        &self.questions[self.answers.len()]
    }

    fn on_other(&self) -> bool {
        self.selected == self.current().options.len()
    }

    pub fn paste(&mut self, text: &str) {
        if self.on_other() {
            self.other.push_str(&text.replace(['\r', '\n'], " "));
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> bool {
        let other = self.current().options.len();
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(other),
            KeyCode::Char(' ') if self.current().multi_select && !self.on_other() => {
                self.checked[self.selected] = !self.checked[self.selected];
            }
            KeyCode::Char(character)
                if self.on_other() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.other.push(character);
            }
            KeyCode::Backspace if self.on_other() => {
                self.other.pop();
            }
            KeyCode::Enter => return self.confirm(),
            _ => {}
        }
        false
    }

    fn confirm(&mut self) -> bool {
        let question = self.current();
        let mut answer: Vec<String> = if question.multi_select {
            question
                .options
                .iter()
                .zip(&self.checked)
                .filter(|(_, checked)| **checked)
                .map(|(choice, _)| choice.label.clone())
                .collect()
        } else {
            question
                .options
                .get(self.selected)
                .map(|choice| choice.label.clone())
                .into_iter()
                .collect()
        };
        let other = self.other.trim();
        if (question.multi_select || self.on_other()) && !other.is_empty() {
            answer.push(format!("Other: {other}"));
        }
        if answer.is_empty() {
            return false;
        }
        self.answers.push(answer);
        if self.answers.len() == self.questions.len() {
            if let Some(reply) = self.reply.take() {
                let _ = reply.send(std::mem::take(&mut self.answers));
            }
            return true;
        }
        self.selected = 0;
        self.checked = vec![false; self.current().options.len()];
        self.other.clear();
        false
    }

    pub fn view(&self) -> Paragraph<'_> {
        let question = self.current();
        let count = if self.questions.len() > 1 {
            format!(" ({}/{})", self.answers.len() + 1, self.questions.len())
        } else {
            String::new()
        };
        let keys = if question.multi_select {
            "↑↓ select, Space toggle, Enter confirm, Esc decline"
        } else {
            "↑↓ select, Enter confirm, Esc decline"
        };
        let mut lines = vec![
            Line::from(format!("{}{count}: {}", question.header, question.question).bold()),
            Line::raw(keys),
        ];
        let mark = |index: usize| match (question.multi_select, self.checked.get(index)) {
            (true, Some(true)) => "[x] ",
            (true, _) => "[ ] ",
            (false, _) => "",
        };
        for (index, choice) in question.options.iter().enumerate() {
            let mut text = format!("{}{}", mark(index), choice.label);
            if choice.recommended {
                text.push_str(" (recommended)");
            }
            if !choice.description.is_empty() {
                text.push_str(&format!(": {}", choice.description));
            }
            lines.push(self.line(index, text));
        }
        let other = question.options.len();
        lines.push(self.line(other, format!("Other: {}", self.other)));
        Paragraph::new(lines).wrap(Wrap { trim: false })
    }

    fn line(&self, index: usize, text: String) -> Line<'static> {
        if index == self.selected {
            Line::from(text.reversed())
        } else {
            Line::raw(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::AgentEvent;
    use crate::ask;
    use crate::tui::{
        App, UiEvent, handle_agent_event,
        test_support::{app_with_reply, new_app, press, screen, type_text},
    };
    use crossterm::event::KeyCode;
    use serde_json::json;
    use tokio::sync::oneshot::{self, error::TryRecvError};

    fn ask(app: &mut App, input: serde_json::Value) -> oneshot::Receiver<Vec<Vec<String>>> {
        let (reply, answers) = oneshot::channel();
        app.busy = true;
        handle_agent_event(
            UiEvent::Agent(AgentEvent::Question {
                questions: ask::parse(&input).unwrap(),
                reply,
            }),
            app,
        );
        answers
    }

    fn output_question(multi_select: bool) -> serde_json::Value {
        json!({ "questions": [{
            "question": "Which output?",
            "header": "Output",
            "multi_select": multi_select,
            "options": [
                { "label": "JSON", "description": "Structured", "recommended": true },
                { "label": "Text", "description": "Readable" }
            ]
        }] })
    }

    #[test]
    fn shows_a_question_and_sends_the_chosen_option() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        let shown = screen(&mut app);
        assert!(shown.contains("Output: Which output?"), "{shown}");
        assert!(shown.contains("JSON (recommended): Structured"), "{shown}");
        assert!(shown.contains("Text: Readable"), "{shown}");
        assert!(shown.contains("Other: "), "{shown}");
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv().unwrap(), vec![vec!["Text".to_string()]]);
        assert!(!screen(&mut app).contains("Which output?"));
    }

    #[test]
    fn sends_a_typed_answer_for_other() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "YAML please");
        assert!(screen(&mut app).contains("Other: YAML please"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["Other: YAML please".to_string()]]
        );
        assert_eq!(app.input, "");
    }

    #[test]
    fn ignores_enter_on_an_empty_other() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Empty));
        assert!(screen(&mut app).contains("Which output?"));
    }

    #[test]
    fn toggles_options_in_a_multiple_choice_question() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(true));
        press(&mut app, KeyCode::Char(' '));
        assert!(screen(&mut app).contains("[x] JSON"));
        assert!(screen(&mut app).contains("[ ] Text"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Down);
        type_text(&mut app, "a b");
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["JSON".to_string(), "Other: a b".to_string()]]
        );
    }

    #[test]
    fn asks_each_question_in_turn() {
        let mut app = new_app();
        let mut input = output_question(false);
        let mut second = input["questions"][0].clone();
        second["question"] = json!("Which colour?");
        second["header"] = json!("Colour");
        input["questions"].as_array_mut().unwrap().push(second);
        let mut answers = ask(&mut app, input);
        assert!(screen(&mut app).contains("Output (1/2): Which output?"));
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Empty));
        assert!(screen(&mut app).contains("Colour (2/2): Which colour?"));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            answers.try_recv().unwrap(),
            vec![vec!["JSON".to_string()], vec!["Text".to_string()]]
        );
    }

    #[test]
    fn declines_the_question_on_escape() {
        let mut app = new_app();
        let mut answers = ask(&mut app, output_question(false));
        press(&mut app, KeyCode::Esc);
        assert_eq!(answers.try_recv(), Err(TryRecvError::Closed));
        assert!(!screen(&mut app).contains("Which output?"));
        assert!(app.busy);
    }

    #[test]
    fn keeps_the_conversation_visible_while_asking() {
        let mut app = app_with_reply();
        ask(&mut app, output_question(false));
        let shown = screen(&mut app);
        assert!(shown.contains("Earlier reply"), "{shown}");
        assert!(shown.contains("Which output?"), "{shown}");
    }

    #[test]
    fn highlights_the_recommended_option_even_when_listed_later() {
        let mut app = new_app();
        let mut answers = ask(
            &mut app,
            json!({ "questions": [{
                "question": "Which output?",
                "header": "Output",
                "options": [
                    { "label": "Text", "description": "Readable" },
                    { "label": "JSON", "description": "Structured", "recommended": true }
                ]
            }] }),
        );
        let shown = screen(&mut app);
        let json_row = shown.find("JSON (recommended)").unwrap();
        let text_row = shown.find("Text: Readable").unwrap();
        assert!(json_row < text_row, "{shown}");
        press(&mut app, KeyCode::Enter);
        assert_eq!(answers.try_recv().unwrap(), vec![vec!["JSON".to_string()]]);
    }
}
