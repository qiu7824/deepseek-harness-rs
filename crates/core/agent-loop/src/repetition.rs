//! Conservative output-loop detection. Similar rows with distinct contents are
//! not loops; only an exact contiguous repetition covering most output qualifies.
use std::collections::HashSet;

pub(crate) fn dominated(text: &str) -> bool {
    let bytes = text.as_bytes();
    let n = bytes.len();
    if n < 800 {
        return false;
    }
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if lines.len() >= 5 && lines.iter().copied().collect::<HashSet<_>>().len() * 2 > lines.len() {
        return false;
    }
    const WINDOW: usize = 64;
    let stride = ((n - WINDOW) / 16).max(1);
    for anchor in (0..=n - WINDOW).step_by(stride).take(17) {
        let window = &bytes[anchor..anchor + WINDOW];
        let mut offset = anchor + 1;
        for _ in 0..8 {
            let Some(found) = bytes[offset..]
                .windows(WINDOW)
                .position(|candidate| candidate == window)
            else {
                break;
            };
            let at = offset + found;
            let period = at - anchor;
            offset = at + 1;
            let mut left = anchor;
            while left > 0 && bytes[left - 1] == bytes[left - 1 + period] {
                left -= 1;
            }
            let mut right = anchor + WINDOW;
            while right + period < n && bytes[right] == bytes[right + period] {
                right += 1;
            }
            let run = right + period - left;
            if run >= 5 * period && run >= n / 2 {
                return true;
            }
        }
    }
    false
}

pub(crate) fn explicitly_requested(text: &str) -> bool {
    let text = text.to_lowercase();
    if ["不要", "停止", "别再", "don't", "do not", "stop"]
        .iter()
        .any(|v| text.contains(v))
    {
        return false;
    }
    text.chars().any(|c| c.is_ascii_digit())
        && ["重复", "repeat", "相同的行", "identical rows"]
            .iter()
            .any(|v| text.contains(v))
        && [
            "输出", "打印", "生成", "写", "说", "output", "print", "write", "say", "generate",
        ]
        .iter()
        .any(|v| text.contains(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catches_unicode_and_long_multi_line_loops_without_rejecting_distinct_rows() {
        assert!(dominated(
            &"检查文件后继续检查文件并继续重复相同操作。".repeat(60)
        ));
        let unit = "Checking the same unchanged result repeatedly without making any progress.\nThe operation still has not completed and no new evidence was produced.\n";
        assert!(dominated(&unit.repeat(20)));
        assert!(!dominated(&(0..1000).map(|i|format!("INSERT INTO records VALUES ({i}, 'same long shared prefix repeated in many real rows');\n")).collect::<String>()));
        assert!(!dominated("short repeated phrase repeated phrase"));
        assert!(explicitly_requested("请输出重复文本 1000 次"));
        assert!(!explicitly_requested("不要重复输出 1000 次"));
    }
}
