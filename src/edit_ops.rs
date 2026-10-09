//! 本体との読み書き。
//!
//! - 読み取りは `call_read_section`（本体の Undo に触れない）。表の監視もこれだけを使う
//! - 書き込みは「反映」を押したときの `call_edit_section` 1 回だけ（ルール au2-rs-plugin。
//!   フォーカスの変化・タイマーから呼ぶと、操作中の本体の Undo を捨てる）

use aviutl2::generic::{EditSection, ObjectHandle, ReadSection};

use crate::alias;
use crate::prefs::Prefs;
use crate::table::{CheckColumn, Column, Table, WriteJob};
use crate::EDIT_HANDLE;

/// 表を作ったときのオブジェクト
pub struct Loaded {
    pub handle: ObjectHandle,
    /// 作ったときのエイリアス。監視で変化を見るのに使う
    pub alias_raw: String,
    pub label: String,
    pub table: Table,
    /// 表全体への注意
    pub notes: Vec<String>,
    /// このオブジェクトにあって、隠しているので読み込まなかった列（効果名・項目名）
    pub hidden_here: Vec<(String, String)>,
    /// オブジェクトのレイヤー（0 始まり）
    pub layer: usize,
}

/// 監視で取るもの
pub struct Poll {
    pub focused: Option<ObjectHandle>,
    /// 表のオブジェクトの今のエイリアス。消えていれば `None`
    pub bound_alias: Option<String>,
    /// 本体で選んでいる区間（フォーカス中のオブジェクトの）
    pub focus_section: Option<usize>,
}

fn ready() -> Result<(), String> {
    if EDIT_HANDLE.is_ready() {
        Ok(())
    } else {
        Err("本体の準備ができていません".into())
    }
}

/// 監視。起動直後で本体の準備ができていなければ `Ok(None)`（警告にしない）
pub fn poll(bound: Option<ObjectHandle>) -> Result<Option<Poll>, String> {
    if !EDIT_HANDLE.is_ready() {
        return Ok(None);
    }
    EDIT_HANDLE
        .call_read_section(move |read| {
            let focused = read.get_focused_object().ok().flatten();
            let bound_alias = bound.filter(|h| read.object_exists(*h)).and_then(|h| read.get_object_alias(h).ok());
            let focus_section = read.get_focus_object_section().ok().flatten();
            Some(Poll { focused, bound_alias, focus_section })
        })
        .map_err(|e| format!("読み取りに失敗しました: {e}"))
}

pub fn load(handle: ObjectHandle, prefs: Prefs) -> Result<Loaded, String> {
    ready()?;
    EDIT_HANDLE
        .call_read_section(move |read| load_in(read, handle, &prefs))
        .map_err(|e| format!("読み取りに失敗しました: {e}"))?
}

fn load_in(read: &ReadSection, handle: ObjectHandle, prefs: &Prefs) -> Result<Loaded, String> {
    let hidden = &prefs.hidden;
    let alias_raw = read.get_object_alias(handle).map_err(|e| format!("エイリアスを取れません: {e}"))?;
    let parsed = alias::parse(&alias_raw);
    let mut notes = Vec::new();
    let frames = parsed.frames.clone();
    if frames.len() < 2 {
        return Err(format!("オブジェクトの範囲が読めません（frame={:?}）", parsed.frames));
    }
    match read.get_object_section_num(handle) {
        Ok(n) if n + 1 != frames.len() => {
            notes.push(format!("区間の数（{n}）と点の数（{}）が合いません", frames.len()));
        }
        _ => {}
    }

    let points = frames.len();
    let mut columns = Vec::new();
    let mut checks = Vec::new();
    let mut hidden_here = Vec::new();
    for effect in &parsed.effects {
        let first_col = columns.len();
        for (key, value) in &effect.items {
            // トラックバーかどうかは本体に聞く（チェックボックスや文字列の項目は失敗する）
            let Ok(info) = read.get_object_track_info(handle, &effect.name, effect.index, key) else {
                // 区間ごとのチェック（`0,1,0` のように区間の数だけ並ぶ。プローブ M9）。区間が 1 つだと普通のチェックと見分けがつかないので出さない
                if points > 2
                    && !hidden.contains(&(effect.name.clone(), key.clone()))
                    && read.get_object_check_value(handle, &effect.name, effect.index, key, frames[0]).is_ok()
                {
                    if let Some(c) = CheckColumn::new(&effect.name, effect.index, key, value, points - 1) {
                        checks.push(c);
                    }
                }
                continue;
            };
            if hidden.contains(&(effect.name.clone(), key.clone())) {
                let k = (effect.name.clone(), key.clone());
                if !hidden_here.contains(&k) {
                    hidden_here.push(k);
                }
                continue;
            }
            let mut col = Column::new(&effect.name, effect.index, key, value, points);
            if info.twopoint {
                col.notes.push("中間点無視が有効（始点と終点だけが効く）".into());
            }
            if info.timecontrol {
                col.notes.push("時間制御が有効".into());
            }
            col.params = info.params.clone();
            col.accelerate = info.accelerate;
            col.decelerate = info.decelerate;
            columns.push(col);
        }
        // 覚えた並び順に並べ替える（効果の中だけ）
        let mine: Vec<Column> = columns.drain(first_col..).collect();
        let names: Vec<&str> = mine.iter().map(|c| c.item.as_str()).collect();
        let order = prefs.sorted_indices(&effect.name, &names);
        let mut mine: Vec<Option<Column>> = mine.into_iter().map(Some).collect();
        columns.extend(order.into_iter().filter_map(|i| mine[i].take()));
    }

    let lf = read.get_object_layer_frame(handle).map_err(|e| format!("位置を取れません: {e}"))?;
    let name = read
        .get_object_name(handle)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .or_else(|| parsed.effects.first().map(|e| e.name.clone()))
        .unwrap_or_default();
    let label = format!("L{} 「{}」 {}-{}  中間点 {} 個", lf.layer + 1, name, lf.start, lf.end, points - 2);

    Ok(Loaded { handle, alias_raw, label, table: Table { frames, columns, checks, selection: None }, notes, hidden_here, layer: lf.layer })
}

/// 本体の今のフレーム（シーン基準）
pub fn current_frame() -> Option<usize> {
    EDIT_HANDLE.is_ready().then(|| EDIT_HANDLE.get_edit_info().frame)
}

/// 中間点の操作（すぐに本体へ書く。1 回が本体の Undo 1 回分）。
/// 実機の結果（中間点プローブ_H v0.3.0 の M8）: 足した点にはその時点の値が入り、動きは変わらない。
/// 移動では値は点と一緒に動く。削除ではその点の値も消える。`section` は表の行番号（0 は開始点）
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Structure {
    Add { frame: usize },
    /// 中間点（行 1〜終了の 1 つ前）だけ。開始点を動かすとオブジェクトの開始位置が動く
    Move { row: usize, frame: usize },
    Delete { row: usize },
}

pub fn change_structure(handle: ObjectHandle, op: Structure) -> Result<String, String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| -> Result<String, String> {
            if !edit.object_exists(handle) {
                return Err("対象のオブジェクトがありません".into());
            }
            let lf = edit.get_object_layer_frame(handle).map_err(|e| e.to_string())?;
            let frames = edit.get_object_section_frames(handle).map_err(|e| e.to_string())?;
            let n = frames.len(); // 区間の数。行（点）は n + 1 個で、最後の行は lf.end
            match op {
                Structure::Add { frame } => {
                    if frame <= lf.start || frame >= lf.end {
                        return Err(format!("フレーム {frame} はオブジェクトの中（{}〜{}）ではありません", lf.start + 1, lf.end - 1));
                    }
                    if frames.contains(&frame) {
                        return Err(format!("フレーム {frame} には既に中間点があります"));
                    }
                    edit.create_object_section(handle, frame).map_err(|e| format!("中間点を足せませんでした: {e}"))?;
                    Ok(format!("フレーム {frame} に中間点を足しました"))
                }
                Structure::Move { row, frame } => {
                    if row == 0 || row >= n {
                        return Err("動かせるのは中間点だけです（開始点・終了点はタイムラインで動かしてください）".into());
                    }
                    let (lo, hi) = (frames[row - 1], if row + 1 < n { frames[row + 1] } else { lf.end });
                    if frame <= lo || frame >= hi {
                        return Err(format!("フレーム {frame} へは動かせません（前後の点のあいだ {}〜{} だけ）", lo + 1, hi - 1));
                    }
                    edit.move_object_section(handle, row, frame).map_err(|e| format!("中間点を動かせませんでした: {e}"))?;
                    Ok(format!("中間点を {} から {frame} へ動かしました", frames[row]))
                }
                Structure::Delete { row } => {
                    if row == 0 || row >= n {
                        return Err("消せるのは中間点だけです".into());
                    }
                    edit.delete_object_section(handle, row).map_err(|e| format!("中間点を消せませんでした: {e}"))?;
                    Ok(format!("フレーム {} の中間点を消しました", frames[row]))
                }
            }
        })
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))?
}

/// 周期複製の置き場所（レイヤー、開始フレーム）。`below` なら 1 つごとに 1 レイヤー下、そうでなければ同じレイヤー。
/// どちらも開始フレームは `interval` ずつずらす。同じレイヤーで間隔が長さより短いと重なるので失敗
pub fn placements(layer: usize, start: usize, len: usize, interval: usize, count: usize, below: bool) -> Result<Vec<(usize, usize)>, String> {
    if count == 0 || count > 100 {
        return Err("回数は 1〜100 にしてください".into());
    }
    if !below && interval < len {
        return Err(format!("同じレイヤーに置くときは、間隔をオブジェクトの長さ（{len} フレーム）以上にしてください"));
    }
    Ok((1..=count).map(|k| (if below { layer + k } else { layer }, start + k * interval)).collect())
}

/// 周期複製。元のオブジェクトをエイリアスごと写し（中間点・効果・設定を含む）、`shifts` の項目は k 個目に `raws[k-1]` を書く。
/// 置き場所が 1 つでも埋まっていれば何も作らない。1 回の `call_edit_section` で作るので、本体の Undo 1 回分になる想定
pub fn duplicate(
    handle: ObjectHandle,
    interval: usize,
    count: usize,
    below: bool,
    shifts: Vec<(String, usize, String, Vec<String>)>,
) -> Result<String, String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| -> Result<String, String> {
            if !edit.object_exists(handle) {
                return Err("対象のオブジェクトがありません".into());
            }
            let lf = edit.get_object_layer_frame(handle).map_err(|e| e.to_string())?;
            let len = lf.end - lf.start + 1;
            let places = placements(lf.layer, lf.start, len, interval, count, below)?;
            // 置き場所が空いているか（作り始める前に全部確かめる。本体は長さ 0 だと重なっても位置をずらして置く）
            for &(layer, frame) in &places {
                let end = frame + len - 1;
                let mut from = 0;
                for _ in 0..10_000 {
                    let Ok(Some(o)) = edit.find_object_after(layer, from) else { break };
                    let Ok(olf) = edit.get_object_layer_frame(o) else { break };
                    if olf.start <= end && olf.end >= frame {
                        return Err(format!(
                            "{} レイヤー目の {frame}〜{end} に別のオブジェクト（{}〜{}）があります。何も作りませんでした",
                            layer + 1,
                            olf.start,
                            olf.end
                        ));
                    }
                    if olf.start > end {
                        break;
                    }
                    from = olf.end + 1;
                }
            }
            let alias = edit.get_object_alias(handle).map_err(|e| format!("エイリアスを取れません: {e}"))?;
            let mut made = 0;
            let mut warnings = Vec::new();
            for (k, &(layer, frame)) in places.iter().enumerate() {
                let copy = edit.create_object_from_alias(&alias, layer, frame, len).map_err(|e| {
                    format!("{} 個目（{} レイヤー目・{frame}）を作れませんでした: {e}。それまでの {made} 個は作りました", k + 1, layer + 1)
                })?;
                made += 1;
                for (effect, index, item, raws) in &shifts {
                    let Some(raw) = raws.get(k) else { continue };
                    let ok = edit.set_object_effect_item(copy, effect, *index, item, raw).is_ok()
                        && edit.get_object_effect_item(copy, effect, *index, item).ok().as_deref() == Some(raw.as_str());
                    if !ok {
                        warnings.push(format!("{} 個目の {effect} / {item}", k + 1));
                    }
                }
            }
            let mut msg = format!("{made} 個を複製しました");
            if !warnings.is_empty() {
                msg.push_str(&format!("（値が思ったとおりに入らなかったもの: {}。範囲の外の値は本体が収めます）", warnings.join("、")));
            }
            Ok(msg)
        })
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::placements;

    #[test]
    fn duplicate_placements() {
        // 同じレイヤー: 長さ 90 のオブジェクトを 120 ごとに 3 つ
        assert_eq!(placements(2, 100, 90, 120, 3, false), Ok(vec![(2, 220), (2, 340), (2, 460)]));
        // 下のレイヤー: 間隔 0 なら同じ時刻に積む
        assert_eq!(placements(2, 100, 90, 0, 2, true), Ok(vec![(3, 100), (4, 100)]));
        // 同じレイヤーで重なる・回数の範囲外
        assert!(placements(2, 100, 90, 60, 3, false).is_err());
        assert!(placements(2, 100, 90, 120, 0, false).is_err());
        assert!(placements(2, 100, 90, 120, 101, false).is_err());
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Written,
    /// 表を作った後に本体側で変わっていたので書かなかった
    Stale(String),
    /// 書いた後に読み返した値が違う
    Mismatch(String),
    Failed(String),
}

/// 書き換えた列をまとめて書く。本体の Undo 1 回分になる想定（仕様の「未確認」の 1）
pub fn apply(handle: ObjectHandle, jobs: Vec<WriteJob>) -> Result<Vec<(WriteJob, Outcome)>, String> {
    ready()?;
    EDIT_HANDLE
        .call_edit_section(move |edit| apply_in(edit, handle, jobs))
        .map_err(|e| format!("書き込みを始められませんでした: {e}"))?
}

fn apply_in(edit: &mut EditSection, handle: ObjectHandle, jobs: Vec<WriteJob>) -> Result<Vec<(WriteJob, Outcome)>, String> {
    if !edit.object_exists(handle) {
        return Err("対象のオブジェクトがありません（削除された可能性があります）".into());
    }
    let mut out = Vec::new();
    for job in jobs {
        let outcome = match edit.get_object_effect_item(handle, &job.effect, job.index, &job.item) {
            Err(e) => Outcome::Failed(format!("読めません: {e}")),
            Ok(current) if current != job.expected_raw => Outcome::Stale(current),
            Ok(_) => match edit.set_object_effect_item(handle, &job.effect, job.index, &job.item, &job.new_raw) {
                Err(e) => Outcome::Failed(format!("書けません: {e}")),
                Ok(()) => match edit.get_object_effect_item(handle, &job.effect, job.index, &job.item) {
                    Ok(back) if back == job.new_raw => Outcome::Written,
                    Ok(back) => Outcome::Mismatch(back),
                    Err(e) => Outcome::Failed(format!("読み返せません: {e}")),
                },
            },
        };
        match &outcome {
            Outcome::Written => {}
            o => tracing::warn!("[MidpointTable_H] {}: {:?}", job.label(), o),
        }
        out.push((job, outcome));
    }
    Ok(out)
}
