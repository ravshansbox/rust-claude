use serde::Deserialize;
use serde_json::{Value, json};

pub const NAME: &str = "ask_user_question";
pub const DECLINED: &str = "The user declined to answer";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub options: Vec<Choice>,
    #[serde(default)]
    pub multi_select: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Choice {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Deserialize)]
struct Input {
    questions: Vec<Question>,
}

pub fn definition() -> Value {
    json!({
        "name": NAME,
        "description": "Ask the user one or more multiple-choice questions and wait for the answers. Use it when the task is ambiguous or needs a decision from the user. The user can always pick Other and type their own answer, so do not add an Other option",
        "input_schema": {
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string", "description": "The full question, ending with a question mark" },
                            "header": { "type": "string", "description": "A short label for the question, at most 12 characters" },
                            "multi_select": { "type": "boolean", "description": "Let the user choose more than one option. Defaults to false" },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string", "description": "A short name for the option" },
                                        "description": { "type": "string", "description": "What the option means or what it leads to" },
                                        "recommended": { "type": "boolean", "description": "Mark the option you recommend. Defaults to false" }
                                    },
                                    "required": ["label", "description"]
                                }
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                }
            },
            "required": ["questions"]
        }
    })
}

pub fn parse(input: &Value) -> Result<Vec<Question>, String> {
    let input = Input::deserialize(input).map_err(|error| error.to_string())?;
    if !(1..=4).contains(&input.questions.len()) {
        return Err("ask between 1 and 4 questions".into());
    }
    if let Some(question) = input
        .questions
        .iter()
        .find(|question| !(2..=4).contains(&question.options.len()))
    {
        return Err(format!(
            "give between 2 and 4 options for: {}",
            question.question
        ));
    }
    Ok(input.questions)
}

pub fn format_answers(questions: &[Question], answers: &[Vec<String>]) -> String {
    questions
        .iter()
        .zip(answers)
        .map(|(question, answer)| format!("{} → {}", question.question, answer.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{Choice, DECLINED, Question, definition, format_answers, parse};

    fn input(questions: usize, options: usize) -> serde_json::Value {
        let options: Vec<_> = (0..options)
            .map(|index| json!({ "label": format!("Option {index}"), "description": "Why" }))
            .collect();
        json!({
            "questions": (0..questions)
                .map(|index| json!({
                    "question": format!("Question {index}?"),
                    "header": "Scope",
                    "options": options,
                    "multi_select": false,
                }))
                .collect::<Vec<_>>()
        })
    }

    #[test]
    fn describes_the_tool() {
        let definition = definition();
        assert_eq!(definition["name"], "ask_user_question");
        let question = &definition["input_schema"]["properties"]["questions"];
        assert_eq!(question["minItems"], 1);
        assert_eq!(question["maxItems"], 4);
        assert_eq!(question["items"]["properties"]["options"]["minItems"], 2);
        assert_eq!(question["items"]["properties"]["options"]["maxItems"], 4);
    }

    #[test]
    fn parses_questions() {
        let questions = parse(&json!({
            "questions": [{
                "question": "Which output?",
                "header": "Output",
                "multi_select": true,
                "options": [
                    { "label": "JSON", "description": "Structured", "recommended": true },
                    { "label": "Text" }
                ]
            }]
        }))
        .unwrap();
        assert_eq!(
            questions,
            vec![Question {
                question: "Which output?".into(),
                header: "Output".into(),
                multi_select: true,
                options: vec![
                    Choice {
                        label: "JSON".into(),
                        description: "Structured".into(),
                        recommended: true,
                    },
                    Choice {
                        label: "Text".into(),
                        description: String::new(),
                        recommended: false,
                    },
                ],
            }]
        );
    }

    #[test]
    fn rejects_wrong_counts() {
        assert!(parse(&input(1, 2)).is_ok());
        assert!(parse(&input(4, 4)).is_ok());
        assert_eq!(
            parse(&input(0, 2)),
            Err("ask between 1 and 4 questions".into())
        );
        assert_eq!(
            parse(&input(5, 2)),
            Err("ask between 1 and 4 questions".into())
        );
        assert_eq!(
            parse(&input(1, 1)),
            Err("give between 2 and 4 options for: Question 0?".into())
        );
        assert_eq!(
            parse(&input(1, 5)),
            Err("give between 2 and 4 options for: Question 0?".into())
        );
        assert!(parse(&json!({ "questions": "no" })).is_err());
    }

    #[test]
    fn formats_answers_as_plain_text() {
        let questions = parse(&input(2, 2)).unwrap();
        let answers = vec![
            vec!["Option 1".to_string()],
            vec!["Option 0".to_string(), "Other: something else".to_string()],
        ];
        assert_eq!(
            format_answers(&questions, &answers),
            "Question 0? → Option 1\nQuestion 1? → Option 0, Other: something else"
        );
        assert_eq!(DECLINED, "The user declined to answer");
    }
}
