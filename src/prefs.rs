//! 列の表示の設定を覚える。どのオブジェクトでも同じ効果・項目に効く。
//!
//! - 隠す列（効果名と項目名の組）。隠した列は表を作るときに読み込まない（選択・貼り付け・ドラッグの対象に混ざらないように）
//! - 列の並び順（効果ごとの項目名の順）。並べたことのない項目は、並べた項目の後ろに元の順で付く
//! - 自動で読み直すか（v0.5.0）
//!
//! 保存先は `Plugin/MidpointTable_H/hidden_columns.txt`。UTF-8・LF。1 行に 1 つ:
//! - `効果名<TAB>項目名` … 隠す列（v0.3.0 からの形）
//! - `@order<TAB>効果名<TAB>項目1<TAB>項目2…` … 並び順（v0.4.0）
//! - `@auto<TAB>0` … 自動で読み直さない（v0.5.0。無ければ読み直す）

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const ORDER: &str = "@order";
const AUTO: &str = "@auto";

#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    pub hidden: BTreeSet<(String, String)>,
    pub order: BTreeMap<String, Vec<String>>,
    /// 本体の変化に合わせて表を読み直すか
    pub auto_reload: bool,
    path: Option<PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { hidden: BTreeSet::new(), order: BTreeMap::new(), auto_reload: true, path: None }
    }
}

impl Prefs {
    pub fn parse(text: &str) -> (BTreeSet<(String, String)>, BTreeMap<String, Vec<String>>) {
        let (mut hidden, mut order) = (BTreeSet::new(), BTreeMap::new());
        for line in text.lines() {
            let parts: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
            match parts.as_slice() {
                [ORDER, effect, items @ ..] if !effect.is_empty() && !items.is_empty() => {
                    order.insert(effect.to_string(), items.iter().filter(|i| !i.is_empty()).map(|i| i.to_string()).collect());
                }
                [e, i] if !e.is_empty() && !i.is_empty() && *e != ORDER && *e != AUTO => {
                    hidden.insert((e.to_string(), i.to_string()));
                }
                _ => {}
            }
        }
        (hidden, order)
    }

    /// 自動で読み直すか（`@auto<TAB>0` の行があれば読み直さない）
    pub fn parse_auto(text: &str) -> bool {
        !text.lines().any(|l| l.trim_end_matches('\r') == format!("{AUTO}\t0"))
    }

    pub fn to_text(hidden: &BTreeSet<(String, String)>, order: &BTreeMap<String, Vec<String>>, auto_reload: bool) -> String {
        let mut s: String = hidden.iter().map(|(e, i)| format!("{e}\t{i}\n")).collect();
        for (e, items) in order {
            s.push_str(&format!("{ORDER}\t{e}\t{}\n", items.join("\t")));
        }
        if !auto_reload {
            s.push_str(&format!("{AUTO}\t0\n"));
        }
        s
    }

    /// 読む。無ければ空（自動で読み直す）
    pub fn load(path: &Path) -> Self {
        let text = std::fs::read(path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        let (hidden, order) = Self::parse(&text);
        Prefs { hidden, order, auto_reload: Self::parse_auto(&text), path: Some(path.to_path_buf()) }
    }

    pub fn set_auto_reload(&mut self, on: bool) {
        self.auto_reload = on;
        self.save();
    }

    pub fn set_hidden(&mut self, effect: &str, item: &str, hide: bool) {
        let key = (effect.to_string(), item.to_string());
        if hide {
            self.hidden.insert(key);
        } else {
            self.hidden.remove(&key);
        }
        self.save();
    }

    /// 効果の項目の並び順を覚える
    pub fn set_order(&mut self, effect: &str, items: Vec<String>) {
        self.order.insert(effect.to_string(), items);
        self.save();
    }

    /// 並び順を忘れる（元の順に戻す）
    pub fn clear_order(&mut self, effect: &str) {
        self.order.remove(effect);
        self.save();
    }

    /// `items`（元の順）を、覚えた並び順に並べ替えたときの添字の列
    pub fn sorted_indices(&self, effect: &str, items: &[&str]) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..items.len()).collect();
        if let Some(order) = self.order.get(effect) {
            idx.sort_by_key(|&i| (order.iter().position(|o| o == items[i]).unwrap_or(usize::MAX), i));
        }
        idx
    }

    fn save(&self) {
        let Some(p) = &self.path else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(p, Self::to_text(&self.hidden, &self.order, self.auto_reload)) {
            tracing::warn!("[MidpointTable_H] 列の設定を保存できません: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut h = BTreeSet::new();
        h.insert(("標準描画".to_string(), "中心X".to_string()));
        h.insert(("ぼかし".to_string(), "範囲".to_string()));
        let mut o = BTreeMap::new();
        o.insert("標準描画".to_string(), vec!["拡大率".to_string(), "X".to_string()]);
        let t = Prefs::to_text(&h, &o, true);
        assert_eq!(Prefs::parse(&t), (h.clone(), o.clone()));
        assert!(Prefs::parse_auto(&t));
        // 自動で読み直さない設定は 1 行足すだけで、隠す列・並び順には混ざらない
        let t = Prefs::to_text(&h, &o, false);
        assert_eq!(Prefs::parse(&t), (h, o));
        assert!(!Prefs::parse_auto(&t));
        assert!(!Prefs::parse_auto("@auto\t0\r\n"));
        assert!(Prefs::parse_auto("@auto\t1\n"));
        // 壊れた行は読み飛ばす。v0.3.0 の形（隠す列だけ）はそのまま読める
        let (h, o) = Prefs::parse("abc\n\tx\ny\t\n標準描画\t中心X\r\n@order\t\n");
        assert_eq!((h.len(), o.len()), (1, 0));
    }

    #[test]
    fn sort_by_order() {
        let mut p = Prefs::default();
        p.order.insert("標準描画".into(), vec!["拡大率".into(), "Y".into()]);
        // 並べた項目（拡大率・Y）が先、残りは元の順（X・Z）
        assert_eq!(p.sorted_indices("標準描画", &["X", "Y", "Z", "拡大率"]), vec![3, 1, 0, 2]);
        assert_eq!(p.sorted_indices("ぼかし", &["範囲", "縦横比"]), vec![0, 1]);
    }
}
