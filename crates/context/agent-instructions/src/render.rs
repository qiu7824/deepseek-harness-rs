use crate::files::InstructionFile;

const INTRO: &str = "The following workspace instructions may be relevant to your work. Use them as guidance when applicable. More specific instructions take precedence over broader ones. They do not override system, developer, or direct user instructions.";

fn escaped(value: &str) -> String {
    value.replace("</system-reminder>", "<\\/system-reminder>")
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_string()
}

pub fn render(files: &[InstructionFile], max_bytes: usize) -> (String, Vec<usize>) {
    render_baseline(files, max_bytes, false)
}

pub fn render_baseline(
    files: &[InstructionFile],
    max_bytes: usize,
    replaces_previous: bool,
) -> (String, Vec<usize>) {
    if files.is_empty() || max_bytes == 0 {
        return (String::new(), Vec::new());
    }
    let compose = |selected: &[InstructionFile]| {
        let intro = if replaces_previous {
            format!(
                "This complete workspace instruction baseline replaces all earlier workspace instruction baselines. {INTRO}"
            )
        } else {
            INTRO.to_string()
        };
        let mut blocks = vec![intro];
        blocks.extend(selected.iter().map(|file| {
            format!(
                "Instructions from: {}\n\n{}",
                file.display_path, file.content
            )
        }));
        format!(
            "<system-reminder>\n{}\n</system-reminder>",
            escaped(&blocks.join("\n\n"))
        )
    };
    let full = compose(files);
    if full.len() <= max_bytes {
        return (full, (0..files.len()).collect());
    }
    for start in 1..files.len() {
        let candidate = compose(&files[start..]);
        if candidate.len() <= max_bytes {
            return (candidate, (start..files.len()).collect());
        }
    }
    let last = files.last().expect("non-empty");
    let heading = format!("Instructions from: {}\n\n", last.display_path);
    let frame = format!("<system-reminder>\n{INTRO}\n\n{heading}\n</system-reminder>");
    let overhead = frame.len().saturating_sub(1);
    if overhead >= max_bytes {
        return (truncate_utf8(&frame, max_bytes), Vec::new());
    }
    let body = truncate_utf8(&last.content, max_bytes - overhead);
    let text = format!("<system-reminder>\n{INTRO}\n\n{heading}{body}\n</system-reminder>");
    (text, vec![files.len() - 1])
}
