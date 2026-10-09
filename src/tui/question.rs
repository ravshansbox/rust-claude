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
