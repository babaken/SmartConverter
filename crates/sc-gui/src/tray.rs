//! トレイアイコン（画面設計 T-01、基本設計 §9.5）。
//!
//! - Windows：`tray-icon`
//! - Linux：`ksni`（StatusNotifierItem を D-Bus で直接扱う。GTK は要らない）。cargo の feature `tray`（既定で有効）。
//!   KDE Plasma は標準で対応している。GNOME は AppIndicator 拡張が必要。D-Bus のセッションが無いなどで
//!   作れなければ、トレイなしで動く（最小化してもタスクバーに残る）。

// トレイを使えないビルドでは、操作の種類などが使われない。
#![cfg_attr(
    not(any(windows, all(target_os = "linux", feature = "tray"))),
    allow(dead_code)
)]

use std::sync::mpsc::Receiver;

use eframe::egui;

/// この環境でトレイを使えるか。
pub const SUPPORTED: bool = cfg!(any(windows, all(target_os = "linux", feature = "tray")));

/// トレイからの操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    ShowLog,
    Abort,
    Options,
    Quit,
}

pub struct Tray {
    rx: Receiver<TrayAction>,
    #[cfg(any(windows, all(target_os = "linux", feature = "tray")))]
    inner: imp::Inner,
}

impl Tray {
    /// トレイアイコンを作る。作れなければ `None`。
    pub fn new(ctx: &egui::Context, icon_png: &[u8]) -> Option<Self> {
        #[cfg(any(windows, all(target_os = "linux", feature = "tray")))]
        {
            let (inner, rx) = imp::Inner::new(ctx, icon_png)?;
            Some(Self { rx, inner })
        }
        #[cfg(not(any(windows, all(target_os = "linux", feature = "tray"))))]
        {
            let _ = (ctx, icon_png);
            None
        }
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        self.rx.try_iter().collect()
    }

    pub fn set_tooltip(&self, text: &str) {
        #[cfg(any(windows, all(target_os = "linux", feature = "tray")))]
        self.inner.set_tooltip(text);
        #[cfg(not(any(windows, all(target_os = "linux", feature = "tray"))))]
        let _ = text;
    }

    pub fn set_abort_enabled(&self, enabled: bool) {
        #[cfg(any(windows, all(target_os = "linux", feature = "tray")))]
        self.inner.set_abort_enabled(enabled);
        #[cfg(not(any(windows, all(target_os = "linux", feature = "tray"))))]
        let _ = enabled;
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::mpsc::{self, Receiver};

    use eframe::egui;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

    use super::TrayAction;

    pub struct Inner {
        icon: TrayIcon,
        abort: MenuItem,
    }

    impl Inner {
        pub fn new(ctx: &egui::Context, icon_png: &[u8]) -> Option<(Self, Receiver<TrayAction>)> {
            let data = eframe::icon_data::from_png_bytes(icon_png).ok()?;
            let icon = Icon::from_rgba(data.rgba, data.width, data.height).ok()?;

            let show = MenuItem::new("メインウィンドウを表示", true, None);
            let log = MenuItem::new("ログを表示", true, None);
            let abort = MenuItem::new("処理中を中断", false, None);
            let options = MenuItem::new("オプション", true, None);
            let quit = MenuItem::new("終了", true, None);
            let menu = Menu::new();
            menu.append_items(&[
                &show,
                &log,
                &PredefinedMenuItem::separator(),
                &abort,
                &options,
                &PredefinedMenuItem::separator(),
                &quit,
            ])
            .ok()?;

            let tray = TrayIconBuilder::new()
                .with_icon(icon)
                .with_tooltip("SmartConverter - 待機中")
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .build()
                .ok()?;

            let (tx, rx) = mpsc::channel();
            let ids = [
                (show.id().clone(), TrayAction::ShowWindow),
                (log.id().clone(), TrayAction::ShowLog),
                (abort.id().clone(), TrayAction::Abort),
                (options.id().clone(), TrayAction::Options),
                (quit.id().clone(), TrayAction::Quit),
            ];
            {
                let tx = tx.clone();
                let ctx = ctx.clone();
                MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                    if let Some((_, a)) = ids.iter().find(|(id, _)| *id == e.id) {
                        let _ = tx.send(*a);
                        ctx.request_repaint();
                    }
                }));
            }
            {
                let ctx = ctx.clone();
                TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                    let show = matches!(
                        e,
                        TrayIconEvent::DoubleClick { .. }
                            | TrayIconEvent::Click {
                                button: MouseButton::Left,
                                button_state: MouseButtonState::Up,
                                ..
                            }
                    );
                    if show {
                        let _ = tx.send(TrayAction::ShowWindow);
                        ctx.request_repaint();
                    }
                }));
            }
            Some((Self { icon: tray, abort }, rx))
        }

        pub fn set_tooltip(&self, text: &str) {
            let _ = self.icon.set_tooltip(Some(text));
        }

        pub fn set_abort_enabled(&self, enabled: bool) {
            self.abort.set_enabled(enabled);
        }
    }
}

#[cfg(all(target_os = "linux", feature = "tray"))]
mod imp {
    use std::cell::RefCell;
    use std::sync::mpsc::{self, Receiver, Sender};

    use eframe::egui;
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::{MenuItem, StandardItem};

    use super::TrayAction;

    /// D-Bus 側のスレッドが持つトレイの状態。
    struct SniTray {
        tx: Sender<TrayAction>,
        ctx: egui::Context,
        icon: Vec<ksni::Icon>,
        tooltip: String,
        abort_enabled: bool,
    }

    impl SniTray {
        fn send(&self, a: TrayAction) {
            let _ = self.tx.send(a);
            self.ctx.request_repaint();
        }
    }

    impl ksni::Tray for SniTray {
        fn id(&self) -> String {
            "smartconverter".into()
        }

        fn title(&self) -> String {
            sc_core::APP_NAME.into()
        }

        // 左クリックでメインウィンドウを出す
        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(TrayAction::ShowWindow);
        }

        fn icon_name(&self) -> String {
            // .desktop と一緒にアイコンが入っていればテーマのアイコンを、無ければ icon_pixmap を使う
            "smartconverter".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.clone()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: sc_core::APP_NAME.into(),
                description: self.tooltip.clone(),
                ..Default::default()
            }
        }

        fn menu(&self) -> Vec<MenuItem<Self>> {
            let item = |label: &str, enabled: bool, action: TrayAction| -> MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    enabled,
                    activate: Box::new(move |t: &mut Self| t.send(action)),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item("メインウィンドウを表示", true, TrayAction::ShowWindow),
                item("ログを表示", true, TrayAction::ShowLog),
                MenuItem::Separator,
                item("処理中を中断", self.abort_enabled, TrayAction::Abort),
                item("オプション", true, TrayAction::Options),
                MenuItem::Separator,
                item("終了", true, TrayAction::Quit),
            ]
        }
    }

    pub struct Inner {
        handle: Handle<SniTray>,
        /// 同じ内容を何度も送らないよう、最後に送った値（ツールチップ、中断の可否）を覚えておく
        last: RefCell<(String, bool)>,
    }

    impl Inner {
        pub fn new(ctx: &egui::Context, icon_png: &[u8]) -> Option<(Self, Receiver<TrayAction>)> {
            let data = eframe::icon_data::from_png_bytes(icon_png).ok()?;
            // StatusNotifierItem の画像は ARGB32（ネットワークバイトオーダー）
            let argb = data
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            let icon = vec![ksni::Icon {
                width: data.width as i32,
                height: data.height as i32,
                data: argb,
            }];
            let (tx, rx) = mpsc::channel();
            let tooltip = String::from("待機中");
            let tray = SniTray {
                tx,
                ctx: ctx.clone(),
                icon,
                tooltip: tooltip.clone(),
                abort_enabled: false,
            };
            let handle = tray.spawn().ok()?;
            Some((
                Self {
                    handle,
                    last: RefCell::new((tooltip, false)),
                },
                rx,
            ))
        }

        pub fn set_tooltip(&self, text: &str) {
            // タイトルにアプリ名が出るので、本文は「SmartConverter - 」を除いた部分にする
            let prefix = format!("{} - ", sc_core::APP_NAME);
            let text = text
                .strip_prefix(prefix.as_str())
                .unwrap_or(text)
                .to_owned();
            let mut last = self.last.borrow_mut();
            if last.0 != text {
                last.0.clone_from(&text);
                self.handle.update(move |t| t.tooltip = text);
            }
        }

        pub fn set_abort_enabled(&self, enabled: bool) {
            let mut last = self.last.borrow_mut();
            if last.1 != enabled {
                last.1 = enabled;
                self.handle.update(move |t| t.abort_enabled = enabled);
            }
        }
    }
}
