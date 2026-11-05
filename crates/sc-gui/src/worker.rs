//! 変換を別スレッドで動かし、イベントをチャネルで画面スレッドへ送る（基本設計 §5）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui;
use sc_core::plan::JobPlan;
use sc_core::runner::{self, JobOutcome, RunOptions, RunnerEvent};

use crate::queue::JobId;

/// 画面スレッドへ送るメッセージ。
pub enum WorkerMsg {
    Event(JobId, RunnerEvent),
    Finished(JobId, JobOutcome),
}

/// 実行中のジョブ 1 件。
pub struct Worker {
    abort: Arc<AtomicBool>,
    rx: Receiver<WorkerMsg>,
    handle: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(job: JobId, plan: JobPlan, opts: RunOptions, ctx: egui::Context) -> Self {
        let abort = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let flag = abort.clone();
        let handle = std::thread::Builder::new()
            .name(format!("job-{job}"))
            .spawn(move || {
                // パニックしてもジョブを失敗にして画面に知らせる（基本設計 §10）。
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runner::run_job(&plan, &opts, &flag, &mut |e| {
                        let _ = tx.send(WorkerMsg::Event(job, e));
                        ctx.request_repaint();
                    })
                }));
                let outcome = result.unwrap_or_else(|_| {
                    JobOutcome::Failed(
                        "内部エラーが発生しました（Runner スレッドの異常終了）".into(),
                    )
                });
                let _ = tx.send(WorkerMsg::Finished(job, outcome));
                ctx.request_repaint();
            })
            .expect("スレッドを作れません");
        Self {
            abort,
            rx,
            handle: Some(handle),
        }
    }

    pub fn abort(&self) {
        self.abort.store(true, Ordering::SeqCst);
    }

    /// 届いているメッセージをすべて取り出す。
    pub fn drain(&self) -> Vec<WorkerMsg> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.abort();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
