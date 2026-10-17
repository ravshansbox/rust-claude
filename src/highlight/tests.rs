use super::*;

#[test]
fn marks_changes_and_keeps_text() {
    let lines = highlight_tool(
        "edit",
        "main.rs",
        "-let a = 1;\n+let a = 2;\n fn b() {}\n… 3 more lines",
        true,
    )
    .unwrap();
    let changes: Vec<Option<Change>> = lines.iter().map(|line| line.change).collect();
    assert_eq!(
        changes,
        vec![
            Some(Change::Delete),
            Some(Change::Insert),
            Some(Change::Equal),
            None
        ]
    );
    assert_eq!(
        texts(&lines),
        vec!["-let a = 1;", "+let a = 2;", " fn b() {}", "… 3 more lines"]
    );
    assert!(lines[0].segments.len() > 2);
    assert_eq!(lines[3].segments[0].foreground, None);
}

fn bash(command: &str) -> Vec<HighlightedLine> {
    highlight_tool("bash", "", command, true).unwrap()
}

fn colour_of(lines: &[HighlightedLine], word: &str) -> Option<Colour> {
    lines
        .iter()
        .flat_map(|line| &line.segments)
        .find(|segment| segment.text == word)
        .and_then(|segment| segment.foreground)
}

fn keyword_colour(path: &str, code: &str, word: &str) -> Colour {
    let lines = highlight_tool("write", path, &format!("+{code}"), true).unwrap();
    colour_of(&lines, word).unwrap()
}

fn texts(lines: &[HighlightedLine]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.segments
                .iter()
                .map(|segment| segment.text.as_str())
                .collect()
        })
        .collect()
}

#[test]
fn highlights_python_heredocs_in_bash() {
    let command = "python3 - <<'EOF'\nimport os\ndef f(x):\n  return 1\nEOF\necho done";
    let lines = bash(command);
    assert_eq!(texts(&lines), command.lines().collect::<Vec<_>>());
    let python =
        highlight_tool("write", "f.py", " import os\n def f(x):\n   return 1", true).unwrap();
    for (line, python) in lines[1..4].iter().zip(&python) {
        assert_eq!(line.segments[..], python.segments[1..]);
    }
    assert_eq!(lines[4], bash("python3 - <<'EOF'\nEOF")[1]);
}

#[test]
fn highlights_python_after_dash_c_in_bash() {
    let keyword = keyword_colour("x.py", "import sys", "import");
    let single = bash("python -u -c 'import sys; print(1)' | wc -c");
    assert_eq!(
        texts(&single),
        ["python -u -c 'import sys; print(1)' | wc -c"]
    );
    assert_eq!(colour_of(&single, "import"), Some(keyword));
    assert_eq!(colour_of(&single, "wc"), colour_of(&bash("wc -c"), "wc"));
    let multiple = bash("python3 -c \"\nimport sys\nprint(\\\"a\\\")\n\"\necho done");
    assert_eq!(colour_of(&multiple, "import"), Some(keyword));
    assert_eq!(
        colour_of(&multiple, "echo"),
        colour_of(&bash("echo"), "echo")
    );
}

#[test]
fn highlights_node_and_ruby_in_bash() {
    let javascript = bash("node -e 'const a = 1; console.log(a)'");
    assert_eq!(
        colour_of(&javascript, "const"),
        Some(keyword_colour("x.js", "const a = 1;", "const"))
    );
    let ruby = bash("ruby <<RUBY\ndef f\nend\nRUBY");
    assert_eq!(
        colour_of(&ruby, "def"),
        Some(keyword_colour("x.rb", "def f", "def"))
    );
}

#[test]
fn leaves_input_to_scripts_as_bash() {
    let lines = bash("python3 script.py <<EOF\nimport os\nEOF");
    assert_eq!(colour_of(&lines, "import"), None);
}

#[test]
fn recognises_versioned_python_by_path() {
    let keyword = keyword_colour("x.py", "import sys", "import");
    let versioned = bash("/usr/bin/python3.12 -c 'import sys'");
    assert_eq!(colour_of(&versioned, "import"), Some(keyword));
    for program in ["python3.", "python3.x", "python2"] {
        let lines = bash(&format!("{program} -c 'import sys'"));
        assert_eq!(colour_of(&lines, "import"), None, "{program}");
    }
}

#[test]
fn ends_heredocs_only_at_an_exact_delimiter_line() {
    let keyword = keyword_colour("x.py", "import os", "import");
    let plain = bash("python3 - <<EOF\nx = 1\n  EOF\nEOF \nimport os\nEOF");
    assert_eq!(colour_of(&plain, "import"), Some(keyword));
    let tabbed = bash("python3 - <<-EOF\nx = 1\nEOF x\nimport os\n\t\tEOF\necho done");
    assert_eq!(colour_of(&tabbed, "import"), Some(keyword));
    assert_eq!(colour_of(&tabbed, "echo"), colour_of(&bash("echo"), "echo"));
}

#[test]
fn ends_a_tab_stripping_heredoc_at_a_tab_indented_line_in_the_preview() {
    let command = "python3 - <<-EOF\n\timport os\n\tEOF\necho done";
    let preview = crate::tools::diff("bash", &serde_json::json!({ "command": command })).unwrap();
    let lines = bash(&preview);
    let keyword = keyword_colour("x.py", "import os", "import");
    assert_eq!(colour_of(&lines, "import"), Some(keyword));
    assert_eq!(colour_of(&lines, "echo"), colour_of(&bash("echo"), "echo"));
}
