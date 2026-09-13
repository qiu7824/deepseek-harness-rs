use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
#[derive(Clone, Debug, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub mono: bool,
    pub strike: bool,
    pub link: Option<String>,
}
#[derive(Clone, Debug)]
pub enum Block {
    Text {
        spans: Vec<Span>,
        level: u8,
        indent: usize,
        quote: bool,
        prefix: String,
    },
    Code {
        language: String,
        text: String,
    },
    Rule,
    Table {
        rows: Vec<Vec<Vec<Span>>>,
    },
    Image {
        url: String,
        alt: String,
    },
}
pub fn parse(source: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut spans: Vec<Span> = Vec::new();
    let (mut bold, mut strike) = (false, false);
    let mut link = None;
    let (mut level, mut quote) = (0, 0usize);
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut prefix = String::new();
    let mut code: Option<(String, String)> = None;
    let mut image: Option<(String, String)> = None;
    let mut table: Option<Vec<Vec<Vec<Span>>>> = None;
    let mut cells: Vec<Vec<Span>> = Vec::new();
    let flush = |out: &mut Vec<Block>,
                 spans: &mut Vec<Span>,
                 level,
                 quote,
                 lists: &Vec<Option<u64>>,
                 prefix: &mut String| {
        if !spans.is_empty() {
            out.push(Block::Text {
                spans: std::mem::take(spans),
                level,
                indent: lists.len(),
                quote: quote > 0,
                prefix: std::mem::take(prefix),
            });
        }
    };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for event in Parser::new_ext(source, opts) {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                code = Some((
                    match kind {
                        CodeBlockKind::Fenced(l) => l.to_string(),
                        _ => String::new(),
                    },
                    String::new(),
                ));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((language, text)) = code.take() {
                    out.push(Block::Code { language, text });
                }
            }
            Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => {
                if let Some((_, text)) = &mut code {
                    text.push_str(&s)
                } else if let Some((_, alt)) = &mut image {
                    alt.push_str(&s)
                } else {
                    spans.push(Span {
                        text: s.into_string(),
                        bold,
                        strike,
                        link: link.clone(),
                        ..Span::default()
                    });
                }
            }
            Event::Code(s) => spans.push(Span {
                text: s.into_string(),
                mono: true,
                bold,
                strike,
                link: link.clone(),
            }),
            Event::Start(Tag::Heading { level: l, .. }) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                level = l as u8;
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                level = 0;
            }
            Event::End(TagEnd::Paragraph) => {
                if table.is_none() {
                    flush(&mut out, &mut spans, level, quote, &lists, &mut prefix)
                }
            }
            Event::Start(Tag::Strong) => bold = true,
            Event::End(TagEnd::Strong) => bold = false,
            Event::Start(Tag::Strikethrough) => strike = true,
            Event::End(TagEnd::Strikethrough) => strike = false,
            Event::Start(Tag::Link { dest_url, .. }) => link = Some(dest_url.to_string()),
            Event::End(TagEnd::Link) => link = None,
            Event::Start(Tag::Image { dest_url, .. }) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                image = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((url, alt)) = image.take() {
                    out.push(Block::Image { url, alt })
                }
            }
            Event::Start(Tag::BlockQuote(_)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                quote += 1;
            }
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::List(start)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                prefix = match lists.last_mut() {
                    Some(Some(n)) => {
                        let s = format!("{n}.");
                        *n += 1;
                        s
                    }
                    _ => "•".into(),
                };
            }
            Event::End(TagEnd::Item) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix)
            }
            Event::TaskListMarker(checked) => prefix = if checked { "☑" } else { "☐" }.into(),
            Event::Rule => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                out.push(Block::Rule);
            }
            Event::SoftBreak => spans.push(Span {
                text: " ".into(),
                ..Span::default()
            }),
            Event::HardBreak => spans.push(Span {
                text: "\n".into(),
                ..Span::default()
            }),
            Event::Start(Tag::Table(_)) => {
                flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
                table = Some(Vec::new());
            }
            Event::End(TagEnd::TableCell) => cells.push(std::mem::take(&mut spans)),
            Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                if let Some(rows) = &mut table {
                    rows.push(std::mem::take(&mut cells));
                }
            }
            Event::End(TagEnd::Table) => {
                if let Some(rows) = table.take() {
                    out.push(Block::Table { rows })
                }
            }
            _ => {}
        }
    }
    flush(&mut out, &mut spans, level, quote, &lists, &mut prefix);
    out
}

use crate::{assets::Assets, theme::Theme};
use zsui::surface::{RasterSurface, SurfacePainter};
use zsui::{Color, TextStyle, ZsImageFrame};
#[derive(Clone)]
pub enum Draw {
    Text {
        value: String,
        style: TextStyle,
        b: [f32; 4],
        link: Option<String>,
    },
    Rect {
        b: [f32; 4],
        r: f32,
        c: Color,
    },
    Image {
        frame: ZsImageFrame,
        x: f32,
        y: f32,
    },
}
#[derive(Clone, Default)]
pub struct Layout {
    pub draws: Vec<Draw>,
    pub height: f32,
    pub copies: Vec<([f32; 4], String)>,
}
impl Layout {
    pub fn estimated_bytes(&self) -> usize {
        self.draws
            .iter()
            .map(|d| {
                std::mem::size_of::<Draw>()
                    + match d {
                        Draw::Text {
                            value, style, link, ..
                        } => {
                            value.len()
                                + style.font_family.len()
                                + link.as_ref().map_or(0, String::len)
                        }
                        _ => 0,
                    }
            })
            .sum::<usize>()
            + self.copies.iter().map(|(_, s)| s.len() + 32).sum::<usize>()
    }
    pub fn paint(
        &self,
        p: &mut SurfacePainter,
        r: &mut RasterSurface,
        x: f32,
        y: f32,
        clip: [f32; 4],
    ) {
        for d in &self.draws {
            match d {
                Draw::Text {
                    value, style, b, ..
                } => {
                    if y + b[1] + b[3] >= clip[1] && y + b[1] < clip[1] + clip[3] {
                        p.text(r, value, style, x + b[0], y + b[1], b[2], clip);
                    }
                }
                Draw::Rect { b, r: radius, c } => {
                    let top = (y + b[1]).max(clip[1]);
                    let end = (y + b[1] + b[3]).min(clip[1] + clip[3]);
                    if end > top {
                        r.rect([x + b[0], top, b[2], end - top], *radius, *c);
                    }
                }
                Draw::Image {
                    frame,
                    x: dx,
                    y: dy,
                } => r.image_clipped(frame, x + dx, y + dy, clip),
            }
        }
    }
}
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            tokens.push(c.to_string());
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}
fn inline(
    p: &mut SurfacePainter,
    spans: &[Span],
    x: f32,
    y: f32,
    width: f32,
    size: f32,
    t: Theme,
    scale: f32,
) -> (Vec<Draw>, f32) {
    let mut draws = Vec::new();
    let (mut dx, mut dy) = (0., 0.);
    let line = size * 1.65;
    for span in spans {
        let mut style = t.text(
            if span.mono { size - 1. } else { size },
            if span.link.is_some() { t.accent } else { t.ink },
            span.bold,
            span.mono,
        );
        style.line_height = line;
        for token in tokenize(&span.text) {
            if token == "\n" {
                dx = 0.;
                dy += line;
                continue;
            }
            let tw = p.width(&token, &style, scale).max(0.);
            let pieces = if tw > width {
                token.chars().map(|c| c.to_string()).collect()
            } else {
                vec![token]
            };
            for piece in pieces {
                let w = p.width(&piece, &style, scale);
                if dx + w > width && dx > 0. {
                    dx = 0.;
                    dy += line;
                }
                if dx == 0. && piece.trim().is_empty() {
                    continue;
                }
                if span.mono {
                    draws.push(Draw::Rect {
                        b: [x + dx - 1., y + dy, w + 2., line],
                        r: 3.,
                        c: t.card,
                    });
                }
                draws.push(Draw::Text {
                    value: piece,
                    style: style.clone(),
                    b: [x + dx, y + dy, w + 1., line],
                    link: span.link.clone(),
                });
                if span.strike {
                    draws.push(Draw::Rect {
                        b: [x + dx, y + dy + line / 2., w, 1.],
                        r: 0.,
                        c: t.secondary,
                    });
                }
                dx += w;
            }
        }
    }
    (draws, dy + line)
}
pub fn layout(
    blocks: &[Block],
    width: f32,
    p: &mut SurfacePainter,
    assets: &mut Assets,
    t: Theme,
    scale: f32,
    cwd: &str,
) -> Layout {
    let mut l = Layout::default();
    let mut y = 0.;
    for block in blocks {
        match block {
            Block::Text {
                spans,
                level,
                indent,
                quote,
                prefix,
            } => {
                let size = match level {
                    1 => 28.,
                    2 => 22.,
                    3 => 19.,
                    4..=6 => 17.,
                    _ => 15.,
                };
                let inset = if *quote { 16. } else { (*indent as f32) * 20. };
                let mut runs = spans.clone();
                if *level > 0 {
                    for s in &mut runs {
                        s.bold = true;
                    }
                    y += 6.;
                }
                let (draws, h) =
                    inline(p, &runs, inset, y, (width - inset).max(40.), size, t, scale);
                l.draws.extend(draws);
                if !prefix.is_empty() {
                    l.draws.push(Draw::Text {
                        value: prefix.clone(),
                        style: t.text(size, t.ink, false, false),
                        b: [inset - 19., y, 19., h],
                        link: None,
                    });
                }
                if *quote {
                    l.draws.push(Draw::Rect {
                        b: [0., y, 3., h],
                        r: 0.,
                        c: t.border,
                    });
                }
                y += h + if *indent > 0 { 6. } else { 14. };
            }
            Block::Rule => {
                l.draws.push(Draw::Rect {
                    b: [0., y + 8., width, 1.],
                    r: 0.,
                    c: t.border,
                });
                y += 25.;
            }
            Block::Code { language, text } => {
                let start = y;
                let mut code_draw = Vec::new();
                let mut line_y = y + 42.;
                for line in text.lines() {
                    let mut spans = Vec::new();
                    let mut string = false;
                    let mut comment = false;
                    for token in tokenize(line) {
                        if token == "\"" || token == "'" {
                            string = !string;
                        }
                        if token == "#" {
                            comment = true;
                        }
                        let keyword = [
                            "fn", "let", "mut", "pub", "use", "return", "if", "else", "for",
                            "while", "async", "await", "def", "class", "import", "const",
                            "function", "true", "false", "null", "None",
                        ]
                        .contains(&token.as_str());
                        let color = if comment {
                            t.muted
                        } else if string {
                            crate::theme::rgb(if t.dark { 0xa5d6a7 } else { 0xa31515 })
                        } else if keyword {
                            crate::theme::rgb(if t.dark { 0xc792ea } else { 0x0000ff })
                        } else if token.chars().all(|c| c.is_ascii_digit()) {
                            t.accent
                        } else {
                            t.ink
                        };
                        spans.push((token, color));
                    }
                    let mut x = 12.;
                    for (token, color) in spans {
                        let s = t.text(13., color, false, true);
                        let w = p.width(&token, &s, scale);
                        if x + w > width - 12. && x > 12. {
                            x = 12.;
                            line_y += 22.;
                        }
                        code_draw.push(Draw::Text {
                            value: token,
                            style: s,
                            b: [x, line_y, w + 1., 22.],
                            link: None,
                        });
                        x += w;
                    }
                    line_y += 22.;
                }
                let h = (line_y - start + 10.).max(70.);
                l.draws.push(Draw::Rect {
                    b: [0., start, width, h],
                    r: 12.,
                    c: t.card,
                });
                l.draws.push(Draw::Text {
                    value: if language.is_empty() {
                        "代码".into()
                    } else {
                        language.clone()
                    },
                    style: t.text(12., t.secondary, false, false),
                    b: [12., start + 9., (width - 90.).max(1.), 22.],
                    link: None,
                });
                l.draws.push(Draw::Text {
                    value: "复制".into(),
                    style: t.text(12., t.secondary, false, false),
                    b: [width - 46., start + 9., 38., 22.],
                    link: None,
                });
                l.copies
                    .push(([width - 52., start + 4., 48., 30.], text.clone()));
                l.draws.extend(code_draw);
                y += h + 16.;
            }
            Block::Table { rows } => {
                let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1).max(1);
                let cw = width / cols as f32;
                for (ri, row) in rows.iter().enumerate() {
                    let mut drawings = Vec::new();
                    let mut h = 28f32;
                    for (ci, spans) in row.iter().enumerate() {
                        let mut spans = spans.clone();
                        if ri == 0 {
                            for s in &mut spans {
                                s.bold = true;
                            }
                        }
                        let (ds, ch) = inline(
                            p,
                            &spans,
                            ci as f32 * cw + 10.,
                            y + 8.,
                            (cw - 20.).max(10.),
                            14.,
                            t,
                            scale,
                        );
                        h = h.max(ch + 16.);
                        drawings.extend(ds);
                    }
                    if ri == 0 {
                        l.draws.push(Draw::Rect {
                            b: [0., y, width, h],
                            r: 0.,
                            c: t.card,
                        });
                    }
                    l.draws.extend(drawings);
                    l.draws.push(Draw::Rect {
                        b: [0., y + h, width, 1.],
                        r: 0.,
                        c: t.border,
                    });
                    y += h;
                }
                y += 16.;
            }
            Block::Image { url, alt } => {
                let loaded =
                    local_path(url, cwd).and_then(|path| assets.local_image(&path, width, scale));
                match loaded {
                    Ok(frame) => {
                        let h = frame.height() as f32 / scale;
                        l.draws.push(Draw::Image { frame, x: 0., y });
                        y += h + 16.;
                    }
                    Err(_) => {
                        l.draws.push(Draw::Rect {
                            b: [0., y, width, 52.],
                            r: 8.,
                            c: t.card,
                        });
                        l.draws.push(Draw::Text {
                            value: format!("图片：{}", if alt.is_empty() { url } else { alt }),
                            style: t.text(14., t.secondary, false, false),
                            b: [12., y + 12., width - 24., 30.],
                            link: Some(url.clone()),
                        });
                        y += 68.;
                    }
                }
            }
        }
    }
    l.height = y.max(24.);
    l
}
fn local_path(url: &str, cwd: &str) -> Result<std::path::PathBuf, String> {
    let root = std::path::Path::new(cwd)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let path = root.join(url).canonicalize().map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err("图片不在会话工作区".into());
    }
    Ok(path)
}
