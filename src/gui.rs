//! 中間点の表のウィンドウ（egui）
//!
//! 監視は読み取りだけ。書き込みは「反映」ボタンからだけ行う。セルのドラッグも未反映の書き換えになるだけ。
//!
//! 監視の重さを抑える（v0.5.0。`issues/20261010_heavy_with_many_midpoints_and_polls_while_hidden.md`）:
//! - ウィンドウが見えていない間は何も読まず、何も描かない（eframe は SW_HIDE のウィンドウでも `ui` を呼ぶ）
//! - 表のオブジェクトのエイリアスは、本体のイベント（`lib.rs` の `OBJECT_DIRTY`）が来たときだけ読む（0.4 秒に 1 回まで）。
//!   フォーカスと区間の印は、エイリアスを読まない軽い読み取りで 1 秒ごとに追う
//! - 「自動で読み直す」を切ると、「再読み込み」を押したときだけ読む
//! - 表は見えている行（と前後の余白・選択・編集中の行）だけを作る
//!
//! キー操作（Ctrl+C / Ctrl+V / 矢印 / Enter / Esc）は、表のセルにフォーカスがあるときだけ受ける。
//! aviutl2-eframe は egui のどこにもフォーカスが無いときだけキーを本体へ渡す（`egui_wants_keyboard_input`）ので、
//! セルをクリックしたらそのセルにフォーカスを置き、本体のオブジェクトのコピー・貼り付けとして働かないようにする。
//! Esc でフォーカスを外すと、キーは本体へ戻る。
//!
//! 周期複製と設定のウィンドウは、パネルの高さに収まるように作る（収まらない分は中をスクロール）。
//! どこにもフォーカスが無いときの Esc で閉じる（パネルが低いと × が見えなくなっていた。v0.5.3）。
//! このときの Esc は aviutl2-eframe が本体にも渡す（本体の Esc に割り当ては無い）。
//! egui はフレームの始めに Esc でフォーカスを外すので、「フォーカスが無かったか」は前のフレームの終わりに覚えた値で見る。
//! 数値の入力欄は、確定（Enter・ほかをクリック）したときだけ値にし、入力中の Esc では入力前の値に戻す

use std::time::{Duration, Instant};

use aviutl2::generic::ObjectHandle;
use aviutl2_eframe::{eframe, egui, AviUtl2EframeHandle};

use std::sync::Arc;

use std::sync::atomic::Ordering;

use crate::edit_ops::{self, Bound, Loaded, Outcome, Structure};
use crate::movement::{self, Scan};
use crate::prefs::Prefs;
use crate::table::{Column, ModeChange, Selection};
use crate::OBJECT_DIRTY;

/// 本体のイベントで読み直す間隔の下限
const POLL_INTERVAL: Duration = Duration::from_millis(400);
/// フォーカスと区間の印を追う軽い読み取りの間隔
const LIGHT_INTERVAL: Duration = Duration::from_millis(1000);
/// 見えていない間に、また見えるようになったかを確かめる間隔
const HIDDEN_CHECK: Duration = Duration::from_millis(500);
/// これより長く見えていたウィンドウが隠れたら、ログに出す（起動直後の一瞬を数えない）
const SHOWN_LONG: Duration = Duration::from_secs(2);
/// 表の 1 行の高さ（見えていない行はこの高さの空きにする）
const ROW_H: f32 = 18.0;
/// 見えている範囲の上下に余分に作る高さ
const ROW_MARGIN: f32 = 200.0;
/// 表の読み込みがこれより長くかかったら、1 回だけログに出す
const SLOW_LOAD: Duration = Duration::from_millis(100);
const CELL_W: f32 = 76.0;
const YELLOW: egui::Color32 = egui::Color32::from_rgb(235, 200, 90);
const RED: egui::Color32 = egui::Color32::from_rgb(235, 110, 110);
/// ドラッグ 1 ピクセルあたりの変化量。Shift で 1/10、Ctrl で 10 倍
const DRAG_STEP: f64 = 1.0;

/// セルのドラッグ中の状態
struct Drag {
    /// 掴んだセル（見えている範囲の外へスクロールしても、その行は作り続ける）
    origin: (usize, usize),
    targets: Vec<((usize, usize), f64)>,
    offset: f64,
}

pub struct MidpointTableApp {
    _handle: AviUtl2EframeHandle,
    loaded: Option<Loaded>,
    focused: Option<ObjectHandle>,
    focus_section: Option<usize>,
    last_poll: Option<Instant>,
    /// 未反映があるうちに本体側で表のオブジェクトが変わった
    host_changed: bool,
    /// 未反映があるうちにフォーカスが別のオブジェクトへ移った
    focus_moved: bool,
    bound_missing: bool,
    editing: Option<((usize, usize), String, bool)>,
    add_text: String,
    mul_text: String,
    status: String,
    status_is_error: bool,
    last_poll_error: Option<String>,
    drag: Option<Drag>,
    /// キー操作を受けるためにフォーカスを置いたセルの Id
    key_focus: Option<egui::Id>,
    /// そのセルの行（作らないとフォーカスが外れるので、見えていなくても作る）
    key_focus_row: Option<usize>,
    /// ウィンドウが見え始めた時刻（見えていなければ None）
    visible_since: Option<Instant>,    /// 読み込みが遅かったことを 1 回だけログに出した
    slow_load_logged: bool,
    /// 本体に登録されている移動方法（起動時に 1 回作る。`movement.rs`）
    movements: Arc<Scan>,
    mode_filter: String,
    /// 設定のウィンドウを開いている列
    settings_col: Option<usize>,
    /// 隠す列（`prefs.rs`）
    prefs: Prefs,
    /// 隠す・戻すの要求（表を作り直すので描画の後で行う）
    hide_request: Option<(String, String, bool)>,
    /// 列の並び順の要求（効果名、新しい順。None は元の並びに戻す）
    order_request: Option<(String, Option<Vec<String>>)>,
    /// 周期複製のウィンドウ
    dup_open: bool,
    /// 周期複製のウィンドウの「ずらす量」の一覧以外の高さ（前のフレームで測る。パネルに収めるのに使う）
    dup_fixed_h: f32,
    /// 前のフレームの終わりに、どこかにフォーカスがあったか・ポップアップが開いていたか（Esc で閉じる判定に使う）
    busy_last_frame: bool,
    dup_interval: usize,
    dup_count: usize,
    dup_below: bool,
    /// 列ごとの「1 つごとにずらす量」（列の添字。表を作り直したら捨てる）
    dup_deltas: Vec<f64>,
    /// 見出しをドラッグしている列（並べ替え）
    col_drag: Option<usize>,
    /// 中間点の位置を入力している行
    frame_editing: Option<(usize, String, bool)>,
    /// 中間点を足す・動かす・消す要求（すぐに本体へ書く。描画の後で行う）
    struct_request: Option<Structure>,
}

impl MidpointTableApp {
    pub fn new(cc: &eframe::CreationContext<'_>, handle: AviUtl2EframeHandle, movements: Arc<Scan>) -> Self {
        cc.egui_ctx.all_styles_mut(|style| {
            style.visuals = aviutl2_eframe::aviutl2_visuals();
        });
        cc.egui_ctx.set_fonts(aviutl2_eframe::aviutl2_fonts());
        // 本体のイベントから UI を起こせるようにする
        let _ = crate::EGUI_CTX.set(cc.egui_ctx.clone());
        Self {
            _handle: handle,
            loaded: None,
            focused: None,
            focus_section: None,
            last_poll: None,
            host_changed: false,
            focus_moved: false,
            bound_missing: false,
            editing: None,
            add_text: "10".into(),
            mul_text: "1.5".into(),
            status: String::new(),
            status_is_error: false,
            last_poll_error: None,
            drag: None,
            key_focus: None,
            key_focus_row: None,
            visible_since: None,
            slow_load_logged: false,
            movements,
            mode_filter: String::new(),
            settings_col: None,
            prefs: Prefs::load(&aviutl2::config::app_data_path().join("Plugin").join("MidpointTable_H").join("hidden_columns.txt")),
            hide_request: None,
            order_request: None,
            dup_open: false,
            dup_fixed_h: 300.0,
            busy_last_frame: false,
            dup_interval: 0,
            dup_count: 3,
            dup_below: false,
            dup_deltas: Vec::new(),
            col_drag: None,
            frame_editing: None,
            struct_request: None,
        }
    }

    fn set_status(&mut self, s: impl Into<String>, error: bool) {
        self.status = s.into();
        self.status_is_error = error;
    }

    fn pending(&self) -> usize {
        self.loaded.as_ref().map(|l| l.table.pending_count()).unwrap_or(0)
    }

    fn load(&mut self, handle: ObjectHandle) {
        let selection = self.loaded.as_ref().filter(|l| l.handle == handle).and_then(|l| l.table.selection);
        let started = Instant::now();
        let result = edit_ops::load(handle, self.prefs.clone());
        let took = started.elapsed();
        if let Ok(l) = &result {
            // 重さの目安（実機での切り分け用。毎回は出さない）
            if took >= SLOW_LOAD && !self.slow_load_logged {
                self.slow_load_logged = true;
                tracing::info!(
                    "[MidpointTable_H] 表の読み込みに {} ms かかりました（点 {}・列 {}）",
                    took.as_millis(),
                    l.table.points(),
                    l.table.columns.len()
                );
            }
        }
        match result {
            Ok(mut l) => {
                // 同じオブジェクトを読み直したときは選択を残す（表の大きさに収まる範囲で）
                if let Some(sel) = selection {
                    let (rows, cols) = sel.rect();
                    if *rows.end() < l.table.points() && *cols.end() < l.table.columns.len() {
                        l.table.selection = Some(sel);
                    }
                }
                self.loaded = Some(l);
                self.host_changed = false;
                self.focus_moved = false;
                self.bound_missing = false;
                self.editing = None;
            }
            Err(e) => {
                self.loaded = None;
                self.set_status(e, true);
            }
        }
    }

    /// 読み直しが要るか決めて読む。本体のイベントが来ていればエイリアスまで、来ていなければ 1 秒ごとにフォーカスだけ
    fn poll_if_due(&mut self, ctx: &egui::Context) {
        let since = self.last_poll.map(|t| t.elapsed());
        let dirty = OBJECT_DIRTY.load(Ordering::Acquire);
        if !dirty && since.is_some_and(|d| d < LIGHT_INTERVAL) {
            return;
        }
        // イベントが続けて来ても 0.4 秒に 1 回まで（本体のトラックバーをドラッグしている間など）
        if let Some(d) = since.filter(|d| *d < POLL_INTERVAL) {
            ctx.request_repaint_after(POLL_INTERVAL - d);
            return;
        }
        let full = OBJECT_DIRTY.swap(false, Ordering::AcqRel);
        self.poll(full);
    }

    /// フォーカスだけを読む（自動で読み直さない設定の「再読み込み」）。読めたら真
    fn refresh_focus(&mut self) -> bool {
        match edit_ops::poll(None, false) {
            Ok(Some(p)) => {
                self.focused = p.focused;
                self.focus_section = p.focus_section;
                true
            }
            Ok(None) => false,
            Err(e) => {
                self.set_status(e, true);
                false
            }
        }
    }

    fn poll(&mut self, full: bool) {
        self.last_poll = Some(Instant::now());
        // 消えたオブジェクトのハンドルは渡さない（渡すたびに本体が invalid object handle の WARN を出す）
        let bound = self.loaded.as_ref().filter(|_| !self.bound_missing).map(|l| l.handle);
        let p = match edit_ops::poll(bound, full) {
            Ok(None) => return,
            Ok(Some(p)) => {
                self.last_poll_error = None;
                p
            }
            Err(e) => {
                // 同じ失敗を毎回ログに出さない（ルール au2-workflow「ログと print」）
                if self.last_poll_error.as_ref() != Some(&e) {
                    tracing::warn!("[MidpointTable_H] {e}");
                    self.last_poll_error = Some(e);
                }
                return;
            }
        };
        self.focused = p.focused;
        self.focus_section = p.focus_section;
        let pending = self.pending() > 0;

        if let Some(l) = self.loaded.as_ref().filter(|_| !self.bound_missing) {
            let handle = l.handle;
            match &p.bound {
                Bound::Missing => {
                    if pending {
                        self.bound_missing = true;
                    } else {
                        self.loaded = None;
                    }
                }
                Bound::Alias(a) if *a != l.alias_raw => {
                    if pending {
                        self.host_changed = true;
                    } else {
                        self.load(handle);
                    }
                }
                _ => {}
            }
        }

        let bound = self.loaded.as_ref().map(|l| l.handle);
        if let Some(f) = p.focused {
            if Some(f) != bound {
                if self.pending() > 0 {
                    self.focus_moved = true;
                } else {
                    self.load(f);
                }
            } else {
                self.focus_moved = false;
            }
        }
    }

    fn apply(&mut self) {
        let Some(l) = &self.loaded else { return };
        let handle = l.handle;
        let jobs = l.table.write_jobs();
        if jobs.is_empty() {
            return;
        }
        let n = jobs.len();
        match edit_ops::apply(handle, jobs) {
            Ok(results) => {
                let mut msgs = Vec::new();
                let mut written = 0;
                for (job, o) in &results {
                    match o {
                        Outcome::Written => written += 1,
                        Outcome::Stale(_) => msgs.push(format!("{}: 本体側で変わっていたので書きませんでした", job.label())),
                        Outcome::Mismatch(back) => msgs.push(format!(
                            "{}: 本体が値を変えて保存しました（範囲の外の値は範囲に収められます）。保存された値: {back}",
                            job.label()
                        )),
                        Outcome::Failed(e) => msgs.push(format!("{}: {e}", job.label())),
                    }
                }
                let head = format!("{written} / {n} 列を反映しました");
                let error = !msgs.is_empty();
                self.set_status(if error { format!("{head}。{}", msgs.join(" / ")) } else { head }, error);
                self.load(handle);
            }
            Err(e) => self.set_status(e, true),
        }
    }

    fn render_top(&mut self, ui: &mut egui::Ui) {
        let pending = self.pending();
        ui.horizontal(|ui| {
            match &self.loaded {
                Some(l) => ui.label(format!("対象: {}", l.label)),
                None => ui.weak("タイムラインでオブジェクトを選んでください"),
            };
            ui.label("(?)").on_hover_text(
                "セルをダブルクリック（または Enter / F2）で数値を入力\n\
                 セルを横にドラッグで値を増減（Shift で 1/10、Ctrl で 10 倍）。選んだ範囲の中を掴むと範囲全部が動く\n\
                 Shift+クリック・Shift+矢印で範囲を選ぶ\n\
                 セルを選んでいる間は Ctrl+C / Ctrl+V / 矢印 が表に効き、本体には渡りません。Esc で本体に戻します\n\
                 どこも選んでいないときの Esc で、周期複製・設定のウィンドウを閉じます\n\
                 書き換えは「反映」を押すまで本体に書きません",
            );
            if self.key_focus.is_some() {
                ui.colored_label(YELLOW, "キー操作: 表（Esc で本体へ）");
            }
            // 隠している列（このオブジェクトにあるもの）を戻す
            let hidden_here = self.loaded.as_ref().map(|l| l.hidden_here.clone()).unwrap_or_default();
            if !hidden_here.is_empty() {
                let mut req = None;
                ui.add_enabled_ui(pending == 0, |ui| {
                    ui.menu_button(format!("隠した列 {}", hidden_here.len()), |ui| {
                        for (e, i) in &hidden_here {
                            if ui.button(format!("{e} / {i} を表示する")).clicked() {
                                req = Some((e.clone(), i.clone(), false));
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text("列の見出しを右クリックすると隠せます。隠した列はどのオブジェクトでも隠します")
                    .on_disabled_hover_text("未反映の書き換えがある間は切り替えられません");
                });
                if req.is_some() {
                    self.hide_request = req;
                }
            }
            if self.loaded.is_some() {
                let b = ui.add_enabled(pending == 0, egui::Button::new("今のフレームに中間点を足す"));
                let b = b
                    .on_hover_text("本体で表示しているフレームに中間点を足します（すぐに本体へ書きます）")
                    .on_disabled_hover_text("未反映の書き換えがある間は、中間点を足せません");
                if b.clicked() {
                    match edit_ops::current_frame() {
                        Some(frame) => self.struct_request = Some(Structure::Add { frame }),
                        None => self.set_status("本体の準備ができていません", true),
                    }
                }
                if ui.add(egui::Button::selectable(self.dup_open, "周期複製…")).on_hover_text("このオブジェクトを一定の間隔で複製し、値を 1 つごとにずらします").clicked() {
                    self.dup_open = !self.dup_open;
                    if self.dup_open && self.dup_interval == 0 {
                        // 既定の間隔はオブジェクトの長さ（同じレイヤーにすき間なく並ぶ）
                        if let Some(l) = &self.loaded {
                            let f = &l.table.frames;
                            self.dup_interval = f.last().copied().unwrap_or(0) - f.first().copied().unwrap_or(0) + 1;
                        }
                    }
                }
            }
            let reload = if pending > 0 { "再読み込み（未反映を捨てる）" } else { "再読み込み" };
            let tip = if self.prefs.auto_reload { "表を読み直します" } else { "本体で選んでいるオブジェクトの表を読み直します" };
            if ui.button(reload).on_hover_text(tip).clicked() {
                // 自動で読み直さない設定では、フォーカスもここで初めて読む
                let follow = self.focus_moved || (!self.prefs.auto_reload && self.refresh_focus());
                let loaded = self.loaded.as_ref().map(|l| l.handle);
                let target = if follow { self.focused.or(loaded) } else { loaded.or(self.focused) };
                if let Some(h) = target {
                    self.load(h);
                }
            }
            let mut auto = self.prefs.auto_reload;
            let r = ui.checkbox(&mut auto, "自動で読み直す").on_hover_text(
                "本体でオブジェクトを選び直したり値を変えたりしたときに、表を読み直します。\n\
                 外すと、「再読み込み」を押したときだけ読みます（選んだオブジェクトにも付いていきません）",
            );
            if r.changed() {
                self.prefs.set_auto_reload(auto);
                if auto {
                    // 外していた間の変化を拾う
                    OBJECT_DIRTY.store(true, Ordering::Release);
                }
            }
        });
        if self.focus_moved {
            ui.colored_label(YELLOW, "フォーカスが別のオブジェクトに移りました。未反映を「反映」するか「元に戻す」と、表が切り替わります");
        }
        if self.host_changed {
            ui.colored_label(YELLOW, "本体側でこのオブジェクトが変わりました。反映すると、変わった列は書かずに飛ばします");
        }
        if self.bound_missing {
            ui.colored_label(RED, "このオブジェクトが見つかりません（削除・移動された可能性があります）。反映はできません");
        }
        if let Some(l) = &self.loaded {
            for n in &l.notes {
                ui.colored_label(YELLOW, n);
            }
        }
    }

    fn render_bottom(&mut self, ui: &mut egui::Ui) {
        let has_sel = self.loaded.as_ref().is_some_and(|l| l.table.selection.is_some());
        ui.horizontal_wrapped(|ui| {
            ui.add_enabled_ui(has_sel, |ui| {
                number_text(ui, &mut self.add_text);
                if ui.button("足す").on_hover_text("選んだセルに、左の値を足します").clicked() {
                    match self.add_text.trim().parse::<f64>() {
                        Ok(d) => self.with_table(|t| t.add(d)),
                        Err(_) => self.set_status("足す値が数値ではありません", true),
                    }
                }
                number_text(ui, &mut self.mul_text);
                if ui.button("掛ける").on_hover_text("選んだセルに、左の値を掛けます").clicked() {
                    match self.mul_text.trim().parse::<f64>() {
                        Ok(k) => self.with_table(|t| t.mul(k)),
                        Err(_) => self.set_status("掛ける値が数値ではありません", true),
                    }
                }
                ui.separator();
                if ui.button("等間隔").on_hover_text("列ごとに、選んだ行の最初と最後の値の間を等しい刻みで埋めます").clicked() {
                    self.with_table(|t| t.interpolate());
                }
                if ui.button("同じ値").on_hover_text("選んだセルを、最初に選んだセルの値にします").clicked() {
                    self.with_table(|t| t.fill_with_anchor());
                }
                ui.separator();
                if ui.button("コピー").on_hover_text("選んだセルをタブ区切りでクリップボードへ（表計算ソフトに貼れます）").clicked() {
                    self.copy();
                }
                if ui
                    .button("貼り付け")
                    .on_hover_text("クリップボードのタブ区切りの値を貼ります。1 セルを選んでいればそこを左上にします")
                    .clicked()
                {
                    self.paste();
                }
            });
        });
        ui.horizontal(|ui| {
            let pending = self.pending();
            let can_apply = pending > 0 && !self.bound_missing;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(can_apply, egui::Button::new(format!("反映（{pending} 件）"))).clicked() {
                    self.apply();
                }
                if ui.add_enabled(pending > 0, egui::Button::new("元に戻す")).on_hover_text("未反映の書き換えを捨てます（本体の「元に戻す」とは別です）").clicked() {
                    if let Some(l) = &mut self.loaded {
                        l.table.discard();
                    }
                    let target = if self.focus_moved { self.focused } else { None };
                    self.host_changed = false;
                    if let Some(h) = target.or(self.loaded.as_ref().map(|l| l.handle)) {
                        self.load(h);
                    }
                }
                let text = egui::RichText::new(if self.status.is_empty() { " " } else { &self.status }).small();
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    if self.status_is_error {
                        ui.colored_label(RED, text);
                    } else {
                        ui.label(text);
                    }
                });
            });
        });
    }

    fn with_table(&mut self, f: impl FnOnce(&mut crate::table::Table) -> usize) {
        if let Some(l) = &mut self.loaded {
            let n = f(&mut l.table);
            if n == 0 {
                self.set_status("編集できる列が選ばれていません", true);
            } else {
                self.set_status("", false);
            }
        }
    }

    fn copy(&mut self) {
        let Some(text) = self.loaded.as_ref().and_then(|l| l.table.copy_tsv()) else { return };
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(()) => self.set_status("コピーしました", false),
            Err(e) => self.set_status(format!("クリップボードに書けません: {e}"), true),
        }
    }

    fn paste(&mut self) {
        match arboard::Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(t) => self.paste_text(&t),
            Err(e) => self.set_status(format!("クリップボードを読めません: {e}"), true),
        }
    }

    fn paste_text(&mut self, text: &str) {
        let Some(l) = &mut self.loaded else { return };
        match l.table.paste_tsv(text) {
            Ok(n) => self.set_status(format!("{n} セルを貼り付けました（未反映）"), false),
            Err(e) => self.set_status(e, true),
        }
    }

    /// 表のセルにフォーカスがあるときのキー操作
    fn handle_keys(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.key_focus else { return };
        if self.editing.is_some() {
            return;
        }
        if !ui.ctx().memory(|m| m.has_focus(id)) {
            self.key_focus = None;
            return;
        }
        // 矢印・Tab・Esc で egui がフォーカスを別のウィジェットへ動かさないようにする
        ui.ctx().memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
            )
        });
        let events = ui.input(|i| i.events.clone());
        let (mut copied, mut pasted) = (false, false);
        for ev in events {
            match ev {
                egui::Event::Copy => {
                    if !copied {
                        self.copy();
                        copied = true;
                    }
                }
                egui::Event::Paste(text) => {
                    if !pasted {
                        self.paste_text(&text);
                        pasted = true;
                    }
                }
                egui::Event::Key { key, pressed: true, modifiers, .. } => match key {
                    egui::Key::C if modifiers.command && !copied => {
                        self.copy();
                        copied = true;
                    }
                    egui::Key::V if modifiers.command && !pasted => {
                        self.paste();
                        pasted = true;
                    }
                    egui::Key::Escape => {
                        ui.ctx().memory_mut(|m| m.surrender_focus(id));
                        self.key_focus = None;
                        return;
                    }
                    egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::ArrowLeft | egui::Key::ArrowRight => {
                        let (dr, dc) = match key {
                            egui::Key::ArrowUp => (-1, 0),
                            egui::Key::ArrowDown => (1, 0),
                            egui::Key::ArrowLeft => (0, -1),
                            _ => (0, 1),
                        };
                        if let Some(l) = &mut self.loaded {
                            l.table.move_selection(dr, dc, modifiers.shift);
                        }
                    }
                    egui::Key::Enter | egui::Key::F2 => {
                        if let Some(l) = &mut self.loaded {
                            if let Some(s) = l.table.selection {
                                let (r, c) = s.end;
                                if l.table.columns.get(c).is_some_and(|col| col.editable(r)) {
                                    self.editing = Some(((r, c), l.table.columns[c].text(r), true));
                                    l.table.selection = Some(Selection::single((r, c)));
                                }
                            }
                        }
                        return;
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    fn render_table(&mut self, ui: &mut egui::Ui) {
        self.handle_keys(ui);
        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.command));
        let focus_row = if self.loaded.as_ref().map(|l| l.handle) == self.focused { self.focus_section } else { None };
        let Some(l) = &mut self.loaded else { return };
        let table = &mut l.table;
        if table.columns.is_empty() {
            ui.weak("トラックバーの項目がありません");
            return;
        }
        let used = table.used_modes();
        let mut commit: Option<((usize, usize), String)> = None;
        let mut new_sel: Option<Selection> = None;
        let mut start_edit: Option<(usize, usize)> = None;
        let mut cancel_edit = false;
        let mut focus_to: Option<egui::Response> = None;
        let mut drag_start: Option<(usize, usize)> = None;
        let mut drag_dx: f32 = 0.0;
        let mut drag_end = false;
        let mut settings_toggle: Option<usize> = None;
        let mut hide_req: Option<(String, String, bool)> = None;
        let mut order_req: Option<(String, Option<Vec<String>>)> = None;
        // 見出しのドラッグで並べ替える（同じ効果の中だけ）
        let mut header_rects: Vec<egui::Rect> = Vec::new();
        let mut col_drag_start: Option<usize> = None;
        let mut col_drag_stop = false;
        let mut struct_req: Option<Structure> = None;
        let mut frame_commit: Option<(usize, String)> = None;
        let mut frame_cancel = false;
        let mut frame_start: Option<usize> = None;
        let pending_total = table.pending_count();
        let n_points = table.points();
        let mut key_row: Option<usize> = None;
        // 見えている範囲の外の行は作らない。ただし選択・編集・キー操作・ドラッグに使っている行は作る（作らないとフォーカスが外れる）
        let clip = ui.clip_rect();
        let mut keep: Vec<usize> = Vec::new();
        if let Some(s) = table.selection {
            keep.extend([s.anchor.0, s.end.0]);
        }
        keep.extend(self.editing.as_ref().map(|((r, _), _, _)| *r));
        keep.extend(self.frame_editing.as_ref().map(|(r, _, _)| *r));
        keep.extend(self.key_focus.and(self.key_focus_row));
        keep.extend(self.drag.as_ref().map(|d| d.origin.0));

        egui::Grid::new("midpoint_table").striped(true).spacing([4.0, 2.0]).show(ui, |ui| {
            // 1 段目: 効果名（同じ効果が続くときは最初の列だけ）
            ui.label("");
            for (c, col) in table.columns.iter().enumerate() {
                let first = c == 0 || {
                    let p = &table.columns[c - 1];
                    p.effect != col.effect || p.index != col.index
                };
                if first {
                    let name = if col.index == 0 { col.effect.clone() } else { format!("{} #{}", col.effect, col.index + 1) };
                    ui.strong(name);
                } else {
                    ui.label("");
                }
            }
            for (k, ch) in table.checks.iter().enumerate() {
                let first = k == 0 || table.checks[k - 1].effect != ch.effect || table.checks[k - 1].index != ch.index;
                if first {
                    ui.strong(if ch.index == 0 { ch.effect.clone() } else { format!("{} #{}", ch.effect, ch.index + 1) });
                } else {
                    ui.label("");
                }
            }
            ui.end_row();

            // 2 段目: 項目名
            ui.label("点");
            for (c, col) in table.columns.iter().enumerate() {
                let pending_here = col.pending_count() > 0;
                // 同じ効果の列（並び順を変えられる範囲）
                let group: Vec<usize> = (0..table.columns.len())
                    .filter(|&k| table.columns[k].effect == col.effect && table.columns[k].index == col.index)
                    .collect();
                let pos = group.iter().position(|&k| k == c).unwrap_or(0);
                let mut text = egui::RichText::new(&col.item);
                if col.readonly.is_some() {
                    text = text.weak();
                } else if !col.notes.is_empty() {
                    text = text.color(YELLOW);
                }
                // 文字の選択を切る（既定では選択できるので、ドラッグが文字の選択になって並べ替えられなかった。v0.4.1）
                let mut r = ui.add_sized(
                    [CELL_W, 18.0],
                    egui::Label::new(text).truncate().selectable(false).sense(egui::Sense::click_and_drag()),
                );
                header_rects.push(r.rect);
                if pending_total == 0 {
                    if r.hovered() && group.len() > 1 {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                    }
                    if r.drag_started() && group.len() > 1 {
                        col_drag_start = Some(c);
                    }
                    if r.drag_stopped() {
                        col_drag_stop = true;
                    }
                }
                let mut tips: Vec<String> = col.notes.clone();
                if let Some(ro) = &col.readonly {
                    tips.push(format!("編集できません: {ro}"));
                }
                if !tips.is_empty() {
                    r = r.on_hover_text(tips.join("\n"));
                }
                r.context_menu(|ui| {
                    let b = ui.add_enabled(!pending_here, egui::Button::new("この列を隠す"));
                    let b = if pending_here { b.on_disabled_hover_text("未反映の書き換えがある列は隠せません") } else { b };
                    if b.clicked() {
                        hide_req = Some((col.effect.clone(), col.item.clone(), true));
                        ui.close();
                    }
                    ui.separator();
                    // 並び順（同じ効果の中で。どのオブジェクトでも同じ順になる）
                    if pending_total > 0 {
                        ui.weak("未反映の書き換えがある間は、並び順を変えられません");
                        return;
                    }
                    let names: Vec<String> = group.iter().map(|&k| table.columns[k].item.clone()).collect();
                    for (label, to) in [("左へ", pos.checked_sub(1)), ("右へ", Some(pos + 1).filter(|&p| p < group.len()))] {
                        if ui.add_enabled(to.is_some(), egui::Button::new(label)).clicked() {
                            let mut v = names.clone();
                            v.swap(pos, to.unwrap_or(pos));
                            order_req = Some((col.effect.clone(), Some(v)));
                            ui.close();
                        }
                    }
                    if ui.button(format!("「{}」の列を元の並びに戻す", col.effect)).clicked() {
                        order_req = Some((col.effect.clone(), None));
                        ui.close();
                    }
                });
            }
            for ch in &table.checks {
                let pending_here = ch.pending_count() > 0;
                let r = ui.add_sized([CELL_W, 18.0], egui::Label::new(&ch.item).truncate());
                r.context_menu(|ui| {
                    let b = ui.add_enabled(!pending_here, egui::Button::new("この列を隠す"));
                    if b.clicked() {
                        hide_req = Some((ch.effect.clone(), ch.item.clone(), true));
                        ui.close();
                    }
                });
            }
            ui.end_row();

            // 3 段目: 移動方法（選び直せる）
            ui.label("移動");
            for (c, col) in table.columns.iter_mut().enumerate() {
                mode_combo(ui, c, col, &used, &self.movements, &mut self.mode_filter);
            }
            for _ in &table.checks {
                ui.add_sized([CELL_W, 18.0], egui::Label::new(egui::RichText::new("区間ごと").small().weak()))
                    .on_hover_text("区間ごとのチェック。各行は、その点から始まる区間の値です");
            }
            ui.end_row();

            // 4 段目: 設定（加速・減速・中間点無視と設定値）
            ui.label("設定");
            for (c, col) in table.columns.iter_mut().enumerate() {
                let open = self.settings_col == Some(c);
                if settings_button(ui, col, &self.movements, open) {
                    settings_toggle = Some(c);
                }
            }
            for _ in &table.checks {
                ui.label("");
            }
            ui.end_row();

            for r in 0..table.points() {
                // 行の上端（画面の座標）。見えている範囲から離れていれば、同じ高さの空きだけ置く
                let top = ui.cursor().min.y;
                if (top > clip.bottom() + ROW_MARGIN || top + ROW_H < clip.top() - ROW_MARGIN) && !keep.contains(&r) {
                    ui.allocate_exact_size(egui::vec2(0.0, ROW_H), egui::Sense::hover());
                    ui.end_row();
                    continue;
                }
                let mark = if focus_row == Some(r) { "▶" } else { " " };
                let midpoint = r > 0 && r + 1 < n_points;
                match &mut self.frame_editing {
                    Some((er, buf, first)) if *er == r => {
                        let resp = ui.add_sized([56.0, 18.0], egui::TextEdit::singleline(buf));
                        if *first {
                            resp.request_focus();
                            *first = false;
                        }
                        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            frame_cancel = true;
                        } else if resp.lost_focus() {
                            frame_commit = Some((r, buf.clone()));
                        }
                    }
                    _ => {
                        let label = egui::RichText::new(format!("{mark} {}", table.frames[r]));
                        let resp = ui.add(egui::Label::new(if midpoint { label } else { label.weak() }).sense(egui::Sense::click()));
                        let resp = if midpoint {
                            resp.on_hover_text("中間点。ダブルクリックで位置（フレーム）を変える。右クリックで消す・足す")
                        } else {
                            resp.on_hover_text("開始点・終了点（位置はタイムラインで変える）。右クリックで中間点を足す")
                        };
                        if midpoint && resp.double_clicked() && pending_total == 0 {
                            frame_start = Some(r);
                        }
                        let gap_mid = (r + 1 < n_points && table.frames[r + 1] > table.frames[r] + 1)
                            .then(|| (table.frames[r] + table.frames[r + 1]) / 2);
                        resp.context_menu(|ui| {
                            if pending_total > 0 {
                                ui.weak("未反映の書き換えがある間は、中間点を足す・動かす・消すことはできません");
                                return;
                            }
                            if midpoint {
                                if ui.button("位置を変える…").clicked() {
                                    frame_start = Some(r);
                                    ui.close();
                                }
                                if ui.button("この中間点を消す").clicked() {
                                    struct_req = Some(Structure::Delete { row: r });
                                    ui.close();
                                }
                            }
                            if let Some(f) = gap_mid {
                                if ui.button(format!("次の点とのあいだ（{f}）に中間点を足す")).clicked() {
                                    struct_req = Some(Structure::Add { frame: f });
                                    ui.close();
                                }
                            }
                        });
                    }
                }
                for (c, col) in table.columns.iter().enumerate() {
                    if let Some(((er, ec), buf, first)) = &mut self.editing {
                        if (*er, *ec) == (r, c) {
                            let resp = ui.add_sized([CELL_W, 18.0], egui::TextEdit::singleline(buf));
                            if *first {
                                resp.request_focus();
                                *first = false;
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                cancel_edit = true;
                            } else if resp.lost_focus() {
                                commit = Some(((r, c), buf.clone()));
                            }
                            continue;
                        }
                    }
                    let selected = table.selection.is_some_and(|s| s.contains((r, c)));
                    let ignored = col.is_ignored_row(r);
                    let mut text = egui::RichText::new(if ignored {
                        "—".to_string()
                    } else if col.is_static() && !col.is_edited(r) {
                        format!("({})", col.text(r))
                    } else {
                        col.text(r)
                    });
                    if !col.editable(r) || (col.is_static() && !col.is_edited(r)) {
                        text = text.weak();
                    }
                    if col.is_edited(r) {
                        text = text.color(YELLOW);
                    }
                    let mut resp = ui
                        .add_sized([CELL_W, 18.0], egui::Button::selectable(selected, text))
                        .interact(egui::Sense::click_and_drag());
                    if ignored {
                        resp = resp.on_hover_text("中間点無視: この点の値は保存されません（始点と終点だけが効きます）");
                    }
                    if col.editable(r) && (resp.hovered() || resp.dragged()) {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                    }
                    if resp.clicked() {
                        new_sel = Some(match (shift, table.selection) {
                            (true, Some(s)) => Selection { anchor: s.anchor, end: (r, c) },
                            _ => Selection::single((r, c)),
                        });
                        focus_to = Some(resp.clone());
                        key_row = Some(r);
                    }
                    if resp.double_clicked() && col.editable(r) {
                        start_edit = Some((r, c));
                    }
                    if col.editable(r) {
                        if resp.drag_started() {
                            drag_start = Some((r, c));
                            focus_to = Some(resp.clone());
                            key_row = Some(r);
                        }
                        if resp.dragged() {
                            drag_dx += resp.drag_delta().x;
                        }
                        if resp.drag_stopped() {
                            drag_end = true;
                        }
                    }
                }
                // 区間ごとのチェック（行 r はその点から始まる区間。最後の行には区間が無い）
                for ch in table.checks.iter_mut() {
                    if r + 1 < n_points {
                        let mut on = ch.value(r);
                        let text = if ch.is_edited(r) { egui::RichText::new("").color(YELLOW) } else { egui::RichText::new("") };
                        let resp = ui.add_sized([CELL_W, 18.0], egui::Checkbox::new(&mut on, text));
                        if resp.changed() {
                            ch.toggle(r);
                        }
                        if ch.is_edited(r) {
                            resp.on_hover_text("未反映");
                        }
                    } else {
                        ui.label("");
                    }
                }
                ui.end_row();
            }
        });

        if let Some(s) = new_sel {
            table.selection = Some(s);
        }
        // 横ドラッグで値を増減する（未反映の書き換えになるだけ。本体へは「反映」で書く）
        if let Some(cell) = drag_start {
            self.drag = Some(Drag { origin: cell, targets: table.drag_targets(cell), offset: 0.0 });
        }
        if let Some(d) = &mut self.drag {
            if drag_dx != 0.0 {
                let scale = if shift { 0.1 } else if ctrl { 10.0 } else { 1.0 };
                d.offset += drag_dx as f64 * DRAG_STEP * scale;
                for ((r, c), base) in &d.targets {
                    table.set_cell(*r, *c, base + d.offset);
                }
            }
        }
        if drag_end {
            self.drag = None;
        }
        // クリックしたセルにフォーカスを置き、キー操作（Ctrl+C / Ctrl+V など）を本体へ渡さず表で受ける
        if let Some(resp) = focus_to {
            resp.request_focus();
            self.key_focus = Some(resp.id);
            self.key_focus_row = key_row;
        }
        if cancel_edit {
            self.editing = None;
        } else if let Some(((r, c), buf)) = commit {
            match buf.trim().parse::<f64>() {
                Ok(v) if v.is_finite() => {
                    table.set_cell(r, c, v);
                    self.status.clear();
                }
                _ => {
                    self.status = format!("「{}」は数値ではありません", buf.trim());
                    self.status_is_error = true;
                }
            }
            self.editing = None;
        }
        if let Some((r, c)) = start_edit {
            self.editing = Some(((r, c), table.columns[c].text(r), true));
            table.selection = Some(Selection::single((r, c)));
        }

        // 中間点の位置の入力
        if frame_cancel {
            self.frame_editing = None;
        } else if let Some((r, buf)) = frame_commit {
            self.frame_editing = None;
            match buf.trim().parse::<usize>() {
                Ok(f) if f != table.frames[r] => struct_req = Some(Structure::Move { row: r, frame: f }),
                Ok(_) => {}
                Err(_) => {
                    self.status = format!("「{}」はフレーム番号ではありません", buf.trim());
                    self.status_is_error = true;
                }
            }
        }
        if let Some(r) = frame_start {
            self.frame_editing = Some((r, table.frames[r].to_string(), true));
        }
        if struct_req.is_some() {
            self.struct_request = struct_req;
        }

        // 設定のウィンドウ（同じボタンをもう一度押すと閉じる）
        if hide_req.is_some() {
            self.hide_request = hide_req;
        }
        if let Some(c) = col_drag_start {
            self.col_drag = Some(c);
        }
        if let Some(src) = self.col_drag {
            // ドラッグ先: 同じ効果の列のうち、ポインタの横位置にある列（端より外なら端の列）
            let group: Vec<usize> = (0..table.columns.len())
                .filter(|&k| table.columns[k].effect == table.columns[src].effect && table.columns[k].index == table.columns[src].index)
                .collect();
            let x = ui.input(|i| i.pointer.interact_pos()).map(|p| p.x);
            let target = x.and_then(|x| {
                group
                    .iter()
                    .copied()
                    .filter(|&k| k < header_rects.len())
                    .min_by(|&a, &b| (header_rects[a].center().x - x).abs().total_cmp(&(header_rects[b].center().x - x).abs()))
            });
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if let (Some(t), Some(r)) = (target, target.and_then(|t| header_rects.get(t))) {
                if t != src {
                    // 入る位置に縦線を引く（右へ動かすなら列の右、左へなら左）
                    let lx = if t > src { r.right() + 2.0 } else { r.left() - 2.0 };
                    ui.painter().vline(lx, r.top()..=r.bottom(), egui::Stroke::new(2.0, YELLOW));
                }
            }
            if col_drag_stop || !ui.input(|i| i.pointer.any_down()) {
                self.col_drag = None;
                if let Some(t) = target.filter(|&t| t != src) {
                    let mut names: Vec<String> = group.iter().map(|&k| table.columns[k].item.clone()).collect();
                    let (from, to) = (group.iter().position(|&k| k == src).unwrap_or(0), group.iter().position(|&k| k == t).unwrap_or(0));
                    let moved = names.remove(from);
                    names.insert(to, moved);
                    order_req = Some((table.columns[src].effect.clone(), Some(names)));
                }
            }
        }
        if order_req.is_some() {
            self.order_request = order_req;
        }
        if let Some(c) = settings_toggle {
            self.settings_col = if self.settings_col == Some(c) { None } else { Some(c) };
        }
        if let Some(c) = self.settings_col {
            match table.columns.get_mut(c).filter(|col| col.readonly.is_none() && col.effective_mode().is_some()) {
                None => self.settings_col = None,
                Some(col) => {
                    let mut open = true;
                    let title = if col.index == 0 {
                        format!("設定: {} / {}", col.effect, col.item)
                    } else {
                        format!("設定: {} #{} / {}", col.effect, col.index + 1, col.item)
                    };
                    let area = ui.ctx().content_rect().shrink(4.0);
                    egui::Window::new(title)
                        .id(egui::Id::new("midpoint_settings"))
                        .open(&mut open)
                        .collapsible(false)
                        .resizable(false)
                        .constrain_to(area)
                        .show(ui.ctx(), |ui| {
                            // パネルが低いときも見出し（×）が隠れないように、中をスクロールさせる
                            egui::ScrollArea::vertical()
                                .max_height((area.height() - 40.0).max(60.0))
                                .show(ui, |ui| settings_editor(ui, c, col, &self.movements));
                        });
                    if !open {
                        self.settings_col = None;
                    }
                }
            }
        }
    }
}

/// 「足す」「掛ける」の値の入力欄。入力中に Esc を押すと、入力を始める前の文字に戻す
fn number_text(ui: &mut egui::Ui, text: &mut String) {
    let resp = ui.add(egui::TextEdit::singleline(text).desired_width(48.0));
    let key = resp.id.with("before_edit");
    if resp.gained_focus() {
        ui.data_mut(|d| d.insert_temp(key, text.clone()));
    }
    if resp.lost_focus() {
        let before = ui.data_mut(|d| d.remove_temp::<String>(key));
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(b) = before {
                *text = b;
            }
        }
    }
}

/// 数値の欄（ルール au2-rs-plugin「入力の確定と取り消し」）。打っている途中の値は使わず、Enter かほかをクリックしたときに確定する。
/// 入力中の Esc は入力前の値に戻す。ドラッグで変えた値はその場で使う。
///
/// egui 0.36.2 の `DragValue` は `update_while_editing(false)` でも、Esc の次のフレームで打った文字を値にしてしまう
/// （フォーカスを失ったとみなす期間が 2 フレームあり、2 フレーム目には Esc が押されていない）。
/// Esc のフレームの値を覚えておき、次のフレームで戻す（参照実装 `LayerSilenceCut_H/src/gui.rs` の `setting_value`）
fn committed_drag<T>(
    ui: &mut egui::Ui,
    value: &mut T,
    style: impl for<'a> FnOnce(egui::DragValue<'a>) -> egui::DragValue<'a>,
) -> egui::Response
where
    T: egui::emath::Numeric + Default + Send + Sync,
{
    let resp = ui.add(style(egui::DragValue::new(value).update_while_editing(false)));
    let key = resp.id.with("escaped");
    let pass = ui.ctx().cumulative_pass_nr();
    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        ui.data_mut(|d| d.insert_temp(key, (pass, *value)));
    } else if let Some((at, before)) = ui.data_mut(|d| d.remove_temp::<(u64, T)>(key)) {
        if pass == at + 1 {
            *value = before;
        }
    }
    resp
}

/// 列の移動方法を選ぶコンボボックス。
/// 選べるのは、本体に登録されている移動方法（`movement.rs`。`aviutl2.ini` は使わない）と、この表の列が今使っているものだけ。
/// 登録されていない名前を書くと本体が例外を出す
fn mode_combo(ui: &mut egui::Ui, c: usize, col: &mut Column, used: &[(String, Option<String>)], scan: &Scan, filter: &mut String) {
    if col.readonly.is_some() {
        let text = col.mode.clone().unwrap_or_else(|| "移動なし".into());
        ui.add_sized([CELL_W, 18.0], egui::Label::new(egui::RichText::new(text).small().weak()).truncate());
        return;
    }
    let current = col.effective_mode();
    let changed = col.mode_changed() || col.needs_mode();
    let mut label = current.as_ref().map(|(m, _)| m.clone()).unwrap_or_else(|| "移動なし".into());
    if current.as_ref().is_some_and(|(m, _)| scan.find(m).is_some_and(|mv| !mv.params.is_empty())) || !col.params.is_empty() {
        label.push_str(" *");
    }
    let mut text = egui::RichText::new(label).small();
    if changed {
        text = text.color(YELLOW);
    }
    let mut choice: Option<ModeChange> = None;
    let resp = egui::ComboBox::from_id_salt(("mode", c)).width(CELL_W).height(320.0).selected_text(text).show_ui(ui, |ui| {
        ui.add(egui::TextEdit::singleline(filter).hint_text("絞り込み").desired_width(160.0));
        let hit = |name: &str| filter.is_empty() || name.contains(filter.as_str());
        let cur = current.as_ref().map(|(m, _)| m.as_str());

        let can_static = col.all_same();
        let r = ui.add_enabled(can_static, egui::Button::selectable(cur.is_none(), "移動なし"));
        let r = if can_static { r } else { r.on_disabled_hover_text("点ごとに値が違うので、移動なしにできません") };
        if r.clicked() {
            choice = Some(ModeChange::Static);
        }
        ui.separator();
        let mut shown: Vec<&str> = Vec::new();
        // この表で使っているもの（設定はその列のものではなく、定義の初期値を使う。分からなければその列の設定）
        for (name, tail) in used {
            if !hit(name) {
                continue;
            }
            shown.push(name);
            if ui.selectable_label(cur == Some(name.as_str()), format!("{name}（使用中）")).clicked() {
                let t = if col.mode.as_deref() == Some(name.as_str()) {
                    col.tail.clone()
                } else {
                    scan.find(name).map(|m| Some(movement::default_tail(m))).unwrap_or(tail.clone())
                };
                choice = Some(ModeChange::Set(name.clone(), t));
            }
        }
        for m in &scan.movements {
            if shown.contains(&m.name.as_str()) || !hit(&m.name) {
                continue;
            }
            let mut r = ui.selectable_label(cur == Some(m.name.as_str()), &m.name);
            if !m.params.is_empty() {
                let defs: Vec<String> = m.params.iter().map(|p| format!("{} = {}", p.label, p.default)).collect();
                r = r.on_hover_text(format!("設定値（初期値で書きます）\n{}", defs.join("\n")));
            }
            if r.clicked() {
                let t = if col.mode.as_deref() == Some(m.name.as_str()) { col.tail.clone() } else { Some(movement::default_tail(m)) };
                choice = Some(ModeChange::Set(m.name.clone(), t));
            }
        }
    });
    resp.response.on_hover_text(col.mode_detail());
    if let Some(ch) = choice {
        col.choose_mode(ch);
    }
}

/// 書くときの移動方法の設定値の定義。`.tra2` の `--param`（組み込みはランダム移動だけ分かる）。
/// 定義が分からなくても、元の移動方法のまま設定値が読めていれば「設定値N」として出す
fn param_defs(col: &Column, scan: &Scan) -> Vec<movement::ParamDef> {
    let Some((name, _)) = col.effective_mode() else { return Vec::new() };
    if let Some(m) = scan.find(&name).filter(|m| !m.params.is_empty()) {
        return m.params.clone();
    }
    if col.same_mode() {
        return col
            .params
            .iter()
            .enumerate()
            .map(|(i, v)| movement::ParamDef { label: format!("設定値{}", i + 1), kind: movement::ParamKind::Number, default: *v })
            .collect();
    }
    Vec::new()
}

/// 列の設定（加速・減速・中間点無視と設定値）の要約ボタン。押すと設定のウィンドウ（`settings_editor`）を開く。
/// egui のメニュー（menu_button）は中のチェックやリストを押した時点で閉じてしまい、リストが選べなかった（v0.2.0）ので使わない
fn settings_button(ui: &mut egui::Ui, col: &Column, scan: &Scan, open: bool) -> bool {
    if col.readonly.is_some() || col.effective_mode().is_none() {
        ui.label("");
        return false;
    }
    let defs = param_defs(col, scan);
    let flags = col.current_flags();
    let mut summary = Vec::new();
    for (bit, mark) in [(1u32, "加"), (2, "減"), (4, "無")] {
        if flags & bit != 0 {
            summary.push(mark.to_string());
        }
    }
    if col.params_editable(&defs).is_ok() {
        summary.push(crate::table::format_params(&col.param_values(&defs)));
    }
    let label = if summary.is_empty() { "—".to_string() } else { summary.join(" ") };
    let mut text = egui::RichText::new(label).small();
    if col.flags_edit.is_some() || col.param_edit.is_some() {
        text = text.color(YELLOW);
    }
    ui.add_sized([CELL_W, 18.0], egui::Button::selectable(open, text))
        .on_hover_text(format!("{}\n（押すと設定のウィンドウを開きます）", col.mode_detail()))
        .clicked()
}

/// 設定のウィンドウの中身
fn settings_editor(ui: &mut egui::Ui, c: usize, col: &mut Column, scan: &Scan) {
    let Some((mode, _)) = col.effective_mode() else {
        ui.weak("移動なしには設定がありません");
        return;
    };
    let defs = param_defs(col, scan);
    ui.strong(&mode);
    let mut f = col.current_flags();
    let mut changed = false;
    for (bit, name) in [(1u32, "加速"), (2, "減速"), (4, "中間点無視（始点と終点だけ）")] {
        let mut on = f & bit != 0;
        if ui.checkbox(&mut on, name).changed() {
            f = if on { f | bit } else { f & !bit };
            changed = true;
        }
    }
    if changed {
        col.set_flags(f);
    }
    ui.separator();
    match col.params_editable(&defs) {
        Err(e) => {
            ui.weak(e);
        }
        Ok(()) => {
            let mut v = col.param_values(&defs);
            let before = v.clone();
            egui::Grid::new(("params", c)).num_columns(2).show(ui, |ui| {
                for (i, d) in defs.iter().enumerate() {
                    ui.label(&d.label);
                    match &d.kind {
                        movement::ParamKind::Number => {
                            committed_drag(ui, &mut v[i], |d| d.speed(0.1));
                        }
                        movement::ParamKind::Check => {
                            let mut on = v[i] != 0.0;
                            if ui.checkbox(&mut on, "").changed() {
                                v[i] = if on { 1.0 } else { 0.0 };
                            }
                        }
                        movement::ParamKind::Select(opts) => {
                            let cur = opts
                                .iter()
                                .find(|(_, x)| *x == v[i])
                                .map(|(k, _)| k.clone())
                                .unwrap_or_else(|| v[i].to_string());
                            egui::ComboBox::from_id_salt(("sel", c, i)).selected_text(cur).show_ui(ui, |ui| {
                                for (k, x) in opts {
                                    if ui.selectable_label(v[i] == *x, k).clicked() {
                                        v[i] = *x;
                                    }
                                }
                            });
                        }
                    }
                    ui.end_row();
                }
            });
            if ui.button("初期値に戻す").clicked() {
                v = defs.iter().map(|d| d.default).collect();
            }
            if v != before {
                col.set_params(v);
            }
        }
    }
    ui.separator();
    ui.weak("書き換えは「反映」を押すまで本体に書きません");
}

impl MidpointTableApp {
    /// 周期複製のウィンドウ。「複製」を押したときだけ書く（1 回の call_edit_section）
    fn render_duplicate(&mut self, ctx: &egui::Context) {
        if !self.dup_open {
            return;
        }
        let pending = self.pending();
        let mut open = true;
        let mut run = false;
        let Some(l) = &self.loaded else {
            self.dup_open = false;
            return;
        };
        let (first, last) = (l.table.frames.first().copied().unwrap_or(0), l.table.frames.last().copied().unwrap_or(0));
        let len = last - first + 1;
        if self.dup_deltas.len() != l.table.columns.len() {
            self.dup_deltas = vec![0.0; l.table.columns.len()];
        }
        let plan = edit_ops::placements(l.layer, first, len, self.dup_interval, self.dup_count, self.dup_below);
        // パネルに収める。「ずらす量」の一覧の高さを、パネルの高さから一覧以外の高さ（前のフレームで測った）を引いた残りにする
        let area = ctx.content_rect().shrink(4.0);
        let list_max = (area.height() - self.dup_fixed_h - 8.0).clamp(48.0, 240.0);
        let mut list_h = 0.0;
        let shown = egui::Window::new("周期複製")
            .id(egui::Id::new("midpoint_duplicate"))
            .open(&mut open)
            .resizable(false)
            .constrain_to(area)
            .show(ctx, |ui| {
            ui.label(format!("対象: {}", l.label));
            egui::Grid::new("dup_opts").num_columns(2).show(ui, |ui| {
                ui.label("間隔（フレーム）");
                committed_drag(ui, &mut self.dup_interval, |d| d.range(0..=100_000));
                ui.end_row();
                ui.label("回数");
                committed_drag(ui, &mut self.dup_count, |d| d.range(1..=100));
                ui.end_row();
                ui.label("置き方");
                ui.vertical(|ui| {
                    ui.radio_value(&mut self.dup_below, false, "同じレイヤーに続けて");
                    ui.radio_value(&mut self.dup_below, true, "1 つごとに下のレイヤーへ");
                });
                ui.end_row();
            });
            ui.separator();
            ui.label("1 つごとにずらす量（全部の点に足す）");
            let out = egui::ScrollArea::vertical().max_height(list_max).show(ui, |ui| {
                egui::Grid::new("dup_deltas").num_columns(2).show(ui, |ui| {
                    for (c, col) in l.table.columns.iter().enumerate() {
                        if col.readonly.is_some() {
                            continue;
                        }
                        ui.label(format!("{} / {}", col.effect, col.item));
                        committed_drag(ui, &mut self.dup_deltas[c], |d| d.speed(0.1));
                        ui.end_row();
                    }
                });
            });
            list_h = out.inner_rect.height();
            ui.separator();
            match &plan {
                Ok(p) => {
                    let shown: Vec<String> = p.iter().take(6).map(|(ly, f)| format!("L{} {f}", ly + 1)).collect();
                    let more = if p.len() > 6 { format!(" ほか {} 個", p.len() - 6) } else { String::new() };
                    ui.weak(format!("置き場所: {}{more}", shown.join(" / ")));
                }
                Err(e) => {
                    ui.colored_label(RED, e);
                }
            }
            let b = ui.add_enabled(plan.is_ok() && pending == 0, egui::Button::new(format!("複製（{} 個）", self.dup_count)));
            let b = b.on_disabled_hover_text(if pending > 0 { "未反映の書き換えがある間は複製できません" } else { "置き場所を直してください" });
            if b.clicked() {
                run = true;
            }
            ui.weak("置き場所に別のオブジェクトがあるときは、何も作りません。値は書き換える前の値（未反映を含まない）からずらします");
        });
        if let Some(r) = shown {
            self.dup_fixed_h = (r.response.rect.height() - list_h).max(0.0);
        }
        if !open {
            self.dup_open = false;
        }
        if run {
            let Some(l) = &self.loaded else { return };
            let n = self.dup_count;
            let shifts: Vec<(String, usize, String, Vec<String>)> = l
                .table
                .columns
                .iter()
                .zip(&self.dup_deltas)
                .filter(|(col, d)| **d != 0.0 && col.readonly.is_none())
                .map(|(col, d)| {
                    (col.effect.clone(), col.index, col.item.clone(), (1..=n).map(|k| col.shifted(d * k as f64)).collect())
                })
                .collect();
            match edit_ops::duplicate(l.bind(), self.dup_interval, n, self.dup_below, shifts) {
                Ok(msg) => self.set_status(msg, false),
                Err(e) => self.set_status(e, true),
            }
        }
    }
}

impl eframe::App for MidpointTableApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 本体のパネルに出ていない間は、読みも描きもしない（イベントの印は残り、出たときに読む）
        let visible = crate::window_visible();
        if !visible {
            // 隠れるたびに 1 回出す（実機で判定が効いているかの目安）。起動直後は本体のパネルに入る前に一瞬見えてから隠れるので、
            // しばらく見えていたときだけ出す（v0.5.0 の最初の版は 1 回限りにしていて、起動時に使い切っていた）
            if self.visible_since.is_some_and(|t| t.elapsed() >= SHOWN_LONG) {
                tracing::info!("[MidpointTable_H] ウィンドウが隠れました。見えていない間は表を読み直しません");
            }
            self.visible_since = None;
            ui.ctx().request_repaint_after(HIDDEN_CHECK);
            return;
        }
        self.visible_since.get_or_insert_with(Instant::now);
        // Esc で小さなウィンドウを閉じるのは、押す前にどこにもフォーカスが無く、ポップアップも開いていなかったときだけ
        // （値の入力・表のキー操作・コンボボックスの Esc は、そちらを取り消すだけにする）。
        // egui はこのフレームの始めに Esc でフォーカスを外している（`Memory::begin_pass`）ので、前のフレームの終わりの状態で見る
        let esc_closes = ui.input(|i| i.key_pressed(egui::Key::Escape))
            && !self.busy_last_frame
            && self.editing.is_none()
            && self.frame_editing.is_none();
        // タイムラインのメニュー（中間点を足す）の結果
        if let Some((msg, error)) = crate::MENU_RESULT.lock().take() {
            self.set_status(msg, error);
        }
        // セルを編集している間は監視で表を差し替えない
        if self.prefs.auto_reload {
            if self.editing.is_none() && self.frame_editing.is_none() {
                self.poll_if_due(ui.ctx());
            }
            ui.ctx().request_repaint_after(LIGHT_INTERVAL);
        }
        egui::Panel::top("top").show(ui, |ui| self.render_top(ui));
        egui::Panel::bottom("bottom").show(ui, |ui| self.render_bottom(ui));
        // 中央パネルは最後に追加する（ルール au2-rs-plugin「egui レイアウト」）
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| self.render_table(ui));
        });
        self.render_duplicate(ui.ctx());
        if esc_closes {
            if self.settings_col.is_some() {
                self.settings_col = None;
            } else {
                self.dup_open = false;
            }
        }
        self.busy_last_frame = ui.ctx().memory(|m| m.focused().is_some()) || ui.ctx().any_popup_open();
        // 中間点を足す・動かす・消す（ボタン・メニューを押したときだけ。1 回が本体の Undo 1 回分）
        if let Some(op) = self.struct_request.take() {
            if self.pending() == 0 {
                if let Some((h, bind)) = self.loaded.as_ref().map(|l| (l.handle, l.bind())) {
                    match edit_ops::change_structure(bind, op) {
                        Ok(msg) => self.set_status(msg, false),
                        Err(e) => self.set_status(e, true),
                    }
                    self.load(h);
                }
            }
        }
        // 列を隠す・戻す（保存して表を作り直す。未反映があるときは UI で押せない）
        if let Some((e, order)) = self.order_request.take() {
            if self.pending() == 0 {
                match order {
                    Some(v) => self.prefs.set_order(&e, v),
                    None => self.prefs.clear_order(&e),
                }
                self.settings_col = None;
                if let Some(h) = self.loaded.as_ref().map(|l| l.handle) {
                    self.load(h);
                }
            }
        }
        if let Some((e, i, hide)) = self.hide_request.take() {
            if self.pending() == 0 {
                self.prefs.set_hidden(&e, &i, hide);
                self.settings_col = None;
                if let Some(h) = self.loaded.as_ref().map(|l| l.handle) {
                    self.load(h);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{committed_drag, number_text};
    use aviutl2_eframe::egui;

    fn key(k: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// 1 フレーム回す。テクスチャの差分は使わないので捨てる（捨てずに落とすと、デバッグビルドで epaint の検査が止める）
    fn run(ctx: &egui::Context, events: Vec<egui::Event>, f: impl FnMut(&mut egui::Ui)) {
        let mut out = ctx.run_ui(egui::RawInput { events, ..Default::default() }, f);
        out.textures_delta.clear();
    }

    fn frame_drag(ctx: &egui::Context, v: &mut usize, events: Vec<egui::Event>) {
        run(ctx, events, |ui| {
            committed_drag(ui, v, |d| d.range(0..=100_000));
        });
    }

    /// Tab で欄にフォーカスを移し、全部選んで `typed` を打つ（まだ確定しない）
    fn type_into(ctx: &egui::Context, v: &mut usize, typed: &str) {
        frame_drag(ctx, v, vec![]);
        frame_drag(ctx, v, vec![key(egui::Key::Tab, egui::Modifiers::NONE)]);
        frame_drag(ctx, v, vec![key(egui::Key::A, egui::Modifiers::COMMAND), egui::Event::Text(typed.into())]);
    }

    #[test]
    fn drag_value_applies_typed_value_only_on_enter() {
        let ctx = egui::Context::default();
        let mut v = 30;
        type_into(&ctx, &mut v, "90");
        assert_eq!(v, 30, "打っている途中は値にしない");
        frame_drag(&ctx, &mut v, vec![key(egui::Key::Enter, egui::Modifiers::NONE)]);
        assert_eq!(v, 90, "Enter で打った値になる");
    }

    #[test]
    fn drag_value_escape_discards_typed_value() {
        let ctx = egui::Context::default();
        let mut v = 30;
        type_into(&ctx, &mut v, "90");
        frame_drag(&ctx, &mut v, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
        assert_eq!(v, 30, "Esc で入力前の値のまま");
        // egui はフォーカスが外れてから 2 フレームのあいだ「外れた」と扱う。その後のフレームでも打った値が入らないこと
        for _ in 0..3 {
            frame_drag(&ctx, &mut v, vec![]);
            assert_eq!(v, 30, "Esc の後のフレームでも入力前の値のまま");
        }
    }

    fn frame_text(ctx: &egui::Context, t: &mut String, focus: bool, events: Vec<egui::Event>) {
        // Tab で欄にフォーカスを移す（クリックと同じ）
        let events = if focus { vec![key(egui::Key::Tab, egui::Modifiers::NONE)] } else { events };
        run(ctx, events, |ui| {
            number_text(ui, t);
        });
    }

    #[test]
    fn number_text_reverts_on_escape() {
        let ctx = egui::Context::default();
        let mut t = String::from("10");
        frame_text(&ctx, &mut t, true, vec![]);
        frame_text(&ctx, &mut t, false, vec![]);
        frame_text(&ctx, &mut t, false, vec![egui::Event::Text("5".into())]);
        assert_eq!(t, "105", "打っている途中の文字は欄に出る");
        frame_text(&ctx, &mut t, false, vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
        assert_eq!(t, "10", "入力中の Esc で入力前の文字に戻る");
        for _ in 0..3 {
            frame_text(&ctx, &mut t, false, vec![]);
            assert_eq!(t, "10");
        }
    }
}
