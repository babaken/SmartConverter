//! 変換キュー（基本設計 §3.2〜3.3、§4.2）。画面に依存しない部分。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use sc_core::progress::ProgressUpdate;

pub type JobId = u64;

/// ジョブの状態（基本設計 §3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Aborted,
    Skipped,
    PresetMissing,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            JobState::Pending => "待機",
            JobState::Running => "実行中",
            JobState::Succeeded => "完了",
            JobState::Failed => "失敗",
            JobState::Aborted => "中断",
            JobState::Skipped => "スキップ",
            JobState::PresetMissing => "プリセットなし",
        }
    }

    /// 終わった状態か（再実行の対象）。
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobState::Succeeded
                | JobState::Failed
                | JobState::Aborted
                | JobState::Skipped
                | JobState::PresetMissing
        )
    }
}

/// 実行中の進み具合。
#[derive(Debug, Clone, Default)]
pub struct JobProgress {
    /// 実行中のコマンド（1 始まり）と総数
    pub command: usize,
    pub command_count: usize,
    pub tool: String,
    pub update: ProgressUpdate,
}

/// キューの 1 行。
#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub input: PathBuf,
    pub preset_title: String,
    pub state: JobState,
    pub progress: JobProgress,
    pub started_at: Option<Instant>,
    pub elapsed: Option<Duration>,
    /// 出力ファイル（拡張子なし）。実行開始時に決まる
    pub output_base: Option<PathBuf>,
    /// 出力されたファイル
    pub outputs: Vec<PathBuf>,
    /// 失敗理由など
    pub message: Option<String>,
}

impl Job {
    pub fn file_name(&self) -> String {
        self.input
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// 経過時間（実行中は今まで、終わったら確定値）。
    pub fn elapsed_now(&self) -> Option<Duration> {
        match (self.state, self.started_at) {
            (JobState::Running, Some(t)) => Some(t.elapsed()),
            _ => self.elapsed,
        }
    }
}

/// キュー全体。
#[derive(Debug, Default)]
pub struct Queue {
    pub jobs: Vec<Job>,
    pub selected: BTreeSet<JobId>,
    next_id: JobId,
}

impl Queue {
    /// 末尾に待機中のジョブを追加する。同じファイルでも重複して追加する（画面設計 §4.3）。
    pub fn add(&mut self, input: PathBuf, preset_title: &str) -> JobId {
        self.next_id += 1;
        let id = self.next_id;
        self.jobs.push(Job {
            id,
            input,
            preset_title: preset_title.to_owned(),
            state: JobState::Pending,
            progress: JobProgress::default(),
            started_at: None,
            elapsed: None,
            output_base: None,
            outputs: Vec::new(),
            message: None,
        });
        id
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|j| j.id == id)
    }

    pub fn get_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.id == id)
    }

    pub fn running(&self) -> Option<&Job> {
        self.jobs.iter().find(|j| j.state == JobState::Running)
    }

    /// 次に実行する待機中のジョブ（上から順）。
    pub fn next_pending(&self) -> Option<JobId> {
        self.jobs
            .iter()
            .find(|j| j.state == JobState::Pending)
            .map(|j| j.id)
    }

    pub fn count(&self, state: JobState) -> usize {
        self.jobs.iter().filter(|j| j.state == state).count()
    }

    /// ステータスバーの件数（中断は失敗と分けて数える）。
    pub fn status_text(&self) -> String {
        format!(
            "待機 {} / 実行中 {} / 完了 {} / 中断 {} / 失敗 {}",
            self.count(JobState::Pending),
            self.count(JobState::Running),
            self.count(JobState::Succeeded),
            self.count(JobState::Aborted),
            self.count(JobState::Failed) + self.count(JobState::PresetMissing),
        )
    }

    /// 全部終わったときの通知の本文（中断があったときだけ中断を出す）。
    pub fn finished_text(&self) -> String {
        let ok = self.count(JobState::Succeeded);
        let aborted = self.count(JobState::Aborted);
        let ng = self.count(JobState::Failed) + self.count(JobState::PresetMissing);
        if aborted > 0 {
            format!("完了 {ok} / 中断 {aborted} / 失敗 {ng}")
        } else {
            format!("完了 {ok} / 失敗 {ng}")
        }
    }

    /// 選択中のジョブを削除する（実行中は除く）。消した数を返す。
    pub fn remove_selected(&mut self) -> usize {
        let before = self.jobs.len();
        let selected = &self.selected;
        self.jobs
            .retain(|j| j.state == JobState::Running || !selected.contains(&j.id));
        self.selected
            .retain(|id| self.jobs.iter().any(|j| j.id == *id));
        before - self.jobs.len()
    }

    /// 完了したジョブを一覧から消す。
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| j.state != JobState::Succeeded);
        self.selected
            .retain(|id| self.jobs.iter().any(|j| j.id == *id));
    }

    /// 選択中のジョブのプリセットを変える（実行中を除く）。終わったジョブは待機に戻す。
    pub fn set_preset_for_selected(&mut self, title: &str) {
        for j in self
            .jobs
            .iter_mut()
            .filter(|j| self.selected.contains(&j.id))
        {
            if j.state == JobState::Running {
                continue;
            }
            j.preset_title = title.to_owned();
            if j.state.is_finished() {
                reset(j);
            }
        }
    }

    /// 選択中の、終わったジョブを待機に戻す。
    pub fn retry_selected(&mut self) {
        for j in self
            .jobs
            .iter_mut()
            .filter(|j| self.selected.contains(&j.id))
        {
            if j.state.is_finished() {
                reset(j);
            }
        }
    }

    /// 選択中の待機ジョブを動かす（`delta` が負なら上へ）。
    pub fn move_selected(&mut self, to: Move) {
        let selected: Vec<usize> = self
            .jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| self.selected.contains(&j.id) && j.state == JobState::Pending)
            .map(|(i, _)| i)
            .collect();
        if selected.is_empty() {
            return;
        }
        match to {
            Move::Up => {
                for &i in &selected {
                    if i > 0 && !selected.contains(&(i - 1)) {
                        self.jobs.swap(i, i - 1);
                    }
                }
            }
            Move::Down => {
                for &i in selected.iter().rev() {
                    if i + 1 < self.jobs.len() && !selected.contains(&(i + 1)) {
                        self.jobs.swap(i, i + 1);
                    }
                }
            }
            Move::Top | Move::Bottom => {
                let mut moving = Vec::new();
                for &i in selected.iter().rev() {
                    moving.push(self.jobs.remove(i));
                }
                moving.reverse();
                if to == Move::Top {
                    for (k, j) in moving.into_iter().enumerate() {
                        self.jobs.insert(k, j);
                    }
                } else {
                    self.jobs.extend(moving);
                }
            }
        }
    }

    /// プリセットのタイトルを変えたとき、そのプリセットを使っているジョブ（実行中を除く）も付け替える。
    pub fn rename_preset(&mut self, old: &str, new: &str) {
        for j in &mut self.jobs {
            if j.state != JobState::Running && j.preset_title == old {
                j.preset_title = new.to_owned();
            }
        }
    }

    /// プリセットの再読み込み後：タイトルが見つからない待機中のジョブを「プリセットなし」にする。
    pub fn mark_missing_presets(&mut self, exists: impl Fn(&str) -> bool) -> usize {
        let mut n = 0;
        for j in &mut self.jobs {
            if j.state == JobState::Pending && !exists(&j.preset_title) {
                j.state = JobState::PresetMissing;
                j.message = Some(format!("プリセット「{}」がありません", j.preset_title));
                n += 1;
            }
        }
        n
    }
}

/// 並べ替えの方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Top,
    Bottom,
}

fn reset(j: &mut Job) {
    j.state = JobState::Pending;
    j.progress = JobProgress::default();
    j.started_at = None;
    j.elapsed = None;
    j.output_base = None;
    j.outputs.clear();
    j.message = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(n: usize) -> Queue {
        let mut q = Queue::default();
        for i in 0..n {
            q.add(PathBuf::from(format!("{i}.ts")), "P");
        }
        q
    }

    fn order(q: &Queue) -> Vec<JobId> {
        q.jobs.iter().map(|j| j.id).collect()
    }

    #[test]
    fn aborted_is_counted_apart_from_failed() {
        let mut q = queue(5);
        q.jobs[0].state = JobState::Succeeded;
        q.jobs[1].state = JobState::Aborted;
        q.jobs[2].state = JobState::Failed;
        q.jobs[3].state = JobState::PresetMissing;
        assert_eq!(
            q.status_text(),
            "待機 1 / 実行中 0 / 完了 1 / 中断 1 / 失敗 2"
        );
        q.jobs[4].state = JobState::Succeeded;
        assert_eq!(q.finished_text(), "完了 2 / 中断 1 / 失敗 2");
        q.jobs[1].state = JobState::Succeeded;
        assert_eq!(q.finished_text(), "完了 3 / 失敗 2");
    }

    #[test]
    fn next_pending_is_top_most() {
        let mut q = queue(3);
        q.jobs[0].state = JobState::Succeeded;
        assert_eq!(q.next_pending(), Some(2));
    }

    #[test]
    fn remove_keeps_running() {
        let mut q = queue(3);
        q.jobs[1].state = JobState::Running;
        q.selected = [1, 2].into();
        assert_eq!(q.remove_selected(), 1);
        assert_eq!(order(&q), vec![2, 3]);
        assert_eq!(q.selected, [2].into());
    }

    #[test]
    fn move_up_down_top_bottom() {
        let mut q = queue(4);
        q.selected = [3].into();
        q.move_selected(Move::Up);
        assert_eq!(order(&q), vec![1, 3, 2, 4]);
        q.move_selected(Move::Top);
        assert_eq!(order(&q), vec![3, 1, 2, 4]);
        q.move_selected(Move::Bottom);
        assert_eq!(order(&q), vec![1, 2, 4, 3]);
        q.move_selected(Move::Down);
        assert_eq!(order(&q), vec![1, 2, 4, 3]);
        // 実行中・完了は動かさない
        q.jobs[0].state = JobState::Succeeded;
        q.selected = [1].into();
        q.move_selected(Move::Down);
        assert_eq!(order(&q), vec![1, 2, 4, 3]);
    }

    #[test]
    fn change_preset_resets_finished_jobs() {
        let mut q = queue(2);
        q.jobs[0].state = JobState::Failed;
        q.jobs[0].message = Some("x".into());
        q.jobs[1].state = JobState::Running;
        q.selected = [1, 2].into();
        q.set_preset_for_selected("Q");
        assert_eq!(q.jobs[0].preset_title, "Q");
        assert_eq!(q.jobs[0].state, JobState::Pending);
        assert!(q.jobs[0].message.is_none());
        assert_eq!(q.jobs[1].preset_title, "P");
    }

    #[test]
    fn rename_skips_running_job() {
        let mut q = queue(3);
        q.jobs[1].state = JobState::Running;
        q.jobs[2].preset_title = "other".into();
        q.rename_preset("P", "P2");
        assert_eq!(q.jobs[0].preset_title, "P2");
        assert_eq!(q.jobs[1].preset_title, "P");
        assert_eq!(q.jobs[2].preset_title, "other");
    }

    #[test]
    fn missing_presets_are_marked() {
        let mut q = queue(2);
        q.jobs[1].preset_title = "gone".into();
        assert_eq!(q.mark_missing_presets(|t| t == "P"), 1);
        assert_eq!(q.jobs[1].state, JobState::PresetMissing);
        q.selected = [2].into();
        q.set_preset_for_selected("P");
        assert_eq!(q.jobs[1].state, JobState::Pending);
    }
}
