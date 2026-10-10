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
//!
//! 下位互換（ルール au2-plugin-checklist「設定ファイルの下位互換」）:
//! - 読めない行（新しい版が書いた行など）は覚えておき、保存するときにそのまま後ろに付けて書き戻す
//! - ファイルが読めない（無い以外の I/O エラー・UTF-8 でない）ときは `hidden_columns.txt.broken-{秒}` へ退避して初期値で動く。
//!   退避もできなければ、その起動の間は保存しない（読めなかったファイルを初期値で上書きしない）
//! - 保存は一時ファイルに書いてから置き換える

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
    /// 読めなかった行（新しい版が書いた行など）。保存するときにそのまま書き戻す
    unknown: Vec<String>,
    path: Option<PathBuf>,
    /// ファイルが読めず、退避もできなかった。この起動の間は保存しない
    save_blocked: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            hidden: BTreeSet::new(),
            order: BTreeMap::new(),
            auto_reload: true,
            unknown: Vec::new(),
            path: None,
            save_blocked: false,
        }
    }
}

/// 1 行の読み方
#[derive(Debug, PartialEq)]
enum Line {
    Empty,
    Hidden(String, String),
    Order(String, Vec<String>),
    /// `@auto<TAB>値`。`0` なら読み直さない
    Auto(bool),
    /// 知らない行（`\r` を除いたそのまま）
    Unknown(String),
}

fn classify(raw: &str) -> Line {
    let line = raw.trim_end_matches('\r');
    if line.is_empty() {
        return Line::Empty;
    }
    let parts: Vec<&str> = line.split('\t').collect();
    match parts.as_slice() {
        [ORDER, effect, items @ ..] if !effect.is_empty() && !items.is_empty() => {
            Line::Order(effect.to_string(), items.iter().filter(|i| !i.is_empty()).map(|i| i.to_string()).collect())
        }
        [AUTO, v] => Line::Auto(*v != "0"),
        [e, i] if !e.is_empty() && !i.is_empty() && *e != ORDER => Line::Hidden(e.to_string(), i.to_string()),
        _ => Line::Unknown(line.to_string()),
    }
}

impl Prefs {
    pub fn parse(text: &str) -> (BTreeSet<(String, String)>, BTreeMap<String, Vec<String>>) {
        let (mut hidden, mut order) = (BTreeSet::new(), BTreeMap::new());
        for line in text.lines() {
            match classify(line) {
                Line::Order(e, items) => {
                    order.insert(e, items);
                }
                Line::Hidden(e, i) => {
                    hidden.insert((e, i));
                }
                _ => {}
            }
        }
        (hidden, order)
    }

    /// 自動で読み直すか（`@auto<TAB>0` の行があれば読み直さない）
    pub fn parse_auto(text: &str) -> bool {
        !text.lines().any(|l| classify(l) == Line::Auto(false))
    }

    /// 知らない行（新しい版が書いた行など）。読んだ順のまま
    fn parse_unknown(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|l| match classify(l) {
                Line::Unknown(s) => Some(s),
                _ => None,
            })
            .collect()
    }

    pub fn to_text(
        hidden: &BTreeSet<(String, String)>,
        order: &BTreeMap<String, Vec<String>>,
        auto_reload: bool,
        unknown: &[String],
    ) -> String {
        let mut s: String = hidden.iter().map(|(e, i)| format!("{e}\t{i}\n")).collect();
        for (e, items) in order {
            s.push_str(&format!("{ORDER}\t{e}\t{}\n", items.join("\t")));
        }
        if !auto_reload {
            s.push_str(&format!("{AUTO}\t0\n"));
        }
        for l in unknown {
            s.push_str(l);
            s.push('\n');
        }
        s
    }

    /// 読む。無ければ空（自動で読み直す）。読めなければ退避して空（退避できなければ保存しない）
    pub fn load(path: &Path) -> Self {
        let mut p = Prefs { path: Some(path.to_path_buf()), ..Default::default() };
        let text = match std::fs::read(path) {
            Ok(bytes) => String::from_utf8(bytes).map_err(|e| format!("UTF-8 ではありません（{e}）")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return p,
            Err(e) => Err(e.to_string()),
        };
        match text {
            Ok(text) => {
                (p.hidden, p.order) = Self::parse(&text);
                p.auto_reload = Self::parse_auto(&text);
                p.unknown = Self::parse_unknown(&text);
            }
            Err(e) => match backup_broken(path) {
                Ok(dest) => tracing::warn!("[MidpointTable_H] 列の設定を読めないので {} へ退避し、初期値にします: {e}", dest.display()),
                Err(be) => {
                    p.save_blocked = true;
                    tracing::warn!("[MidpointTable_H] 列の設定を読めず、退避もできないので、初期値で動き保存しません: {e} / {be}");
                }
            },
        }
        p
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
        if self.save_blocked {
            return;
        }
        let Some(p) = &self.path else { return };
        if let Err(e) = write_replace(p, &Self::to_text(&self.hidden, &self.order, self.auto_reload, &self.unknown)) {
            tracing::warn!("[MidpointTable_H] 列の設定を保存できません: {}: {e}", p.display());
        }
    }
}

/// `{名前}.tmp` に書いてから置き換える（書いている途中で落ちても元のファイルは壊れない）
fn write_replace(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// 読めなかった設定を `{名前}.broken-{秒}` へ移す
fn backup_broken(path: &Path) -> std::io::Result<PathBuf> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".broken-{secs}"));
    let dest = PathBuf::from(name);
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テストごとの空のフォルダ
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("midpoint_prefs_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn broken_files(dir: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.contains(".broken-")))
            .collect()
    }

    #[test]
    fn roundtrip() {
        let mut h = BTreeSet::new();
        h.insert(("標準描画".to_string(), "中心X".to_string()));
        h.insert(("ぼかし".to_string(), "範囲".to_string()));
        let mut o = BTreeMap::new();
        o.insert("標準描画".to_string(), vec!["拡大率".to_string(), "X".to_string()]);
        let t = Prefs::to_text(&h, &o, true, &[]);
        assert_eq!(Prefs::parse(&t), (h.clone(), o.clone()));
        assert!(Prefs::parse_auto(&t));
        // 自動で読み直さない設定は 1 行足すだけで、隠す列・並び順には混ざらない
        let t = Prefs::to_text(&h, &o, false, &[]);
        assert_eq!(Prefs::parse(&t), (h, o));
        assert!(!Prefs::parse_auto(&t));
        assert!(!Prefs::parse_auto("@auto\t0\r\n"));
        assert!(Prefs::parse_auto("@auto\t1\n"));
        // 壊れた行は隠す列・並び順に混ぜない。v0.3.0 の形（隠す列だけ）はそのまま読める
        let (h, o) = Prefs::parse("abc\n\tx\ny\t\n標準描画\t中心X\r\n@order\t\n");
        assert_eq!((h.len(), o.len()), (1, 0));
    }

    #[test]
    fn unknown_lines_are_written_back() {
        // 新しい版が書いた行（欄が 3 つ以上の @ 行・知らない @ 行）は、読み飛ばしたうえで保存のときに残す
        let text = "標準描画\t中心X\n@future\ta\tb\tc\n@order\t標準描画\tY\tX\n@newkey\t1\n@auto\t0\n\n";
        let (h, o) = Prefs::parse(text);
        // 2 欄の @ 行は、前の版と同じく隠す列として読み書きする（そのまま書き戻される）
        assert_eq!((h.len(), o.len()), (2, 1));
        assert!(h.contains(&("@newkey".to_string(), "1".to_string())));
        assert!(!h.contains(&("@future".to_string(), "a".to_string())));
        let unknown = Prefs::parse_unknown(text);
        assert_eq!(unknown, vec!["@future\ta\tb\tc".to_string()]);
        let out = Prefs::to_text(&h, &o, Prefs::parse_auto(text), &unknown);
        assert!(out.contains("@future\ta\tb\tc\n"));
        assert!(out.contains("@newkey\t1\n"));
        assert!(out.contains("@auto\t0\n"));
        // 書いたものを読み直しても同じ
        assert_eq!(Prefs::parse(&out), (h, o));
        assert_eq!(Prefs::parse_unknown(&out), unknown);
    }

    #[test]
    fn load_and_save_keep_unknown_lines() {
        let dir = temp_dir("keep");
        let p = dir.join("hidden_columns.txt");
        std::fs::write(&p, "標準描画\t中心X\r\n@future\ta\tb\tc\r\n").unwrap();
        let mut prefs = Prefs::load(&p);
        assert!(prefs.hidden.contains(&("標準描画".to_string(), "中心X".to_string())));
        prefs.set_hidden("ぼかし", "範囲", true);
        let saved = std::fs::read_to_string(&p).unwrap();
        assert!(saved.contains("@future\ta\tb\tc\n"));
        assert!(saved.contains("ぼかし\t範囲\n"));
        assert!(saved.contains("標準描画\t中心X\n"));
        assert!(!dir.join("hidden_columns.txt.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_is_empty_and_saves() {
        let dir = temp_dir("missing");
        let p = dir.join("sub").join("hidden_columns.txt");
        let mut prefs = Prefs::load(&p);
        assert!(prefs.hidden.is_empty() && prefs.auto_reload && !prefs.save_blocked);
        prefs.set_auto_reload(false);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "@auto\t0\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_utf8_file_is_moved_aside() {
        let dir = temp_dir("broken");
        let p = dir.join("hidden_columns.txt");
        let bytes = b"\x95\x57\x8f\x80\t\x92\x86\x90\x53X\n".to_vec(); // CP932 の「標準\t中心X」
        std::fs::write(&p, &bytes).unwrap();
        let mut prefs = Prefs::load(&p);
        assert!(prefs.hidden.is_empty() && !prefs.save_blocked);
        let moved = broken_files(&dir);
        assert_eq!(moved.len(), 1);
        assert_eq!(std::fs::read(&moved[0]).unwrap(), bytes);
        // 退避できたので、初期値からの保存は新しいファイルに書く
        prefs.set_hidden("ぼかし", "範囲", true);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "ぼかし\t範囲\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 読めず退避もできない（他のプロセスが共有なしで開いている）ときは、保存しない
    #[cfg(windows)]
    #[test]
    fn locked_file_is_not_overwritten() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("locked");
        let p = dir.join("hidden_columns.txt");
        std::fs::write(&p, "標準描画\t中心X\n").unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&p).unwrap();
        let mut prefs = Prefs::load(&p);
        assert!(prefs.save_blocked && prefs.hidden.is_empty());
        prefs.set_hidden("ぼかし", "範囲", true);
        drop(lock);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "標準描画\t中心X\n");
        assert!(broken_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
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
