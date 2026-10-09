//! MidpointTable_H（中間点の表）
//!
//! フォーカス中のオブジェクトについて、行を点（開始・中間点・終了）、列をトラックバー項目にした表を出し、
//! 書き換えた値を「反映」でまとめて書き込む。仕様は `AI/specifications/20261008_MidpointTable_H_spec.md`、
//! 根拠にした実機の結果は `AI/docs/20261008_パラメータ編集_SDK実機確認.md`。
//!
//! 自分で起こすスレッドは無い。監視は egui の更新の中で読むだけで、エイリアスを読むのは本体のイベント
//! （オブジェクトの更新・フォーカスの変更・シーンの変更）が来たときだけ。ウィンドウが見えていない間は何も読まない。

pub mod alias;
mod edit_ops;
mod gui;
pub mod movement;
mod prefs;
pub mod table;
pub mod track;

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;

use aviutl2::generic::{EventType, GlobalEditHandle};
use aviutl2::raw_window_handle::HasWindowHandle;
use aviutl2::AnyResult;
use aviutl2_eframe::egui;
use parking_lot::Mutex;

pub const WINDOW_NAME: &str = "中間点の表";

pub(crate) static EDIT_HANDLE: GlobalEditHandle = GlobalEditHandle::new();

/// ウィンドウの HWND（メニューから前に出すため）
static WINDOW_HWND: AtomicIsize = AtomicIsize::new(0);

/// 本体のイベントで立てる印。イベント用スレッドで立て、UI のスレッドで下ろす（ここから本体を読まない）
pub(crate) static OBJECT_DIRTY: AtomicBool = AtomicBool::new(true);

/// UI のスレッドを起こすための egui の Context（ウィンドウができたら入る）
pub(crate) static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// 本体のイベント: 印を立てて UI を起こす。立っている間は起こし直さない（ドラッグ中の連続したイベントで描き直しを重ねない）
fn on_host_event() {
    if !OBJECT_DIRTY.swap(true, Ordering::AcqRel) {
        if let Some(ctx) = EGUI_CTX.get() {
            ctx.request_repaint();
        }
    }
}

/// ウィンドウが見えているか（本体のパネルに出ていなければ偽）。ハンドルが取れていなければ見えているとみなす
pub(crate) fn window_visible() -> bool {
    let hwnd = WINDOW_HWND.load(Ordering::Acquire);
    if hwnd == 0 {
        return true;
    }
    unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(windows::Win32::Foundation::HWND(hwnd as _)).as_bool() }
}

/// 中間点の表のウィンドウを表示して前に出す（ColorHistory_H の `open_window` と同じ）
fn open_window() {
    let hwnd = WINDOW_HWND.load(Ordering::Acquire);
    if hwnd == 0 {
        tracing::warn!("[MidpointTable_H] ウィンドウがまだありません");
        return;
    }
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW};
        let hwnd = HWND(hwnd as _);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
}

/// タイムラインの右クリック（オブジェクト）から: 選んだオブジェクトの今のフレームに中間点を足す。
/// メニューを選んだときだけ書く（ルール au2-rs-plugin）
fn add_midpoint_from_menu() {
    let result = EDIT_HANDLE.call_edit_section(|edit| -> Result<String, String> {
        let handle = match edit.get_selected_objects() {
            Ok(v) if !v.is_empty() => v[0],
            _ => edit.get_focused_object().ok().flatten().ok_or("オブジェクトが選ばれていません")?,
        };
        let frame = edit.info.frame;
        let lf = edit.get_object_layer_frame(handle).map_err(|e| e.to_string())?;
        if frame <= lf.start || frame >= lf.end {
            return Err(format!("今のフレーム {frame} はオブジェクトの中（{}〜{}）ではありません", lf.start + 1, lf.end - 1));
        }
        edit.create_object_section(handle, frame).map_err(|e| format!("フレーム {frame} に中間点を足せませんでした（既にある位置かもしれません）: {e}"))?;
        Ok(format!("フレーム {frame} に中間点を足しました"))
    });
    match result {
        Ok(Ok(msg)) => tracing::info!("[MidpointTable_H] {msg}"),
        Ok(Err(e)) => tracing::warn!("[MidpointTable_H] {e}"),
        Err(e) => tracing::error!("[MidpointTable_H] 書き込みを始められませんでした: {e}"),
    }
}

fn init_logging() {
    use aviutl2::tracing::Level;
    let level = if cfg!(debug_assertions) { Level::DEBUG } else { Level::INFO };
    let _ = aviutl2::tracing_subscriber::fmt()
        .with_max_level(level)
        .event_format(aviutl2::logger::AviUtl2Formatter)
        .with_writer(aviutl2::logger::AviUtl2LogWriter)
        .try_init();
}

#[aviutl2::plugin(GenericPlugin)]
pub struct MidpointTablePlugin {
    window: Mutex<Option<aviutl2_eframe::EframeWindow>>,
    /// 読み込まれた時刻。これより新しい .tra2 は本体に登録されていない（`movement.rs`）
    loaded_at: std::time::SystemTime,
}

/// 本体に登録されている移動方法の一覧を作る（起動時に 1 回）
fn scan_movements(loaded_at: std::time::SystemTime) -> movement::Scan {
    let script_dir = aviutl2::config::app_data_path().join("Script");
    let bundled = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("script.tra2")));
    let s = movement::scan(&script_dir, bundled.as_deref(), Some(loaded_at));
    tracing::info!("移動方法 {} 件（外したもの {} 件）", s.movements.len(), s.skipped.len());
    s
}

impl aviutl2::generic::GenericPlugin for MidpointTablePlugin {
    fn new(_info: aviutl2::AviUtl2Info) -> AnyResult<Self> {
        init_logging();
        tracing::info!("MidpointTable_H v{} 初期化", env!("CARGO_PKG_VERSION"));
        Ok(Self { window: Mutex::new(None), loaded_at: std::time::SystemTime::now() })
    }

    fn plugin_info(&self) -> aviutl2::generic::GenericPluginTable {
        aviutl2::generic::GenericPluginTable {
            name: "MidpointTable_H".to_string(),
            information: format!("MidpointTable_H v{} - 中間点の表 / by HexBrowns", env!("CARGO_PKG_VERSION")),
        }
    }

    fn register(&mut self, registry: &mut aviutl2::generic::HostAppHandle) {
        EDIT_HANDLE.init(registry.create_edit_handle());
        let movements = std::sync::Arc::new(scan_movements(self.loaded_at));
        let window = match aviutl2_eframe::EframeWindow::new(WINDOW_NAME, move |cc, handle| {
            Ok(Box::new(gui::MidpointTableApp::new(cc, handle, movements)))
        }) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("中間点の表のウィンドウを作れませんでした: {e:#}");
                return;
            }
        };
        match window.handle() {
            Ok(handle) => {
                if let Err(e) = registry.register_window_client(WINDOW_NAME, &handle) {
                    tracing::error!("register_window_client 失敗: {e}");
                }
                if let Ok(raw) = handle.window_handle() {
                    if let aviutl2::raw_window_handle::RawWindowHandle::Win32(h) = raw.as_raw() {
                        WINDOW_HWND.store(h.hwnd.get(), Ordering::Release);
                    }
                }
            }
            Err(e) => tracing::error!("中間点の表のウィンドウのハンドルを取れませんでした: {e:#}"),
        }
        *self.window.lock() = Some(window);

        // タイムラインの右クリック（オブジェクト・レイヤー）と編集メニュー（ショートカットを割り当てられる）
        registry.register_object_menu("中間点の表\\表を開く", open_window);
        registry.register_object_menu("中間点の表\\今のフレームに中間点を足す", add_midpoint_from_menu);
        registry.register_layer_menu("中間点の表を開く", open_window);
        registry.register_edit_menu("中間点の表を開く", open_window);
        registry.register_edit_menu("中間点の表: 今のフレームに中間点を足す", add_midpoint_from_menu);

        // 表を読み直す合図（フレームの移動は使わない。再生中に毎フレーム来るので、区間の印は 1 秒ごとの軽い読み取りで追う）
        for ev in [EventType::UpdateObject, EventType::ChangeFocusObject, EventType::ChangeEditScene] {
            registry.register_event_listener(ev, on_host_event);
        }
    }
}

// 本体が呼ぶ関数（GetCommonPluginTable など）を書き出す（ルール au2-rs-plugin「ビルド」）
aviutl2::register_generic_plugin!(MidpointTablePlugin);
