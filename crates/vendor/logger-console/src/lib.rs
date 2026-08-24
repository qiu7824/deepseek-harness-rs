//! Console logger exporter for cordis: Rust port of
//! `@deepseek-ai/cordis-plugin-logger-console` (the shared + Node builds).
//!
//! # Deviations
//!
//! - `util.inspect` object formatting is approximated with compact JSON for
//!   `serde_json::Value` payloads and plain stringification otherwise.
//! - Color support detection uses `stdout().is_terminal()` plus the
//!   `NO_COLOR`/`FORCE_COLOR` env conventions instead of the
//!   `supports-color` package (no truecolor probing).

use std::collections::HashMap;
use std::sync::Arc;

use cordis::{
    ArcValue, Context, Exporter, LoggerLevel, LoggerType, Message, Plugin, PluginError, arc,
};
use indexmap::IndexMap;
use parking_lot::Mutex;
use serde::Deserialize;

/// ANSI 16-color palette indexes used for logger name coloring.
pub const C16: [usize; 6] = [6, 2, 3, 4, 5, 1];
/// ANSI 256-color palette indexes used for logger name coloring.
pub const C256: &[usize] = &[
    20, 21, 26, 27, 32, 33, 38, 39, 40, 41, 42, 43, 44, 45, 56, 57, 62, 63, 68, 69, 74, 75, 76, 77,
    78, 79, 80, 81, 92, 93, 98, 99, 112, 113, 129, 134, 135, 148, 149, 160, 161, 162, 163, 164,
    165, 166, 167, 168, 169, 170, 171, 172, 173, 178, 179, 184, 185, 196, 197, 198, 199, 200, 201,
    202, 203, 204, 205, 206, 207, 208, 209, 214, 215, 220, 221,
];

/// Terminal color support level compatible with supports-color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorLevel {
    Disabled,
    Level(usize),
}

impl<'de> Deserialize<'de> for ColorLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Bool(false) => Ok(ColorLevel::Disabled),
            serde_json::Value::Bool(true) => Ok(ColorLevel::Level(3)),
            serde_json::Value::Number(number) => Ok(ColorLevel::Level(
                number.as_u64().unwrap_or(0).min(3) as usize,
            )),
            _ => Ok(ColorLevel::Disabled),
        }
    }
}

/// Formatting options for the logger name label (TS `LabelStyle`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LabelStyle {
    #[serde(default)]
    pub width: Option<usize>,
    #[serde(default)]
    pub margin: Option<usize>,
    #[serde(default)]
    pub align: Option<String>,
}

/// Config namespace for console logger exporters (TS
/// `ConsoleExporter.Config`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ConsoleConfig {
    #[serde(default)]
    pub colors: Option<ColorLevel>,
    #[serde(default)]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub levels: Option<IndexMap<String, usize>>,
    #[serde(default)]
    pub show_diff: Option<bool>,
    #[serde(default)]
    pub show_time: Option<String>,
    #[serde(default)]
    pub label: Option<LabelStyle>,
}

fn color(colors: Option<usize>, code: usize, value: &str, decoration: &str) -> String {
    let Some(level) = colors else {
        return value.to_string();
    };
    if level == 0 {
        return value.to_string();
    }
    let code_text = if code < 8 {
        code.to_string()
    } else {
        format!("8;5;{code}")
    };
    let decoration = if level >= 2 { decoration } else { "" };
    format!("\u{1b}[3{code_text}{decoration}m{value}\u{1b}[0m")
}

/// Hash a logger name into a palette index (TS `Logger.code`).
pub fn name_code(name: &str, level: Option<usize>) -> usize {
    let mut hash: i32 = 0;
    for ch in name.chars() {
        hash = hash
            .wrapping_mul(7)
            .wrapping_add(ch as u32 as i32)
            .wrapping_add(13);
    }
    let palette: &[usize] = match level {
        Some(level) if level >= 2 => C256,
        _ => &C16,
    };
    palette[hash.unsigned_abs() as usize % palette.len()]
}

/// Stringify an argument value (approximation of `String(value)`).
fn stringify(value: &ArcValue) -> String {
    if let Some(text) = cordis::downcast::<String>(value) {
        return text.clone();
    }
    if let Some(text) = cordis::downcast::<&'static str>(value) {
        return (*text).to_string();
    }
    if let Some(number) = cordis::downcast::<f64>(value) {
        return format!("{number}");
    }
    if let Some(number) = cordis::downcast::<i64>(value) {
        return number.to_string();
    }
    if let Some(bool) = cordis::downcast::<bool>(value) {
        return bool.to_string();
    }
    if let Some(json) = cordis::downcast::<serde_json::Value>(value) {
        return serde_json::to_string(json).unwrap_or_default();
    }
    if let Some(error) = cordis::downcast::<anyhow::Error>(value) {
        return format!("{error:#}");
    }
    if let Some(error) = cordis::downcast::<cordis::PluginError>(value) {
        return error.message();
    }
    "[object]".to_string()
}

/// Port of `Logger.format`: printf-style placeholder substitution plus
/// max-length truncation.
pub fn format_message(message: &Message, exporter: &ConsoleExporter) -> String {
    let mut args: Vec<ArcValue> = message.args.clone();
    let first_is_error = args
        .first()
        .is_some_and(|arg| cordis::downcast::<anyhow::Error>(arg).is_some());
    if first_is_error {
        let error = args.remove(0);
        let text = stringify(&error);
        args.insert(0, arc(text));
        args.insert(0, arc("%s".to_string()));
    } else if !args
        .first()
        .is_some_and(|arg| cordis::downcast::<String>(arg).is_some())
    {
        args.insert(0, arc("%o".to_string()));
    }

    let format: String = stringify(&args.remove(0));
    let mut output = String::with_capacity(format.len());
    let mut chars = format.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            output.push(ch);
            continue;
        }
        let Some(next) = chars.next() else {
            output.push('%');
            break;
        };
        if next == '%' {
            output.push('%');
            continue;
        }
        let value = args.remove(0);
        let formatted = match next {
            's' => stringify(&value),
            'd' | 'i' => stringify(&arc(
                stringify(&value).parse::<f64>().unwrap_or(0.0).trunc() as i64,
            )),
            'f' => stringify(&arc(stringify(&value).parse::<f64>().unwrap_or(0.0))),
            'o' | 'O' => exporter.inspect(&value),
            'c' => String::new(),
            'C' => {
                let code = name_code(&message.name, exporter.colors_level());
                color(exporter.colors_level(), code, &stringify(&value), "")
            }
            other => format!("%{other}"),
        };
        output += &formatted;
    }
    for arg in args {
        output.push(' ');
        output += &exporter.inspect(&arg);
    }

    let max_length = exporter.max_length.unwrap_or(10240);
    output
        .split('\n')
        .map(|line| {
            if line.len() > max_length {
                format!("{}...", &line[..max_length])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Console log exporter (TS `ConsoleExporter`).
#[derive(Debug)]
pub struct ConsoleExporter {
    pub colors: Option<usize>,
    pub max_length: Option<usize>,
    pub levels: HashMap<String, LoggerLevel>,
    pub show_diff: bool,
    pub show_time: String,
    pub label: Option<LabelStyle>,
    pub timestamp: Mutex<i64>,
}

impl ConsoleExporter {
    pub fn new(config: &ConsoleConfig) -> Arc<Self> {
        let colors = config
            .colors
            .map(|level| match level {
                ColorLevel::Disabled => None,
                ColorLevel::Level(level) => Some(level),
            })
            .unwrap_or_else(detect_color_level);
        let show_time = config
            .show_time
            .clone()
            .unwrap_or_else(|| "yyyy-MM-dd hh:mm:ss ".to_string());
        let levels = config
            .levels
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|(name, level)| (name, LoggerLevel::from_usize(level)))
            .collect();
        Arc::new(Self {
            colors,
            max_length: config.max_length,
            levels,
            show_diff: config.show_diff.unwrap_or(false),
            show_time,
            label: config.label.clone(),
            timestamp: Mutex::new(0),
        })
    }

    fn colors_level(&self) -> Option<usize> {
        self.colors
    }

    /// Object inspection formatter (TS `util.inspect` approximation).
    pub fn inspect(&self, value: &ArcValue) -> String {
        if let Some(json) = cordis::downcast::<serde_json::Value>(value) {
            return serde_json::to_string(json).unwrap_or_default();
        }
        stringify(value)
    }

    /// Render one message (TS `render`).
    pub fn render(&self, message: &Message) -> String {
        let prefix = format!("[{}]", logger_type_letter(message.r#type));
        let space = " ".repeat(self.label.as_ref().and_then(|l| l.margin).unwrap_or(1));
        let mut indent = 3 + space.len();
        let mut output = String::new();
        if !self.show_time.is_empty() {
            indent += self.show_time.len();
            let time = dsh_cosmokit::template(&self.show_time, &chrono::Local::now());
            output += &color(self.colors, 8, &time, "");
        }
        let code = name_code(&message.name, self.colors);
        let label_text = color(self.colors, code, &message.name, ";1");
        let width = self.label.as_ref().and_then(|l| l.width).unwrap_or(0);
        let pad_length = width + label_text.len() - message.name.len();
        if self.label.as_ref().and_then(|l| l.align.as_deref()) == Some("right") {
            output += &format!("{:>pad_length$}", label_text);
            output += &space;
            output += &prefix;
            output += &space;
            indent += width + space.len();
        } else {
            output += &prefix;
            output += &space;
            output += &format!("{: <pad_length$}", label_text);
            output += &space;
        }
        output +=
            &format_message(message, self).replace('\n', &format!("\n{}", " ".repeat(indent)));
        if self.show_diff {
            let mut timestamp = self.timestamp.lock();
            if *timestamp != 0 {
                let diff = message.ts as i64 - *timestamp;
                output += &color(
                    self.colors,
                    code,
                    &format!(" +{}", dsh_cosmokit::format(diff)),
                    "",
                );
            }
            *timestamp = message.ts as i64;
        }
        output
    }
}

fn logger_type_letter(r#type: LoggerType) -> char {
    match r#type {
        LoggerType::Error => 'E',
        LoggerType::Info => 'I',
        LoggerType::Warn => 'W',
        LoggerType::Debug => 'D',
    }
}

impl Exporter for ConsoleExporter {
    fn default_level(&self) -> LoggerLevel {
        LoggerLevel::Info
    }

    fn levels(&self) -> &HashMap<String, LoggerLevel> {
        &self.levels
    }

    fn export(&self, message: &Message) {
        println!("{}", self.render(message));
    }
}

/// Detect terminal color support (supports-color approximation).
pub fn detect_color_level() -> Option<usize> {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_some() {
        return None;
    }
    if std::env::var_os("FORCE_COLOR").is_some() {
        return Some(3);
    }
    if std::env::var_os("TERM").is_some_and(|term| term == "dumb") {
        return None;
    }
    if std::io::stdout().is_terminal() {
        return Some(1);
    }
    None
}

/// Console logger plugin entrypoint (`export default ConsoleExporter` in TS).
pub fn plugin() -> Arc<dyn Plugin> {
    Arc::new(ConsolePlugin)
}

struct ConsolePlugin;

#[async_trait::async_trait]
impl Plugin for ConsolePlugin {
    fn name(&self) -> Option<&'static str> {
        Some("logger-console")
    }

    async fn apply(&self, ctx: &Context, config: ArcValue) -> Result<(), PluginError> {
        let raw = cordis::downcast::<serde_json::Value>(&config)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let config: ConsoleConfig = serde_json::from_value(raw).unwrap_or_default();
        let exporter = ConsoleExporter::new(&config);
        ctx.logger.exporter(ctx, exporter);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cordis::Message;

    fn message(name: &str, args: Vec<ArcValue>) -> Message {
        Message {
            sn: 1,
            ts: 1000,
            name: name.to_string(),
            r#type: LoggerType::Info,
            level: LoggerLevel::Info,
            args,
        }
    }

    fn exporter() -> ConsoleExporter {
        let config = ConsoleConfig::default();
        let exporter = ConsoleExporter::new(&config);
        Arc::try_unwrap(exporter).expect("exclusive exporter")
    }

    #[test]
    fn render_basic_message() {
        let exporter = exporter();
        let output = exporter.render(&message("test", vec![arc("hello".to_string())]));
        assert!(output.contains("[I]"), "got: {output}");
        assert!(output.contains("test"), "got: {output}");
        assert!(output.contains("hello"), "got: {output}");
    }

    #[test]
    fn render_without_colors_and_time() {
        let config = ConsoleConfig {
            colors: Some(ColorLevel::Disabled),
            show_time: Some(String::new()),
            ..ConsoleConfig::default()
        };
        let exporter = Arc::try_unwrap(ConsoleExporter::new(&config)).unwrap();
        let output = exporter.render(&message(
            "abc",
            vec![arc("%s!".to_string()), arc("hi".to_string())],
        ));
        assert_eq!(output, "[I] abc hi!");
    }

    #[test]
    fn placeholder_substitution() {
        let exporter = exporter();
        let output = exporter.render(&message(
            "x",
            vec![
                arc("%s %d %f".to_string()),
                arc("n".to_string()),
                arc(3.7f64),
                arc(2.5f64),
            ],
        ));
        assert!(output.contains("n 3 2.5"), "got: {output}");
    }

    #[test]
    fn max_length_truncates_lines() {
        let config = ConsoleConfig {
            max_length: Some(5),
            colors: Some(ColorLevel::Disabled),
            show_time: Some(String::new()),
            ..ConsoleConfig::default()
        };
        let exporter = Arc::try_unwrap(ConsoleExporter::new(&config)).unwrap();
        let output = exporter.render(&message("x", vec![arc("abcdefghij".to_string())]));
        assert!(output.ends_with("abcde..."), "got: {output}");
    }

    #[test]
    fn label_right_alignment() {
        let config = ConsoleConfig {
            colors: Some(ColorLevel::Disabled),
            show_time: Some(String::new()),
            label: Some(LabelStyle {
                width: Some(8),
                margin: Some(1),
                align: Some("right".to_string()),
            }),
            ..ConsoleConfig::default()
        };
        let exporter = Arc::try_unwrap(ConsoleExporter::new(&config)).unwrap();
        let output = exporter.render(&message("ab", vec![arc("x".to_string())]));
        assert!(output.starts_with("      ab [I]"), "got: {output}");
    }

    #[test]
    fn name_code_is_stable_and_bounded() {
        for name in ["loader", "timer", "session", "agent", "web"] {
            let code = name_code(name, Some(3));
            assert!(code < 256, "{name}: {code}");
            assert_eq!(code, name_code(name, Some(3)));
        }
    }

    #[test]
    fn color_escaping() {
        assert_eq!(color(None, 6, "x", ""), "x");
        assert_eq!(color(Some(0), 6, "x", ""), "x");
        assert!(color(Some(1), 6, "x", "").contains("\u{1b}[36m"));
        assert!(color(Some(2), 200, "x", ";1").contains("8;5;200;1"));
    }
}
