//! OS 依存の処理（基本設計 §6.3）。`#[cfg]` はこのモジュールの中だけに書く。
//!
//! | 処理 | Windows | Linux |
//! |---|---|---|
//! | コンソール非表示 | `CREATE_NO_WINDOW` | 不要 |
//! | 優先度 0/1/2/3 | IDLE / NORMAL / HIGH / REALTIME | nice 19 / 0 / -5 / -10 |
//! | プロセスツリー | Job Object（`KILL_ON_JOB_CLOSE`）。止めた状態で起動し、Job に入れてから動かす | 新しいプロセスグループ |
//! | 中断 | `TerminateJobObject` | `killpg(SIGTERM)` → 残れば `SIGKILL` |

use std::process::{Child, Command};

/// 起動前に Command へ OS ごとの設定をする。戻り値は注意事項（ログに出す）。
///
/// Windows では止めた状態（`CREATE_SUSPENDED`）で起動するので、起動したら必ず
/// [`ProcessTree::attach`] を呼んで動かすこと。
pub fn configure(cmd: &mut Command, priority: u8) -> Option<String> {
    imp::configure(cmd, priority)
}

/// そのプロセス ID のプロセスが動いているか（残った一時フォルダを片付けてよいかの判定用）。
///
/// 調べられないとき（権限がないなど）は、動いているものとして扱う（消しすぎないように）。
pub fn process_exists(pid: u32) -> bool {
    imp::process_exists(pid)
}

/// レジストリの文字列の値を読む（Windows のみ。HKEY_CURRENT_USER → HKEY_LOCAL_MACHINE の順）。
/// `subkey` は `SOFTWARE\VapourSynth` の形。ほかの OS では常に `None`。
pub fn registry_string(subkey: &str, value: &str) -> Option<String> {
    imp::registry_string(subkey, value)
}

/// 起動した子プロセス（とその子孫）をまとめて管理する。
pub struct ProcessTree {
    inner: imp::Tree,
}

impl ProcessTree {
    /// 起動直後に呼ぶ。Windows では、止めた状態で起動したプロセスを Job Object に入れてから動かす
    /// （入れる前に孫プロセスを起動されると、その孫は Job の外に出てしまうため）。
    /// 動かせなかったときはエラー（プロセスは終了させてある）。
    pub fn attach(child: &mut Child) -> std::io::Result<Self> {
        Ok(Self {
            inner: imp::Tree::attach(child)?,
        })
    }

    /// 終了を求める（Windows はすぐ終了、Linux は SIGTERM）。
    pub fn terminate(&self) {
        self.inner.terminate();
    }

    /// 強制終了する（Linux は SIGKILL。Windows は terminate と同じ）。
    pub fn kill(&self) {
        self.inner.kill();
    }
}

#[cfg(windows)]
mod imp {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Child, Command};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, OpenThread, ResumeThread, CREATE_NO_WINDOW,
        CREATE_SUSPENDED, HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        PROCESS_QUERY_LIMITED_INFORMATION, REALTIME_PRIORITY_CLASS, THREAD_SUSPEND_RESUME,
    };

    pub fn registry_string(subkey: &str, value: &str) -> Option<String> {
        use windows_sys::Win32::System::Registry::{
            RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ,
            RRF_RT_REG_SZ,
        };
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (subkey, value) = (wide(subkey), wide(value));
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let mut buf = vec![0u16; 1024];
            let mut bytes = (buf.len() * 2) as u32;
            // SAFETY: 文字列は NUL 終端。buf の大きさは bytes で渡す。REG_EXPAND_SZ は展開される。
            let err = unsafe {
                RegGetValueW(
                    root,
                    subkey.as_ptr(),
                    value.as_ptr(),
                    RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
                    std::ptr::null_mut(),
                    buf.as_mut_ptr().cast(),
                    &mut bytes,
                )
            };
            if err == 0 {
                let len = (bytes as usize / 2).min(buf.len());
                let s = String::from_utf16_lossy(&buf[..len]);
                let s = s.trim_end_matches('\0').to_owned();
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }

    /// GetExitCodeProcess が返す「まだ動いている」（STILL_ACTIVE）。
    const STILL_ACTIVE: u32 = 259;

    pub fn process_exists(pid: u32) -> bool {
        // SAFETY: OpenProcess は失敗すると null を返す。開けたハンドルは 1 回だけ閉じる。
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                // ERROR_INVALID_PARAMETER（87）はその ID のプロセスがない。ほかは分からないので動いている扱い。
                return windows_sys::Win32::Foundation::GetLastError() != 87;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            !ok || code == STILL_ACTIVE
        }
    }

    pub fn configure(cmd: &mut Command, priority: u8) -> Option<String> {
        let class = match priority {
            0 => IDLE_PRIORITY_CLASS,
            1 => NORMAL_PRIORITY_CLASS,
            2 => HIGH_PRIORITY_CLASS,
            _ => REALTIME_PRIORITY_CLASS,
        };
        // 止めた状態で起動し、Tree::attach で Job Object に入れてから動かす。
        cmd.creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED | class);
        None
    }

    /// 止めた状態で起動したプロセスのスレッドを動かす。
    ///
    /// `std::process::Child` はメインスレッドのハンドルを持たないので、スナップショットで
    /// そのプロセスのスレッドを探す（起動直後で止まっているので、スレッドは 1 つだけ）。
    fn resume_threads(pid: u32) -> std::io::Result<()> {
        // SAFETY: スナップショットのハンドルは最後に閉じる。THREADENTRY32 は dwSize を入れてから渡す。
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut resumed = 0;
            let mut error = None;
            let mut ok = Thread32First(snap, &mut entry) != 0;
            while ok {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if thread.is_null() {
                        error = Some(std::io::Error::last_os_error());
                    } else {
                        if ResumeThread(thread) == u32::MAX {
                            error = Some(std::io::Error::last_os_error());
                        } else {
                            resumed += 1;
                        }
                        CloseHandle(thread);
                    }
                }
                ok = Thread32Next(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
            match (resumed, error) {
                (0, Some(e)) => Err(e),
                (0, None) => Err(std::io::Error::other(
                    "起動したプロセスのスレッドが見つかりません",
                )),
                _ => Ok(()),
            }
        }
    }

    /// Job Object。ハンドルを閉じると、中に残っているプロセスも終了する
    /// （アプリが強制終了されても子プロセスが残らない）。
    pub struct Tree {
        job: HANDLE,
    }

    // HANDLE は生ポインタだが、Job Object のハンドルはどのスレッドから使ってもよい。
    unsafe impl Send for Tree {}
    unsafe impl Sync for Tree {}

    impl Tree {
        pub fn attach(child: &mut Child) -> std::io::Result<Self> {
            // SAFETY: 引数はすべて有効（null は「名前なし・既定のセキュリティ」）。
            let job = unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if !job.is_null() {
                    let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as *const core::ffi::c_void,
                        std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                    );
                    // まだ動いていないので、孫プロセスを起動される前に Job に入る。
                    // 入れられなくても（古い Windows で入れ子の Job が使えないなど）、変換は続ける。
                    AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
                }
                job
            };
            let tree = Self { job };
            if let Err(e) = resume_threads(child.id()) {
                // 止まったまま残さない。
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
            Ok(tree)
        }

        #[cfg(test)]
        pub fn contains(&self, child: &Child) -> bool {
            use windows_sys::Win32::System::JobObjects::IsProcessInJob;
            let mut result = 0;
            // SAFETY: どちらも有効なハンドル。
            unsafe { IsProcessInJob(child.as_raw_handle() as HANDLE, self.job, &mut result) };
            result != 0
        }

        pub fn terminate(&self) {
            if !self.job.is_null() {
                // SAFETY: 自分で作った有効な Job Object のハンドル。
                unsafe {
                    TerminateJobObject(self.job, 1);
                }
            }
        }

        pub fn kill(&self) {
            self.terminate();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            if !self.job.is_null() {
                // SAFETY: 自分で作った有効なハンドルを 1 回だけ閉じる。
                unsafe {
                    CloseHandle(self.job);
                }
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    pub fn registry_string(_subkey: &str, _value: &str) -> Option<String> {
        None
    }

    pub fn process_exists(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: シグナル 0 は送らずに、存在と権限だけを調べる。
        let r = unsafe { libc::kill(pid, 0) };
        // ESRCH ならない。EPERM（ほかのユーザーのプロセス）などは、あるものとして扱う。
        r == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }

    pub fn configure(cmd: &mut Command, priority: u8) -> Option<String> {
        let wanted: libc::c_int = match priority {
            0 => 19,
            1 => 0,
            2 => -5,
            _ => -10,
        };
        // 一般ユーザーは nice を下げられない（優先度を上げられない）ので 0 に丸める。
        // SAFETY: geteuid は常に成功する。
        let is_root = unsafe { libc::geteuid() } == 0;
        let (nice, warning) = if wanted < 0 && !is_root {
            (
                0,
                Some(format!(
                    "優先度を上げる権限がないため、nice {wanted} ではなく 0 で実行します"
                )),
            )
        } else {
            (wanted, None)
        };
        cmd.process_group(0);
        #[cfg(target_os = "linux")]
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: fork 後・exec 前に呼ばれる。setpriority・prctl・getppid は async-signal-safe。
        unsafe {
            cmd.pre_exec(move || {
                if nice != 0 {
                    libc::setpriority(libc::PRIO_PROCESS as _, 0, nice);
                }
                // 親（SmartConverter）が SIGKILL などで突然終わったときも、子が残って動き続けないようにする。
                // PDEATHSIG は「子を起動したスレッド」が終わったときに届くが、子はジョブのスレッドから起動し、
                // そのスレッドは子が終わるまで待っているので問題ない。孫（sh -c の先など）には届かない。
                #[cfg(target_os = "linux")]
                {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    // 設定する前に親がもう終わっていたら、ここで終わる
                    // （fork 後はメモリを確保しないよう、OS のエラー番号だけで返す）
                    if libc::getppid() != parent {
                        return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                    }
                }
                Ok(())
            });
        }
        warning
    }

    /// プロセスグループ（グループ ID ＝ 子プロセスの PID）。
    pub struct Tree {
        pgid: libc::pid_t,
    }

    impl Tree {
        pub fn attach(child: &mut Child) -> std::io::Result<Self> {
            Ok(Self {
                pgid: child.id() as libc::pid_t,
            })
        }

        pub fn terminate(&self) {
            // SAFETY: 自分が作ったプロセスグループへのシグナル送信。
            unsafe {
                libc::kill(-self.pgid, libc::SIGTERM);
            }
        }

        pub fn kill(&self) {
            // SAFETY: 同上。
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_existence() {
        assert!(process_exists(std::process::id()));
        // ありえない大きな ID
        assert!(!process_exists(4_000_000_000));
        // 起動して終わった子プロセスの ID
        let mut child = if cfg!(windows) {
            Command::new("cmd").args(["/c", "exit"]).spawn().unwrap()
        } else {
            Command::new("true").spawn().unwrap()
        };
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_exists(pid));
    }

    /// configure で止めた状態で起動し、attach で Job に入れてから動かす（Windows）。
    #[cfg(windows)]
    #[test]
    fn child_runs_inside_job() {
        use std::io::Read;
        use std::process::Stdio;
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "echo started"]).stdout(Stdio::piped());
        configure(&mut cmd, 1);
        let mut child = cmd.spawn().unwrap();
        let tree = ProcessTree::attach(&mut child).unwrap();
        assert!(tree.inner.contains(&child));
        let mut out = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(out.trim(), "started");
    }

    /// 孫プロセスも Job に入っていて、Job を閉じると一緒に終わる（Windows）。
    #[cfg(windows)]
    #[test]
    fn grandchild_ends_with_job() {
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        // 子（cmd）が孫（ping）を起動してすぐ終わる。孫の PID を知るため、PowerShell で起動して出力させる。
        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-Command",
            "$p = Start-Process -PassThru -WindowStyle Hidden ping -ArgumentList '-n','30','127.0.0.1'; $p.Id",
        ])
        .stdout(Stdio::piped());
        configure(&mut cmd, 1);
        let mut child = cmd.spawn().unwrap();
        let tree = ProcessTree::attach(&mut child).unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        child.wait().unwrap();
        let grandchild: u32 = line.trim().parse().unwrap();
        assert!(process_exists(grandchild));
        drop(tree);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while process_exists(grandchild) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(!process_exists(grandchild));
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;

    #[test]
    fn reads_registry_strings() {
        let v = registry_string(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ProductName",
        );
        if cfg!(windows) {
            assert!(v.is_some_and(|s| s.contains("Windows")));
        } else {
            assert!(v.is_none());
        }
        assert!(registry_string(r"SOFTWARE\SmartConverterNoSuchKey", "x").is_none());
    }
}
