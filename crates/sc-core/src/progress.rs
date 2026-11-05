//! 外部ツールの出力から進捗を解析する（要件 F-PG-02、基本設計 §7）。
//!
//! テストには実際のツールの出力（`tests/fixtures/progress/`）を使う。

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;

/// 解析結果。分からない項目は `None`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProgressUpdate {
    /// 進捗率（0.0〜1.0）
    pub ratio: Option<f32>,
    /// 処理済みフレーム数
    pub frames: Option<u64>,
    /// 総フレーム数
    pub total_frames: Option<u64>,
    /// 処理速度（fps）
    pub fps: Option<f32>,
    /// 残り時間
    pub eta: Option<Duration>,
    /// 予想ファイルサイズ（バイト）
    pub estimated_size: Option<u64>,
}

/// 進捗パーサ。1 行ずつ渡し、進捗が分かった行で `Some` を返す。
pub trait ProgressParser: Send {
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate>;
}

/// 実行中は 100% を出さない（終わったときに呼び出し側で 100% にする）。
const RUNNING_MAX: f32 = 0.99;

/// ツール名（`cmdline::tool_name`）とコマンド全体から、パーサを選ぶ。
pub fn parser_for(tool: &str, command: &str) -> Box<dyn ProgressParser> {
    let t = tool.to_ascii_lowercase();
    match t.as_str() {
        "ffmpeg" => Box::new(FfmpegParser::new(command)),
        "qsvencc" | "nvencc" | "vceencc" | "mppenc" | "rkmppenc" => Box::new(RigayaParser),
        "x264" | "x265" => Box::new(X26xParser),
        "mp4box" => Box::new(Mp4BoxParser),
        _ => Box::new(GenericParser),
    }
}

// ---------------------------------------------------------------------------
// ffmpeg

/// ffmpeg：映像があればフレーム数、なければ時間の比率で進捗を出す（基本設計 §7.1）。
pub struct FfmpegParser {
    section: Section,
    duration: Option<f64>,
    input_fps: Option<f64>,
    output_fps: Option<f64>,
    output_has_video: bool,
    seek: Option<f64>,
    limit: Option<f64>,
    until: Option<f64>,
}

#[derive(PartialEq)]
enum Section {
    None,
    Input,
    Output,
}

static RE_DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Duration:\s*(\d+):(\d\d):(\d\d(?:\.\d+)?)").unwrap());
static RE_FPS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([\d.]+)\s+fps").unwrap());
static RE_KV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\w+)=\s*([^\s]+)").unwrap());

impl FfmpegParser {
    pub fn new(command: &str) -> Self {
        let words: Vec<&str> = command
            .split_whitespace()
            .map(|w| w.trim_matches('"'))
            .collect();
        let opt = |name: &str| {
            words
                .windows(2)
                .find(|w| w[0] == name)
                .and_then(|w| parse_time(w[1]))
        };
        Self {
            section: Section::None,
            duration: None,
            input_fps: None,
            output_fps: None,
            output_has_video: false,
            seek: opt("-ss"),
            limit: opt("-t"),
            until: opt("-to"),
        }
    }

    /// `-ss` / `-t` / `-to` を考慮した、処理する長さ（秒）。
    fn effective_duration(&self) -> Option<f64> {
        let mut d = self.duration?;
        let ss = self.seek.unwrap_or(0.0);
        if let Some(to) = self.until {
            d = d.min(to);
        }
        d -= ss;
        if let Some(t) = self.limit {
            d = d.min(t);
        }
        (d > 0.0).then_some(d)
    }
}

impl ProgressParser for FfmpegParser {
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate> {
        let t = line.trim_start();
        if t.starts_with("Input #") {
            self.section = Section::Input;
            return None;
        }
        if t.starts_with("Output #") {
            self.section = Section::Output;
            return None;
        }
        if self.section == Section::Input && self.duration.is_none() {
            if let Some(c) = RE_DURATION.captures(t) {
                self.duration = Some(hms(&c[1], &c[2], &c[3]));
                return None;
            }
        }
        if t.starts_with("Stream #") && t.contains(": Video:") {
            let fps = RE_FPS.captures(t).and_then(|c| c[1].parse::<f64>().ok());
            match self.section {
                Section::Input if self.input_fps.is_none() => self.input_fps = fps,
                Section::Output => {
                    self.output_has_video = true;
                    if self.output_fps.is_none() {
                        self.output_fps = fps;
                    }
                }
                _ => {}
            }
            return None;
        }
        if !t.contains("time=") {
            return None;
        }

        let mut frame = None;
        let mut fps = None;
        let mut size = None;
        let mut time = None;
        let mut speed = None;
        for c in RE_KV.captures_iter(t) {
            let v = &c[2];
            match &c[1] {
                "frame" => frame = v.parse::<u64>().ok(),
                "fps" => fps = v.parse::<f32>().ok(),
                "size" | "Lsize" => size = parse_size(v),
                "time" => time = parse_time(v),
                "speed" => speed = v.trim_end_matches('x').parse::<f64>().ok(),
                _ => {}
            }
        }

        let duration = self.effective_duration();
        let rate = self.output_fps.or(self.input_fps);
        let total_frames = match (self.output_has_video, duration, rate) {
            (true, Some(d), Some(r)) => Some((d * r).round() as u64),
            _ => None,
        };
        let ratio = match (frame, total_frames) {
            (Some(f), Some(total)) if total > 0 && self.output_has_video => {
                Some(f as f64 / total as f64)
            }
            _ => match (time, duration) {
                (Some(now), Some(d)) => Some(now / d),
                _ => None,
            },
        }
        .map(|r| (r as f32).clamp(0.0, RUNNING_MAX));

        let eta = match (frame, total_frames, fps) {
            (Some(f), Some(total), Some(fps)) if fps > 0.0 && self.output_has_video => Some(
                Duration::from_secs_f64(total.saturating_sub(f) as f64 / fps as f64),
            ),
            _ => match (time, duration, speed) {
                (Some(now), Some(d), Some(s)) if s > 0.0 => {
                    Some(Duration::from_secs_f64(((d - now) / s).max(0.0)))
                }
                _ => None,
            },
        };
        let estimated_size = match (size, ratio) {
            (Some(s), Some(r)) if r > 0.05 => Some((s as f64 / r as f64) as u64),
            _ => None,
        };

        Some(ProgressUpdate {
            ratio,
            frames: frame.filter(|_| self.output_has_video),
            total_frames,
            fps: fps.filter(|_| self.output_has_video),
            eta,
            estimated_size,
        })
    }
}

// ---------------------------------------------------------------------------
// rigaya 系（QSVEncC / NVEncC / VCEEncC）

static RE_RIGAYA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(\d+(?:\.\d+)?)%\]\s+(\d+)\s+frames:\s+([\d.]+)\s+fps").unwrap()
});
static RE_REMAIN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"remain\s+(\d+):(\d\d):(\d\d)").unwrap());
static RE_EST_SIZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"est(?:\.|\s+out)\s*size\s+([\d.]+)\s*([KMG])i?B").unwrap());

/// `[9.7%] 684 frames: 1404.52 fps, 495 kbps, remain 0:00:04, …, est out size 13.9MB`
pub struct RigayaParser;

impl ProgressParser for RigayaParser {
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate> {
        let c = RE_RIGAYA.captures(line)?;
        let percent: f32 = c[1].parse().ok()?;
        Some(ProgressUpdate {
            ratio: Some((percent / 100.0).clamp(0.0, RUNNING_MAX)),
            frames: c[2].parse().ok(),
            total_frames: None,
            fps: c[3].parse().ok(),
            eta: RE_REMAIN
                .captures(line)
                .map(|r| Duration::from_secs_f64(hms(&r[1], &r[2], &r[3]))),
            estimated_size: est_size(line),
        })
    }
}

// ---------------------------------------------------------------------------
// x264 / x265

static RE_X26X: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(\d+(?:\.\d+)?)%\]\s+(\d+)/(\d+)\s+frames,\s+([\d.]+)\s+fps").unwrap()
});
static RE_ETA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"eta\s+(\d+):(\d\d):(\d\d)").unwrap());

/// `[12.3%] 1234/5678 frames, 123.45 fps, 1234.56 kb/s, eta 0:00:30, est.size 12.3 MB`
pub struct X26xParser;

impl ProgressParser for X26xParser {
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate> {
        let c = RE_X26X.captures(line)?;
        let percent: f32 = c[1].parse().ok()?;
        Some(ProgressUpdate {
            ratio: Some((percent / 100.0).clamp(0.0, RUNNING_MAX)),
            frames: c[2].parse().ok(),
            total_frames: c[3].parse().ok(),
            fps: c[4].parse().ok(),
            eta: RE_ETA
                .captures(line)
                .map(|r| Duration::from_secs_f64(hms(&r[1], &r[2], &r[3]))),
            estimated_size: est_size(line),
        })
    }
}

// ---------------------------------------------------------------------------
// MP4Box

static RE_MP4BOX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\((\d+)/100\)").unwrap());

/// `Import: |=====               | (25/100)`
pub struct Mp4BoxParser;

impl ProgressParser for Mp4BoxParser {
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate> {
        let c = RE_MP4BOX.captures(line)?;
        let n: f32 = c[1].parse().ok()?;
        Some(ProgressUpdate {
            ratio: Some((n / 100.0).clamp(0.0, RUNNING_MAX)),
            ..Default::default()
        })
    }
}

// ---------------------------------------------------------------------------
// 汎用

static RE_PERCENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{1,3}(?:\.\d+)?)\s?%").unwrap());

/// 行の中の `xx.x%` を拾う（opusenc の `[/] 23% 00:01:09.23 69.2x realtime` など）。
pub struct GenericParser;

impl ProgressParser for GenericParser {
    fn feed(&mut self, line: &str) -> Option<ProgressUpdate> {
        let c = RE_PERCENT.captures_iter(line).last()?;
        let percent: f32 = c[1].parse().ok()?;
        (percent <= 100.0).then(|| ProgressUpdate {
            ratio: Some((percent / 100.0).clamp(0.0, RUNNING_MAX)),
            ..Default::default()
        })
    }
}

// ---------------------------------------------------------------------------
// 補助

fn hms(h: &str, m: &str, s: &str) -> f64 {
    h.parse::<f64>().unwrap_or(0.0) * 3600.0
        + m.parse::<f64>().unwrap_or(0.0) * 60.0
        + s.parse::<f64>().unwrap_or(0.0)
}

/// `00:01:09.23`、`1:09`、`69.5` を秒にする。
fn parse_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    let nums: Option<Vec<f64>> = parts.iter().map(|p| p.parse::<f64>().ok()).collect();
    let nums = nums?;
    Some(match nums.as_slice() {
        [s] => *s,
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    })
}

/// `6656KiB`、`512kB`、`1.5MiB` をバイトにする。
fn parse_size(s: &str) -> Option<u64> {
    let idx = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let n: f64 = s[..idx].parse().ok()?;
    let mult = match s[idx..].to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kb" | "kib" => 1024.0,
        "mb" | "mib" => 1024.0 * 1024.0,
        "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

fn est_size(line: &str) -> Option<u64> {
    let c = RE_EST_SIZE.captures(line)?;
    let n: f64 = c[1].parse().ok()?;
    let mult = match &c[2] {
        "K" => 1024.0,
        "M" => 1024.0 * 1024.0,
        _ => 1024.0 * 1024.0 * 1024.0,
    };
    Some((n * mult) as u64)
}

/// 出力の塊を行に分ける。`\r` で終わる行は「同じ行の上書き」として `true` を付ける。
pub fn split_lines(text: &str) -> Vec<(&str, bool)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push((&text[start..i], false));
                start = i + 1;
            }
            b'\r' => {
                if bytes.get(i + 1) == Some(&b'\n') {
                    out.push((&text[start..i], false));
                    i += 1;
                } else {
                    out.push((&text[start..i], true));
                }
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if start < text.len() {
        out.push((&text[start..], false));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(parser: &mut dyn ProgressParser, text: &str) -> Vec<ProgressUpdate> {
        split_lines(text)
            .into_iter()
            .filter_map(|(l, _)| parser.feed(l))
            .collect()
    }

    fn fixture(name: &str) -> String {
        let path = format!(
            "{}/tests/fixtures/progress/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        String::from_utf8_lossy(&std::fs::read(path).unwrap()).into_owned()
    }

    #[test]
    fn ffmpeg_video_uses_frames() {
        let mut p =
            FfmpegParser::new("ffmpeg -y -i sample.ts -c:v libx264 -preset medium -an video.mp4");
        let ups = feed_all(&mut p, &fixture("ffmpeg_video.txt"));
        assert!(ups.len() > 5, "progress lines: {}", ups.len());
        // 5 分 × 29.97fps ＝ 8991 フレーム
        assert_eq!(ups[0].total_frames, Some(8991));
        // 開始直後の行は frame=0 fps=0.0 なので、途中の行で確かめる
        let first = &ups[ups.len() / 2];
        assert!(first.frames.unwrap() > 0 && first.fps.unwrap() > 0.0);
        let r = first.ratio.unwrap();
        let expect = first.frames.unwrap() as f32 / 8991.0;
        assert!((r - expect).abs() < 1e-4);
        assert!(first.eta.is_some());
        // 比率は増えていき、99% で止まる
        assert!(ups.windows(2).all(|w| w[0].ratio <= w[1].ratio));
        assert!(ups.last().unwrap().ratio.unwrap() <= RUNNING_MAX);
    }

    #[test]
    fn ffmpeg_audio_uses_time() {
        let mut p =
            FfmpegParser::new("ffmpeg -y -threads 0 -i sample.ts -vn -c:a pcm_s16le -f wav a.wav");
        let ups = feed_all(&mut p, &fixture("ffmpeg_audio.txt"));
        assert!(!ups.is_empty());
        let u = &ups[0];
        assert!(u.frames.is_none() && u.total_frames.is_none());
        assert!(u.ratio.unwrap() > 0.0);
        assert!(u.eta.is_some());
    }

    #[test]
    fn ffmpeg_cut_and_rate_change() {
        let mut p = FfmpegParser::new(
            "ffmpeg -y -ss 30 -i sample.ts -t 120 -r 24000/1001 -c:v libx264 -preset medium -an cut.mp4",
        );
        let ups = feed_all(&mut p, &fixture("ffmpeg_cut_rate.txt"));
        // 120 秒 × 23.98fps ≒ 2878 フレーム（出力のフレームレートを使う）
        let total = ups[0].total_frames.unwrap();
        assert!((2870..=2880).contains(&total), "total {total}");
    }

    #[test]
    fn qsvencc_progress() {
        let ups = feed_all(&mut RigayaParser, &fixture("qsvencc.txt"));
        assert!(ups.len() >= 3);
        let u = &ups[0];
        assert!((u.ratio.unwrap() - 0.097).abs() < 1e-3);
        assert_eq!(u.frames, Some(684));
        assert_eq!(u.eta, Some(Duration::from_secs(4)));
        assert_eq!(u.estimated_size, Some((13.9 * 1024.0 * 1024.0) as u64));
    }

    #[test]
    fn x264_cli_line() {
        let u = X26xParser
            .feed(
                "[12.3%] 1234/5678 frames, 123.45 fps, 1234.56 kb/s, eta 0:01:30, est.size 12.3 MB",
            )
            .unwrap();
        assert_eq!(u.total_frames, Some(5678));
        assert_eq!(u.eta, Some(Duration::from_secs(90)));
        assert!(u.estimated_size.is_some());
    }

    #[test]
    fn mp4box_progress() {
        let ups = feed_all(&mut Mp4BoxParser, &fixture("mp4box.txt"));
        assert!(ups.len() > 50);
        assert!(ups.iter().any(|u| u.ratio == Some(0.5)));
    }

    #[test]
    fn opusenc_uses_generic_percent() {
        let mut p = parser_for("opusenc", "opusenc a.wav b.opus");
        let ups = feed_all(p.as_mut(), &fixture("opusenc.txt"));
        assert!(ups.iter().any(|u| u.ratio == Some(0.23)));
    }

    #[test]
    fn helpers() {
        assert_eq!(parse_time("00:01:09.5"), Some(69.5));
        assert_eq!(parse_time("1:30"), Some(90.0));
        assert_eq!(parse_time("30"), Some(30.0));
        assert_eq!(parse_size("6656KiB"), Some(6656 * 1024));
        assert_eq!(parse_size("2kB"), Some(2048));
        assert_eq!(
            split_lines("a\rb\r\nc\nd"),
            vec![("a", true), ("b", false), ("c", false), ("d", false)]
        );
    }
}
