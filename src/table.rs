//! 表のモデル。行 = 点（開始・中間点・終了）、列 = トラックバー項目。
//!
//! 書き換えは「未反映」として持ち、`write_jobs` で列ごとの書き込みにまとめる。本体へ書くのは
//! 「反映」を押したときだけ（`edit_ops::apply`）。

use std::cell::Cell;

use crate::movement::ParamDef;
use crate::track;

/// 移動なしの項目を移動ありにするときの既定。設定 `0` は実機で確かめた形（直線移動）
pub const DEFAULT_MODE: (&str, &str) = ("直線移動", "0");

/// 値が始点と終点の 2 つだけで保存される移動方法か（中間点プローブ_H の M1・M3）
fn two_value(mode: &str, tail: Option<&str>) -> bool {
    track::flags_of(tail) & track::FLAG_TWOPOINT != 0 || crate::movement::TWO_VALUE.contains(&mode)
}

/// 移動方法の選び直し
#[derive(Debug, Clone, PartialEq)]
pub enum ModeChange {
    /// 移動なしにする（全部の点が同じ値のときだけ選べる）
    Static,
    /// 移動方法と設定（`0|設定値…`）
    Set(String, Option<String>),
}

#[derive(Debug, Clone)]
pub struct Column {
    pub effect: String,
    pub index: usize,
    pub item: String,
    /// 読んだときの文字列。書く直前に読み直して、これと違えば書かない
    pub raw: String,
    pub mode: Option<String>,
    pub tail: Option<String>,
    pub decimals: usize,
    /// 点の数に合わせた値（移動なしは同じ値を並べる）
    pub base: Vec<f64>,
    /// 未反映の書き換え
    pub edits: Vec<Option<f64>>,
    /// 選び直した移動方法（未反映）
    pub mode_change: Option<ModeChange>,
    /// 表示だけの注意（値の数の不一致、中間点無視、時間制御）
    pub notes: Vec<String>,
    /// 編集できない理由
    pub readonly: Option<String>,
    /// 移動方法の設定値（`get_object_track_info` の `params`）。表示だけ（書き戻しは `tail` をそのまま使う）
    pub params: Vec<f64>,
    pub accelerate: bool,
    pub decelerate: bool,
    /// 書き換えた加速・減速・中間点無視（設定の `|` の前の数値。未反映）
    pub flags_edit: Option<u32>,
    /// 書き換えた設定値（未反映）。数は移動方法の定義に合わせる
    pub param_edit: Option<Vec<f64>>,
    /// `all_same` と `edited_count` の結果。どちらも全部の点を文字列にして比べるので、セルを書き換えるまで覚えておく。
    /// 移動なしの列ではセル 1 つを描くたびに `all_same` が数回呼ばれ、点 1800 個で 1 フレーム 330 ms かかっていた（v0.5.3）
    same_cache: Cell<Option<bool>>,
    edited_cache: Cell<Option<usize>>,
}

impl Column {
    /// 読んだ文字列から作る。`points` は点の数
    pub fn new(effect: &str, index: usize, item: &str, raw: &str, points: usize) -> Self {
        let mut col = Column {
            effect: effect.to_string(),
            index,
            item: item.to_string(),
            raw: raw.to_string(),
            mode: None,
            tail: None,
            decimals: 2,
            base: vec![0.0; points],
            edits: vec![None; points],
            mode_change: None,
            notes: Vec::new(),
            readonly: None,
            params: Vec::new(),
            accelerate: false,
            decelerate: false,
            flags_edit: None,
            param_edit: None,
            same_cache: Cell::new(None),
            edited_cache: Cell::new(None),
        };
        match track::parse(raw) {
            Ok(t) => {
                // 組み立て直して元に戻らない形は、壊さないように読むだけにする
                if track::build(&t.values, t.decimals, t.mode.as_deref(), t.tail.as_deref()) != raw {
                    col.readonly = Some(format!("この形式は組み立て直せません: {raw}"));
                }
                let twopoint = t.mode.as_deref().is_some_and(|m| two_value(m, t.tail.as_deref()));
                if twopoint && t.values.len() == 2 && points > 2 {
                    // 中間点無視は始点と終点の 2 つだけが保存される（プローブ M1）。途中の行は表示用に直線で埋める
                    let (a, b) = (t.values[0], t.values[1]);
                    col.base = (0..points).map(|r| a + (b - a) * r as f64 / (points - 1) as f64).collect();
                } else {
                    if t.mode.is_some() && t.values.len() != points {
                        col.notes.push(format!("値が {} 個（点は {points} 個）。反映すると点の数に揃えます", t.values.len()));
                    }
                    col.base = track::fit(&t.values, points);
                }
                col.mode = t.mode;
                col.tail = t.tail;
                col.decimals = t.decimals;
            }
            Err(e) => col.readonly = Some(e),
        }
        col
    }

    pub fn is_static(&self) -> bool {
        self.mode.is_none()
    }

    pub fn value(&self, row: usize) -> f64 {
        self.edits[row].unwrap_or(self.base[row])
    }

    pub fn text(&self, row: usize) -> String {
        track::format_value(self.value(row), self.decimals)
    }

    pub fn is_edited(&self, row: usize) -> bool {
        self.edits[row].is_some_and(|v| {
            track::format_value(v, self.decimals) != track::format_value(self.base[row], self.decimals)
        })
    }

    pub fn edited_count(&self) -> usize {
        if let Some(n) = self.edited_cache.get() {
            return n;
        }
        let n = (0..self.base.len()).filter(|&r| self.is_edited(r)).count();
        self.edited_cache.set(Some(n));
        n
    }

    /// 全部の点が同じ値（移動なしにできる）
    pub fn all_same(&self) -> bool {
        if let Some(same) = self.same_cache.get() {
            return same;
        }
        let first = self.text(0);
        let same = (1..self.base.len()).all(|r| self.text(r) == first);
        self.same_cache.set(Some(same));
        same
    }

    /// セルの値を変えたら、覚えておいた結果を捨てる
    fn values_changed(&self) {
        self.same_cache.set(None);
        self.edited_cache.set(None);
    }

    /// 移動なしの列で、移動方法を選んでいないのに点ごとに違う値になっている（既定の直線移動で書く）
    pub fn needs_mode(&self) -> bool {
        self.is_static() && self.mode_change.is_none() && !self.all_same()
    }

    /// 書くときの移動方法と設定。`None` は移動なし
    pub fn effective_mode(&self) -> Option<(String, Option<String>)> {
        let base = match &self.mode_change {
            Some(ModeChange::Static) => None,
            Some(ModeChange::Set(m, t)) => Some((m.clone(), t.clone())),
            None if self.needs_mode() => Some((DEFAULT_MODE.0.to_string(), Some(DEFAULT_MODE.1.to_string()))),
            None => self.mode.clone().map(|m| (m, self.tail.clone())),
        };
        base.map(|(m, t)| (m, self.apply_setting_edits(t)))
    }

    /// 設定（`数値|設定値1,設定値2,…`）に、書き換えた加減速などと設定値を入れる。
    /// 設定値を書き換えていなければ `|` の後ろはそのまま残す（時間制御の `8|…` など、数値でない形も壊さない）
    fn apply_setting_edits(&self, tail: Option<String>) -> Option<String> {
        if self.flags_edit.is_none() && self.param_edit.is_none() {
            return tail;
        }
        let t = tail.unwrap_or_else(|| "0".into());
        let (head, rest) = match t.split_once('|') {
            Some((h, r)) => (h.to_string(), Some(r.to_string())),
            None => (t.clone(), None),
        };
        let flags = self.flags_edit.unwrap_or_else(|| head.trim().parse().unwrap_or(0));
        Some(match (&self.param_edit, rest) {
            (Some(v), _) if !v.is_empty() => format!("{flags}|{}", format_params(v)),
            (Some(_), _) => flags.to_string(),
            (None, Some(r)) => format!("{flags}|{r}"),
            (None, None) => flags.to_string(),
        })
    }

    /// 移動方法か設定（加減速・設定値）を元から変えたか
    pub fn mode_changed(&self) -> bool {
        self.effective_mode() != self.mode.clone().map(|m| (m, self.tail.clone()))
    }

    /// 未反映の数（書き換えたセル＋移動方法・設定の変更）
    pub fn pending_count(&self) -> usize {
        self.edited_count() + usize::from(self.mode_changed())
    }

    /// 移動方法を選ぶ。元と同じものを選んだら変更を取り消す。別の移動方法にしたら、加減速と設定値の書き換えは捨てる
    pub fn choose_mode(&mut self, change: ModeChange) {
        let same = match &change {
            ModeChange::Static => self.mode.is_none(),
            ModeChange::Set(m, _) => self.mode.as_deref() == Some(m.as_str()),
        };
        let current = self.effective_mode().map(|(m, _)| m);
        let next = match &change {
            ModeChange::Static => None,
            ModeChange::Set(m, _) => Some(m.clone()),
        };
        if current != next {
            self.flags_edit = None;
            self.param_edit = None;
        }
        self.mode_change = if same { None } else { Some(change) };
    }

    /// 書くときの加速・減速・中間点無視（設定の `|` の前の数値）
    pub fn current_flags(&self) -> u32 {
        self.effective_mode().map(|(_, t)| track::flags_of(t.as_deref())).unwrap_or(0)
    }

    pub fn set_flags(&mut self, flags: u32) {
        self.flags_edit = Some(flags);
    }

    /// 移動方法を元のまま使っているか（設定値の今の値が `params` にある）
    pub fn same_mode(&self) -> bool {
        match &self.mode_change {
            None => true,
            Some(ModeChange::Static) => false,
            Some(ModeChange::Set(m, _)) => self.mode.as_deref() == Some(m.as_str()),
        }
    }

    /// 設定値を編集できるか。`defs` は書くときの移動方法の設定値の定義
    pub fn params_editable(&self, defs: &[ParamDef]) -> Result<(), String> {
        if self.readonly.is_some() || self.effective_mode().is_none() {
            return Err("移動なしには設定値がありません".into());
        }
        if defs.is_empty() {
            return Err("この移動方法には設定値がありません（分からない組み込みのものを含む）".into());
        }
        if self.same_mode() {
            // 読んだ設定の `|` の後ろが、定義と同じ数の数値の並びのときだけ（時間制御のデータなどを壊さない）
            let rest = self.tail.as_deref().and_then(|t| t.split_once('|')).map(|(_, r)| r).unwrap_or("");
            if !rest.is_empty() {
                let nums: Vec<Option<f64>> = rest.split(',').map(|p| p.trim().parse::<f64>().ok()).collect();
                if nums.iter().any(|n| n.is_none()) || nums.len() != defs.len() {
                    return Err(format!("設定値が定義（{} 個）と違う形で保存されています: {rest}", defs.len()));
                }
            }
            if !self.params.is_empty() && self.params.len() != defs.len() {
                return Err(format!("設定値の数（{}）が定義（{}）と合いません", self.params.len(), defs.len()));
            }
        }
        Ok(())
    }

    /// 設定値の今の値（書き換え ＞ 元の移動方法なら読んだ値 ＞ 定義の初期値）
    pub fn param_values(&self, defs: &[ParamDef]) -> Vec<f64> {
        if let Some(v) = &self.param_edit {
            return v.clone();
        }
        if self.same_mode() && self.params.len() == defs.len() {
            return self.params.clone();
        }
        defs.iter().map(|d| d.default).collect()
    }

    /// 設定値を書き換える（数は定義と同じにする。合わないと本体は初期値を使う。プローブ M7）
    pub fn set_params(&mut self, values: Vec<f64>) {
        self.param_edit = Some(values);
    }

    /// 移動方法の見出しのホバーに出す説明（設定値と加減速）
    pub fn mode_detail(&self) -> String {
        let mut parts = Vec::new();
        if let Some(m) = &self.mode {
            parts.push(m.clone());
        }
        if !self.params.is_empty() {
            let p: Vec<String> = self.params.iter().map(|v| track::format_value(*v, 2)).collect();
            parts.push(format!("設定値: {}", p.join(", ")));
        }
        if self.accelerate {
            parts.push("加速".into());
        }
        if self.decelerate {
            parts.push("減速".into());
        }
        if let Some(t) = &self.tail {
            parts.push(format!("（保存形式: {t}。値だけを変えたときはそのまま残します）"));
        }
        if self.mode_changed() || self.needs_mode() {
            match self.effective_mode() {
                Some((m, t)) => parts.push(format!("→ 反映すると「{m}」（設定 {}）", t.unwrap_or_default())),
                None => parts.push("→ 反映すると移動なし".into()),
            }
        }
        if parts.is_empty() {
            parts.push("移動なし".into());
        }
        parts.join("\n")
    }

    /// 始点と終点の 2 つだけで保存される（設定の中間点無視、または再生範囲）。書くときの移動方法で決まる
    pub fn twopoint(&self) -> bool {
        // 移動なしの列で移動方法を選んでいなければ、書くのは移動なしか既定の直線移動で、中間点無視を立てない限り 2 つにならない。
        // そのときは全部の点を比べない（`all_same`）。範囲をまとめて書き換えると 1 セルごとにここを通り、点の数の 2 乗になる
        if self.is_static() && self.mode_change.is_none() && self.flags_edit.is_none_or(|f| f & track::FLAG_TWOPOINT == 0) {
            return false;
        }
        self.effective_mode().is_some_and(|(m, t)| two_value(&m, t.as_deref()))
    }

    /// 中間点無視の列の途中の行（保存されない）
    pub fn is_ignored_row(&self, row: usize) -> bool {
        self.twopoint() && row > 0 && row + 1 < self.base.len()
    }

    pub fn editable(&self, row: usize) -> bool {
        self.readonly.is_none() && !self.is_ignored_row(row)
    }

    pub(crate) fn set(&mut self, row: usize, v: f64) {
        if self.editable(row) && v.is_finite() {
            self.edits[row] = Some(v);
            self.values_changed();
        }
    }

    /// 書く文字列。値の数は点の数に揃える
    pub fn output(&self) -> String {
        let n = self.base.len();
        let vals: Vec<f64> = (0..n).map(|r| self.value(r)).collect();
        match self.effective_mode() {
            None => track::format_value(vals[0], self.decimals),
            // 中間点無視は始点と終点だけ（本体も 2 つに切り詰めて保存する）
            Some((m, t)) if self.twopoint() => {
                track::build(&[vals[0], vals[n - 1]], self.decimals, Some(&m), t.as_deref())
            }
            Some((m, t)) => track::build(&vals, self.decimals, Some(&m), t.as_deref()),
        }
    }

    /// 周期複製で、読んだ値の全部の点に `delta` を足した文字列（未反映の書き換えは使わない）。
    /// 移動方法と設定は読んだまま。中間点無視・再生範囲は 2 つ、移動なしは 1 つで書く
    pub fn shifted(&self, delta: f64) -> String {
        let n = self.base.len();
        let vals: Vec<f64> = self.base.iter().map(|v| v + delta).collect();
        match &self.mode {
            None => track::format_value(vals[0], self.decimals),
            Some(m) if two_value(m, self.tail.as_deref()) => {
                track::build(&[vals[0], vals[n - 1]], self.decimals, Some(m), self.tail.as_deref())
            }
            Some(m) => track::build(&vals, self.decimals, Some(m), self.tail.as_deref()),
        }
    }

    pub fn discard(&mut self) {
        self.edits.iter_mut().for_each(|e| *e = None);
        self.values_changed();
        self.mode_change = None;
        self.flags_edit = None;
        self.param_edit = None;
    }
}

/// 設定値をカンマでつなぐ（整数はそのまま、小数は末尾の 0 を落とす）
pub fn format_params(values: &[f64]) -> String {
    values
        .iter()
        .map(|v| {
            if v.fract() == 0.0 {
                format!("{}", *v as i64)
            } else {
                let s = format!("{v:.4}");
                s.trim_end_matches('0').trim_end_matches('.').to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// 1 列分の書き込み
#[derive(Debug, Clone, PartialEq)]
pub struct WriteJob {
    pub effect: String,
    pub index: usize,
    pub item: String,
    pub expected_raw: String,
    pub new_raw: String,
}

impl WriteJob {
    pub fn label(&self) -> String {
        if self.index == 0 {
            format!("{} / {}", self.effect, self.item)
        } else {
            format!("{} #{} / {}", self.effect, self.index + 1, self.item)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub anchor: (usize, usize),
    pub end: (usize, usize),
}

impl Selection {
    pub fn single(cell: (usize, usize)) -> Self {
        Selection { anchor: cell, end: cell }
    }
    /// (行の範囲, 列の範囲)
    pub fn rect(&self) -> (std::ops::RangeInclusive<usize>, std::ops::RangeInclusive<usize>) {
        let (r0, r1) = (self.anchor.0.min(self.end.0), self.anchor.0.max(self.end.0));
        let (c0, c1) = (self.anchor.1.min(self.end.1), self.anchor.1.max(self.end.1));
        (r0..=r1, c0..=c1)
    }
    pub fn contains(&self, cell: (usize, usize)) -> bool {
        let (rs, cs) = self.rect();
        rs.contains(&cell.0) && cs.contains(&cell.1)
    }
}

/// 区間ごとのチェック（`--checksection`）。保存形式は区間ごとの `0` / `1` をカンマでつないだもの（`0,1,0`。プローブ M9）
#[derive(Debug, Clone, PartialEq)]
pub struct CheckColumn {
    pub effect: String,
    pub index: usize,
    pub item: String,
    pub raw: String,
    /// 区間ごとの値（区間の数 = 点の数 - 1）
    pub base: Vec<bool>,
    pub edits: Vec<Option<bool>>,
}

impl CheckColumn {
    /// 区間の数だけ `0` / `1` が並んでいるときだけ作る（それ以外の形は扱わない）
    pub fn new(effect: &str, index: usize, item: &str, raw: &str, sections: usize) -> Option<Self> {
        let base: Option<Vec<bool>> = raw
            .split(',')
            .map(|p| match p.trim() {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            })
            .collect();
        let base = base.filter(|b| b.len() == sections && sections >= 2)?;
        Some(CheckColumn {
            effect: effect.to_string(),
            index,
            item: item.to_string(),
            raw: raw.to_string(),
            edits: vec![None; base.len()],
            base,
        })
    }

    pub fn value(&self, section: usize) -> bool {
        self.edits[section].unwrap_or(self.base[section])
    }

    pub fn toggle(&mut self, section: usize) {
        if section < self.base.len() {
            let v = !self.value(section);
            self.edits[section] = if v == self.base[section] { None } else { Some(v) };
        }
    }

    pub fn is_edited(&self, section: usize) -> bool {
        self.edits[section].is_some_and(|v| v != self.base[section])
    }

    pub fn pending_count(&self) -> usize {
        (0..self.base.len()).filter(|&s| self.is_edited(s)).count()
    }

    pub fn output(&self) -> String {
        (0..self.base.len()).map(|s| if self.value(s) { "1" } else { "0" }).collect::<Vec<_>>().join(",")
    }
}

#[derive(Debug, Clone, Default)]
pub struct Table {
    /// 開始・中間点・終了のフレーム（シーン基準）
    pub frames: Vec<usize>,
    pub columns: Vec<Column>,
    /// 区間ごとのチェックの列（表の右に並べる。選択・ドラッグ・貼り付けの対象にしない）
    pub checks: Vec<CheckColumn>,
    pub selection: Option<Selection>,
}

impl Table {
    pub fn points(&self) -> usize {
        self.frames.len()
    }

    /// 未反映の数（書き換えたセル＋移動方法の変更＋区間ごとのチェック）
    pub fn pending_count(&self) -> usize {
        self.columns.iter().map(|c| c.pending_count()).sum::<usize>() + self.checks.iter().map(|c| c.pending_count()).sum::<usize>()
    }

    pub fn discard(&mut self) {
        self.columns.iter_mut().for_each(|c| c.discard());
        self.checks.iter_mut().for_each(|c| c.edits.iter_mut().for_each(|e| *e = None));
    }

    pub fn set_cell(&mut self, row: usize, col: usize, v: f64) {
        if let Some(c) = self.columns.get_mut(col) {
            if row < c.base.len() {
                c.set(row, v);
            }
        }
    }

    /// 選んだ範囲の編集できる列にだけ `f` を掛ける
    fn each_selected(&mut self, mut f: impl FnMut(&mut Column, std::ops::RangeInclusive<usize>)) -> usize {
        let Some(sel) = self.selection else { return 0 };
        let (rows, cols) = sel.rect();
        let mut n = 0;
        for c in cols {
            if let Some(col) = self.columns.get_mut(c) {
                if col.readonly.is_none() {
                    f(col, rows.clone());
                    n += 1;
                }
            }
        }
        n
    }

    pub fn add(&mut self, d: f64) -> usize {
        self.each_selected(|col, rows| rows.for_each(|r| col.set(r, col.value(r) + d)))
    }

    pub fn mul(&mut self, k: f64) -> usize {
        self.each_selected(|col, rows| rows.for_each(|r| col.set(r, col.value(r) * k)))
    }

    /// 列ごとに、選んだ行の最初と最後の値の間を等しい刻みで埋める
    pub fn interpolate(&mut self) -> usize {
        self.each_selected(|col, rows| {
            let (r0, r1) = (*rows.start(), *rows.end());
            if r1 <= r0 + 1 {
                return;
            }
            let (v0, v1) = (col.value(r0), col.value(r1));
            for r in r0 + 1..r1 {
                let t = (r - r0) as f64 / (r1 - r0) as f64;
                col.set(r, v0 + (v1 - v0) * t);
            }
        })
    }

    /// 選んだ範囲を、最初に選んだセルの値にする
    pub fn fill_with_anchor(&mut self) -> usize {
        let Some(sel) = self.selection else { return 0 };
        let Some(v) = self.columns.get(sel.anchor.1).map(|c| c.value(sel.anchor.0)) else { return 0 };
        self.each_selected(|col, rows| rows.for_each(|r| col.set(r, v)))
    }

    /// ドラッグで動かすセルと、動かし始めの値。選んだ範囲の中を掴んだら範囲の編集できるセル全部、外なら掴んだセルだけ
    pub fn drag_targets(&mut self, cell: (usize, usize)) -> Vec<((usize, usize), f64)> {
        let inside = self.selection.is_some_and(|s| s.contains(cell));
        if !inside {
            self.selection = Some(Selection::single(cell));
        }
        let (rows, cols) = self.selection.map(|s| s.rect()).unwrap_or((cell.0..=cell.0, cell.1..=cell.1));
        let mut out = Vec::new();
        for c in cols {
            let Some(col) = self.columns.get(c) else { continue };
            if col.readonly.is_some() {
                continue;
            }
            for r in rows.clone() {
                if r < col.base.len() {
                    out.push(((r, c), col.value(r)));
                }
            }
        }
        out
    }

    /// 矢印キーで選んだセルを動かす。`extend` なら範囲を広げる
    pub fn move_selection(&mut self, dr: isize, dc: isize, extend: bool) {
        if self.columns.is_empty() || self.points() == 0 {
            return;
        }
        let s = self.selection.unwrap_or(Selection::single((0, 0)));
        let clamp = |v: usize, d: isize, max: usize| (v as isize + d).clamp(0, max as isize - 1) as usize;
        let end = (clamp(s.end.0, dr, self.points()), clamp(s.end.1, dc, self.columns.len()));
        self.selection = Some(if extend { Selection { anchor: s.anchor, end } } else { Selection::single(end) });
    }

    /// 選んだ範囲をタブ区切りにする（表計算ソフトへ貼れる形）
    pub fn copy_tsv(&self) -> Option<String> {
        let sel = self.selection?;
        let (rows, cols) = sel.rect();
        let mut out = String::new();
        for r in rows {
            let line: Vec<String> = cols.clone().filter_map(|c| self.columns.get(c).map(|col| col.text(r))).collect();
            out.push_str(&line.join("\t"));
            out.push_str("\r\n");
        }
        Some(out)
    }

    /// タブ区切りを貼る。選んだのが 1 セルならそこを左上にして貼り、範囲なら大きさが同じときだけ貼る
    pub fn paste_tsv(&mut self, text: &str) -> Result<usize, String> {
        let sel = self.selection.ok_or("貼り付ける場所（セル）を選んでください")?;
        let mut data: Vec<Vec<f64>> = Vec::new();
        for (i, line) in text.split('\n').enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                continue;
            }
            let mut row = Vec::new();
            for cell in line.split('\t') {
                let cell = cell.trim();
                let v = cell
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| format!("{} 行目の「{cell}」が数値ではありません", i + 1))?;
                row.push(v);
            }
            data.push(row);
        }
        if data.is_empty() {
            return Err("クリップボードに値がありません".into());
        }
        let width = data[0].len();
        if data.iter().any(|r| r.len() != width) {
            return Err("行ごとに列の数が違います".into());
        }
        let (rows, cols) = sel.rect();
        let (r0, c0) = (*rows.start(), *cols.start());
        let single = rows.clone().count() == 1 && cols.clone().count() == 1;
        if !single && (rows.clone().count() != data.len() || cols.clone().count() != width) {
            return Err(format!(
                "貼り付ける値は {} 行 {} 列で、選んだ範囲（{} 行 {} 列）と合いません",
                data.len(),
                width,
                rows.count(),
                cols.count()
            ));
        }
        if r0 + data.len() > self.points() || c0 + width > self.columns.len() {
            return Err(format!("貼り付ける値（{} 行 {} 列）が表からはみ出します", data.len(), width));
        }
        if let Some(c) = (c0..c0 + width).find(|&c| self.columns[c].readonly.is_some()) {
            return Err(format!("「{}」は編集できない列です", self.columns[c].item));
        }
        for (dr, row) in data.iter().enumerate() {
            for (dc, v) in row.iter().enumerate() {
                self.columns[c0 + dc].set(r0 + dr, *v);
            }
        }
        self.selection = Some(Selection { anchor: (r0, c0), end: (r0 + data.len() - 1, c0 + width - 1) });
        Ok(data.len() * width)
    }

    /// この表の列が今使っている移動方法と設定（使われているので本体に登録されている）
    pub fn used_modes(&self) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        for c in &self.columns {
            if let Some(m) = &c.mode {
                if !out.iter().any(|(n, _)| n == m) {
                    out.push((m.clone(), c.tail.clone()));
                }
            }
        }
        out
    }

    /// 書き換えた列の書き込み
    pub fn write_jobs(&self) -> Vec<WriteJob> {
        self.columns
            .iter()
            .filter(|c| c.readonly.is_none() && c.pending_count() > 0)
            .map(|c| WriteJob {
                effect: c.effect.clone(),
                index: c.index,
                item: c.item.clone(),
                expected_raw: c.raw.clone(),
                new_raw: c.output(),
            })
            .chain(self.checks.iter().filter(|c| c.pending_count() > 0).map(|c| WriteJob {
                effect: c.effect.clone(),
                index: c.index,
                item: c.item.clone(),
                expected_raw: c.raw.clone(),
                new_raw: c.output(),
            }))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Table {
        let frames = vec![100, 130, 160, 189];
        let n = frames.len();
        Table {
            frames,
            columns: vec![
                Column::new("標準描画", 0, "X", "100.00,200.00,300.00,400.00,直線移動,0", n),
                Column::new("標準描画", 0, "Y", "0.00", n),
                Column::new("標準描画", 0, "拡大率", "100.000,100.000,100.000,100.000,直線移動,0", n),
                Column::new("何か", 0, "変", "1.0,2.0,3.0,4.0,", n),
            ],
            checks: vec![CheckColumn::new("プローブ区間チェック_H", 0, "反転", "0,0,0", n - 1).unwrap()],
            selection: None,
        }
    }

    #[test]
    fn shifted_for_duplicates() {
        let t = table();
        assert_eq!(t.columns[0].shifted(50.0), "150.00,250.00,350.00,450.00,直線移動,0");
        assert_eq!(t.columns[1].shifted(-1.5), "-1.50");
        let c = Column::new("標準描画", 0, "X", "100.00,400.00,直線移動,4", 4);
        assert_eq!(c.shifted(10.0), "110.00,410.00,直線移動,4");
        // 未反映の書き換えは使わない
        let mut t = table();
        t.set_cell(1, 0, 999.0);
        assert_eq!(t.columns[0].shifted(0.0), "100.00,200.00,300.00,400.00,直線移動,0");
    }

    #[test]
    fn check_columns() {
        // 区間の数と合わない・0/1 以外・区間が 1 つは扱わない
        assert!(CheckColumn::new("e", 0, "c", "0,1", 3).is_none());
        assert!(CheckColumn::new("e", 0, "c", "0,2,1", 3).is_none());
        assert!(CheckColumn::new("e", 0, "c", "0", 1).is_none());
        let mut t = table();
        t.checks[0].toggle(1);
        assert_eq!(t.pending_count(), 1);
        let jobs = t.write_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!((jobs[0].expected_raw.as_str(), jobs[0].new_raw.as_str()), ("0,0,0", "0,1,0"));
        // もう一度押すと元に戻り、未反映でなくなる
        t.checks[0].toggle(1);
        assert_eq!(t.pending_count(), 0);
        t.checks[0].toggle(2);
        t.discard();
        assert_eq!(t.pending_count(), 0);
    }

    #[test]
    fn columns_from_raw() {
        let t = table();
        assert_eq!(t.columns[0].base, vec![100.0, 200.0, 300.0, 400.0]);
        assert_eq!(t.columns[1].base, vec![0.0; 4]);
        assert!(t.columns[1].is_static());
        assert!(t.columns[3].readonly.is_some());
        // 値が足りない列（実機の T4 の形）は最後の値で埋め、注意を出す
        let c = Column::new("標準描画", 0, "X", "11.00,22.00,直線移動,0", 4);
        assert_eq!(c.base, vec![11.0, 22.0, 22.0, 22.0]);
        assert_eq!(c.notes.len(), 1);
        assert_eq!(c.output(), "11.00,22.00,22.00,22.00,直線移動,0");
    }

    #[test]
    fn edit_and_write_jobs() {
        let mut t = table();
        assert!(t.write_jobs().is_empty());
        t.set_cell(1, 0, 250.0);
        // 同じ値に戻したものは数えない
        t.set_cell(2, 0, 300.0);
        assert_eq!(t.pending_count(), 1);
        let jobs = t.write_jobs();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].expected_raw, "100.00,200.00,300.00,400.00,直線移動,0");
        assert_eq!(jobs[0].new_raw, "100.00,250.00,300.00,400.00,直線移動,0");
        t.discard();
        assert_eq!(t.pending_count(), 0);
    }

    #[test]
    fn twopoint_column() {
        // 中間点無視（設定 4）は値が 2 つで保存される。途中の行は編集できず、書くのは始点と終点だけ
        let mut c = Column::new("標準描画", 0, "X", "100.00,400.00,直線移動,4", 4);
        assert!(c.notes.is_empty());
        assert_eq!(c.base, vec![100.0, 200.0, 300.0, 400.0]);
        assert!(c.twopoint() && c.is_ignored_row(1) && !c.is_ignored_row(3));
        c.set(1, 999.0);
        assert_eq!(c.pending_count(), 0);
        c.set(3, 500.0);
        assert_eq!(c.output(), "100.00,500.00,直線移動,4");
        // 中間点無視でない移動方法に変えると、途中の行も書く
        c.choose_mode(ModeChange::Set("補間移動".into(), Some("0".into())));
        assert!(!c.twopoint());
        assert_eq!(c.output(), "100.00,200.00,300.00,500.00,補間移動,0");
        // 再生範囲は設定が 0 でも 2 つだけ
        c.choose_mode(ModeChange::Set("再生範囲".into(), Some("0".into())));
        assert!(c.twopoint());
        assert_eq!(c.output(), "100.00,500.00,再生範囲,0");
        let r = Column::new("標準描画", 0, "X", "0.00,10.00,再生範囲,0", 4);
        assert!(r.notes.is_empty() && r.is_ignored_row(1));
    }

    fn defs3() -> Vec<ParamDef> {
        use crate::movement::ParamKind;
        vec![
            ParamDef { label: "速さ".into(), kind: ParamKind::Number, default: 5.0 },
            ParamDef { label: "反転".into(), kind: ParamKind::Check, default: 1.0 },
            ParamDef { label: "種別".into(), kind: ParamKind::Select(vec![("A".into(), 1.0), ("B".into(), 2.0)]), default: 2.0 },
        ]
    }

    #[test]
    fn edit_params_and_flags() {
        // 本体の設定ダイアログで変えた形（中間点プローブ_H v0.2.3）
        let mut c = Column::new("標準描画", 0, "X", "100.00,200.00,300.00,400.00,プローブ移動_H,0|9,0,1", 4);
        c.params = vec![9.0, 0.0, 1.0];
        let d = defs3();
        assert!(c.params_editable(&d).is_ok());
        assert_eq!(c.param_values(&d), vec![9.0, 0.0, 1.0]);
        c.set_params(vec![7.5, 1.0, 2.0]);
        assert_eq!(c.output(), "100.00,200.00,300.00,400.00,プローブ移動_H,0|7.5,1,2");
        assert_eq!(c.pending_count(), 1);
        // 加速・減速
        c.set_flags(3);
        assert_eq!(c.output(), "100.00,200.00,300.00,400.00,プローブ移動_H,3|7.5,1,2");
        // 中間点無視を立てると 2 つで書く
        c.set_flags(4);
        assert_eq!(c.output(), "100.00,400.00,プローブ移動_H,4|7.5,1,2");
        // 元に戻せば未反映は無い
        c.set_flags(0);
        c.set_params(vec![9.0, 0.0, 1.0]);
        assert_eq!(c.pending_count(), 0);
        // 別の移動方法を選ぶと書き換えは捨て、新しい移動方法の初期値になる
        c.set_params(vec![1.0, 1.0, 1.0]);
        c.choose_mode(ModeChange::Set("ランダム移動".into(), Some("0".into())));
        assert!(c.param_edit.is_none());
        let rd = vec![ParamDef { label: "設定値1".into(), kind: crate::movement::ParamKind::Number, default: 15.0 }];
        assert_eq!(c.param_values(&rd), vec![15.0]);
        c.set_params(vec![300.0]);
        assert_eq!(c.output(), "100.00,200.00,300.00,400.00,ランダム移動,0|300");
    }

    #[test]
    fn params_not_editable_when_tail_is_not_numbers() {
        // 時間制御のデータなど、数値の並びでない設定は壊さないように編集させない
        let c = Column::new("標準描画", 0, "X", "0.00,1.00,移動無し,8|10^@min(音量/20-3,2)", 2);
        let one = vec![ParamDef { label: "a".into(), kind: crate::movement::ParamKind::Number, default: 0.0 }];
        assert!(c.params_editable(&one).is_err());
        // 数が定義と違う
        let c = Column::new("標準描画", 0, "X", "0.00,1.00,プローブ移動_H,0|7,0", 2);
        assert!(c.params_editable(&defs3()).is_err());
        // 加減速だけなら、`|` の後ろはそのまま残す
        let mut c = Column::new("標準描画", 0, "X", "0.00,1.00,移動無し,8|10^@min(音量/20-3,2)", 2);
        c.set_flags(9);
        assert_eq!(c.output(), "0.00,1.00,移動無し,9|10^@min(音量/20-3,2)");
        assert_eq!(format_params(&[1.0, 0.25, -3.0]), "1,0.25,-3");
    }

    #[test]
    fn change_mode_of_moving_column() {
        let mut t = table();
        // 移動方法だけを変えても 1 件の未反映になる。設定は選んだものになる
        t.columns[0].choose_mode(ModeChange::Set("反復移動".into(), Some("0|15".into())));
        assert_eq!(t.pending_count(), 1);
        assert_eq!(t.write_jobs()[0].new_raw, "100.00,200.00,300.00,400.00,反復移動,0|15");
        // 元と同じ移動方法を選び直すと取り消し
        t.columns[0].choose_mode(ModeChange::Set("直線移動".into(), Some("0".into())));
        assert_eq!(t.pending_count(), 0);
        // 値が点ごとに違う列は移動なしにしても最初の値になる（UI では全部同じときだけ選べる）
        t.columns[2].choose_mode(ModeChange::Static);
        assert!(t.columns[2].all_same());
        assert_eq!(t.columns[2].output(), "100.000");
        assert_eq!(t.pending_count(), 1);
    }

    #[test]
    fn static_column_needs_mode() {
        let mut t = table();
        // 全部同じ値なら移動なしのまま 1 つで書く
        t.selection = Some(Selection { anchor: (0, 1), end: (3, 1) });
        t.add(5.0);
        assert!(!t.columns[1].needs_mode());
        assert_eq!(t.columns[1].output(), "5.00");
        // 点ごとに違えば既定の直線移動で書く
        t.set_cell(3, 1, 50.0);
        assert!(t.columns[1].needs_mode());
        assert_eq!(t.columns[1].output(), "5.00,5.00,5.00,50.00,直線移動,0");
        // 書き換えたセル 4 つ＋移動方法（既定の直線移動になる）1 つ
        assert_eq!(t.columns[1].pending_count(), 5);
        t.columns[1].choose_mode(ModeChange::Set("4次式@Basic_S".into(), Some("0".into())));
        assert_eq!(t.columns[1].output(), "5.00,5.00,5.00,50.00,4次式@Basic_S,0");
    }

    #[test]
    fn range_operations() {
        let mut t = table();
        t.selection = Some(Selection { anchor: (0, 0), end: (3, 0) });
        t.mul(2.0);
        assert_eq!((0..4).map(|r| t.columns[0].value(r)).collect::<Vec<_>>(), vec![200.0, 400.0, 600.0, 800.0]);
        t.set_cell(1, 0, 0.0);
        t.interpolate();
        assert_eq!((0..4).map(|r| t.columns[0].text(r)).collect::<Vec<_>>(), vec!["200.00", "400.00", "600.00", "800.00"]);
        t.selection = Some(Selection { anchor: (2, 0), end: (0, 0) });
        t.fill_with_anchor();
        assert_eq!((0..3).map(|r| t.columns[0].value(r)).collect::<Vec<_>>(), vec![600.0; 3]);
        // 読むだけの列は変わらない
        t.selection = Some(Selection { anchor: (0, 2), end: (3, 3) });
        assert_eq!(t.add(1.0), 1);
        assert_eq!(t.columns[3].edited_count(), 0);
    }

    #[test]
    fn copy_and_paste() {
        let mut t = table();
        t.selection = Some(Selection { anchor: (0, 0), end: (1, 1) });
        assert_eq!(t.copy_tsv().unwrap(), "100.00\t0.00\r\n200.00\t0.00\r\n");
        // 1 セルを選んでいれば、そこから広げて貼る
        t.selection = Some(Selection::single((2, 0)));
        assert_eq!(t.paste_tsv("1\t2\r\n3\t4\r\n"), Ok(4));
        assert_eq!(t.columns[0].value(3), 3.0);
        assert_eq!(t.columns[1].value(2), 2.0);
        assert_eq!(t.selection, Some(Selection { anchor: (2, 0), end: (3, 1) }));
        // 大きさが合わない・はみ出す・数値でない・読むだけの列
        t.selection = Some(Selection { anchor: (0, 0), end: (2, 0) });
        assert!(t.paste_tsv("1\n2\n").is_err());
        t.selection = Some(Selection::single((3, 0)));
        assert!(t.paste_tsv("1\n2\n").is_err());
        t.selection = Some(Selection::single((0, 0)));
        assert!(t.paste_tsv("1\tx\n").is_err());
        t.selection = Some(Selection::single((0, 3)));
        assert!(t.paste_tsv("1\n").is_err());
    }

    #[test]
    fn drag_targets_and_arrow_keys() {
        let mut t = table();
        // 範囲の外を掴むとそのセルだけ
        t.selection = Some(Selection { anchor: (0, 0), end: (1, 0) });
        let d = t.drag_targets((3, 2));
        assert_eq!(d, vec![((3, 2), 100.0)]);
        assert_eq!(t.selection, Some(Selection::single((3, 2))));
        // 範囲の中を掴むと範囲全部（読むだけの列は除く）
        t.selection = Some(Selection { anchor: (0, 2), end: (1, 3) });
        let d = t.drag_targets((1, 2));
        assert_eq!(d.iter().map(|(c, _)| *c).collect::<Vec<_>>(), vec![(0, 2), (1, 2)]);
        // 矢印キー
        t.selection = Some(Selection::single((0, 0)));
        t.move_selection(-1, -1, false);
        assert_eq!(t.selection, Some(Selection::single((0, 0))));
        t.move_selection(2, 1, true);
        assert_eq!(t.selection, Some(Selection { anchor: (0, 0), end: (2, 1) }));
        t.move_selection(9, 9, false);
        assert_eq!(t.selection, Some(Selection::single((3, 3))));
    }

    #[test]
    fn static_column_with_twopoint_flag() {
        // 移動なしの列でも、点ごとに違う値にして中間点無視を立てれば、途中の行は保存されない
        let mut t = table();
        t.set_cell(3, 1, 50.0);
        assert!(!t.columns[1].twopoint());
        t.columns[1].set_flags(track::FLAG_TWOPOINT);
        assert!(t.columns[1].twopoint() && t.columns[1].is_ignored_row(1));
        assert_eq!(t.columns[1].output(), "0.00,50.00,直線移動,4");
        // 全部同じ値に戻すと移動なしで、中間点無視も効かない
        t.set_cell(3, 1, 0.0);
        assert!(!t.columns[1].twopoint());
        assert_eq!(t.columns[1].output(), "0.00");
    }

    #[test]
    fn used_modes_are_unique() {
        let t = table();
        assert_eq!(t.used_modes(), vec![("直線移動".to_string(), Some("0".to_string()))]);
    }
}
