//! 本体に登録されている移動方法の一覧を作る。
//!
//! **`aviutl2.ini` の `[Movement.*]` は使わない。** 消したスクリプトの名前も残り続ける（本番で 246 件中、ファイルの無いものが
//! 約 120 件。devenv でも消したプローブの名前が残っていた。2026-10-08）。本体に登録されていない名前を書くと
//! 本体が例外 `not found movement` を出し、書き込みの処理ごと打ち切られる（`AI/docs/20261008_パラメータ編集_SDK実機確認.md`）。
//!
//! そこで、本体が読む場所の `.tra2` から名前を作る（本番で、ファイルから作った 121 件がすべて ini にあることを確かめた）:
//! - `Script/` と、その一つ下のフォルダの `.tra2`
//!   - `@名前.tra2`（マルチセクション）: 行頭の `@セクション` ごとに `セクション@名前`
//!   - それ以外: ファイル名から `.tra2` を除いたもの
//! - 本体のフォルダの `script.tra2`（同梱。`@反復移動` `@回転`）: セクション名そのもの
//! - 本体に組み込みのもの（`BUILTIN`。devenv の素の本体 2.1.11a の ini から、スクリプト由来の 2 つを除いたもの）
//!
//! 本体はスクリプトを起動時に読むので、起動後に置かれた `.tra2` は登録されていない。プラグインが読み込まれた時刻より
//! 新しいファイルは外す。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 本体に組み込みの移動方法（スクリプトのファイルを持たない）。
/// 「移動無し」も本体に登録されているが、書くと移動なし（値 1 つ）になる（中間点プローブ_H v0.2.2 の M3）ので一覧に入れない。
/// 表では「移動なし」として別に選ぶ
pub const BUILTIN: &[&str] = &[
    "直線移動",
    "直線移動(時間制御)",
    "直線移動(回転)",
    "補間移動",
    "補間移動(時間制御)",
    "補間移動(回転)",
    "瞬間移動",
    "移動量指定",
    "ランダム移動",
    "再生範囲",
];

/// 値が始点と終点の 2 つだけで保存される移動方法（設定の中間点無視とは別に）。
/// 再生範囲は点が 4 つでも 2 つに切り詰められた（M3）
pub const TWO_VALUE: &[&str] = &["再生範囲"];

#[derive(Debug, Clone, PartialEq)]
pub enum ParamKind {
    Number,
    Check,
    Select(Vec<(String, f64)>),
}

/// `--param:` 1 行分（`lua.txt` の「トラックバー移動スクリプト」）
#[derive(Debug, Clone, PartialEq)]
pub struct ParamDef {
    pub label: String,
    pub kind: ParamKind,
    pub default: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Movement {
    pub name: String,
    /// 設定値の定義。組み込みのものは分からないので空
    pub params: Vec<ParamDef>,
    pub speed: bool,
    pub twopoint: bool,
    pub timecontrol: bool,
    pub builtin: bool,
    /// どのファイルから作ったか（組み込みは `None`）
    pub source: Option<PathBuf>,
}

impl Movement {
    fn builtin(name: &str) -> Self {
        // 組み込みで設定値を持つのはランダム移動だけ（`0` を書いたときの設定値。プローブ v0.2.2 の M3。名前は取れない）
        let params = if name == "ランダム移動" {
            vec![ParamDef { label: "設定値1".into(), kind: ParamKind::Number, default: 15.0 }]
        } else {
            Vec::new()
        };
        Movement {
            name: name.to_string(),
            params,
            speed: false,
            twopoint: false,
            timecontrol: false,
            builtin: true,
            source: None,
        }
    }
}

/// `--param:` の後ろを読む。`index` は名前が無いときの番号（1 始まり）
pub fn parse_param(spec: &str, index: usize) -> ParamDef {
    let (head, default) = match spec.rsplit_once(',') {
        Some((h, d)) => (Some(h), d),
        None => (None, spec),
    };
    let default = default.trim().parse::<f64>().unwrap_or(0.0);
    let Some(head) = head else {
        return ParamDef { label: format!("設定値{index}"), kind: ParamKind::Number, default };
    };
    let mut parts = head.split('/');
    let label = parts.next().unwrap_or("").trim().to_string();
    let label = if label.is_empty() { format!("設定値{index}") } else { label };
    let kind = match parts.next() {
        Some("check") => ParamKind::Check,
        Some("select") => ParamKind::Select(
            parts
                .filter_map(|p| p.split_once('=').map(|(k, v)| (k.to_string(), v.trim().parse::<f64>().unwrap_or(0.0))))
                .collect(),
        ),
        _ => ParamKind::Number,
    };
    ParamDef { label, kind, default }
}

/// ヘッダーの指定を読む（`--param` `--speed` `--twopoint` `--timecontrol`）
fn read_directives(lines: &[&str]) -> (Vec<ParamDef>, bool, bool, bool) {
    let (mut params, mut speed, mut twopoint, mut timecontrol) = (Vec::new(), false, false, false);
    for line in lines {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("--param:") {
            params.push(parse_param(rest, params.len() + 1));
        } else if line.starts_with("--speed") {
            speed = true;
        } else if line.starts_with("--twopoint") {
            twopoint = true;
        } else if line.starts_with("--timecontrol") {
            timecontrol = true;
        }
    }
    (params, speed, twopoint, timecontrol)
}

/// マルチセクションの本文をセクションに分ける（`@名前` の行から次の `@` 行まで）
fn split_sections(text: &str) -> Vec<(String, Vec<&str>)> {
    let mut out: Vec<(String, Vec<&str>)> = Vec::new();
    for line in text.lines() {
        let l = line.trim_end_matches('\r');
        if let Some(name) = l.strip_prefix('@') {
            out.push((name.to_string(), Vec::new()));
        } else if let Some(last) = out.last_mut() {
            last.1.push(l);
        }
    }
    out
}

/// 1 ファイルから移動方法を作る。`bundled` は本体同梱の `script.tra2`（名前に `@ファイル名` を付けない）
pub fn movements_in_file(path: &Path, text: &str, bundled: bool) -> Vec<Movement> {
    let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
    let Some(stem) = file.strip_suffix(".tra2") else { return Vec::new() };
    let make = |name: String, lines: &[&str]| {
        let (params, speed, twopoint, timecontrol) = read_directives(lines);
        Movement { name, params, speed, twopoint, timecontrol, builtin: bundled, source: Some(path.to_path_buf()) }
    };
    if bundled {
        return split_sections(text).into_iter().map(|(sec, lines)| make(sec, &lines)).collect();
    }
    if let Some(base) = stem.strip_prefix('@') {
        split_sections(text).into_iter().map(|(sec, lines)| make(format!("{sec}@{base}"), &lines)).collect()
    } else {
        let lines: Vec<&str> = text.lines().collect();
        vec![make(stem.to_string(), &lines)]
    }
}

/// 一覧の結果
#[derive(Debug, Default)]
pub struct Scan {
    pub movements: Vec<Movement>,
    /// 外したもの（名前と理由）
    pub skipped: Vec<(String, String)>,
}

impl Scan {
    pub fn find(&self, name: &str) -> Option<&Movement> {
        self.movements.iter().find(|m| m.name == name)
    }
}

fn tra2_files(script_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(script_dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&p) {
                out.extend(sub.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "tra2")));
            }
        } else if p.extension().is_some_and(|x| x == "tra2") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// 一覧を作る。`bundled` は本体同梱の `script.tra2`、`loaded_at` より新しいファイルは外す
pub fn scan(script_dir: &Path, bundled: Option<&Path>, loaded_at: Option<SystemTime>) -> Scan {
    let mut s = Scan { movements: BUILTIN.iter().map(|n| Movement::builtin(n)).collect(), skipped: Vec::new() };
    let push = |s: &mut Scan, m: Movement| {
        if s.movements.iter().any(|x| x.name == m.name) {
            s.skipped.push((m.name, "同じ名前が先にある".into()));
        } else {
            s.movements.push(m);
        }
    };
    if let Some(b) = bundled {
        if let Ok(bytes) = std::fs::read(b) {
            for m in movements_in_file(b, &String::from_utf8_lossy(&bytes), true) {
                push(&mut s, m);
            }
        }
    }
    for f in tra2_files(script_dir) {
        let Ok(bytes) = std::fs::read(&f) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let newer = loaded_at.is_some_and(|t| std::fs::metadata(&f).and_then(|m| m.modified()).is_ok_and(|m| m > t));
        for m in movements_in_file(&f, &text, false) {
            if newer {
                s.skipped.push((m.name, "本体の起動後に置かれたファイル（再起動するまで登録されない）".into()));
            } else {
                push(&mut s, m);
            }
        }
    }
    s
}

/// 移動方法を選び直したときに書く設定。**常に `0`**（加速・減速・中間点無視なし、設定値は本体が初期値で埋める）。
///
/// 中間点プローブ_H v0.2.2（devenv 2.1.11a）で確かめたこと:
/// - `0` だけを書くと、`get_object_track_info` の設定値は定義の初期値になる（ランダム移動 15、反復移動 15、回転 360、
///   プローブ移動_H の 3 つ 5 / 1 / 2）
/// - `|` の後ろに設定値を `|` で並べても、保存されるのは 1 つ目だけ（`0|7|0|1` は `0|7`）
/// - 本体の設定ダイアログで変えると `0|9,0,1` で保存された（v0.2.3）。**設定値どうしはカンマ区切り**
/// - 数が定義と合えば効く。合わないと文字列はそのまま保存されるが、設定値は初期値になる（失敗も例外も無い。v0.2.4 の M7）。
///   設定値の編集（第 2 段階）を入れるまでは書かない
pub fn default_tail(_m: &Movement) -> String {
    String::from("0")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_forms() {
        assert_eq!(parse_param("15", 1), ParamDef { label: "設定値1".into(), kind: ParamKind::Number, default: 15.0 });
        assert_eq!(parse_param("速さ,5", 1), ParamDef { label: "速さ".into(), kind: ParamKind::Number, default: 5.0 });
        assert_eq!(parse_param("加速/check,0", 2).kind, ParamKind::Check);
        let p = parse_param("種別/select/直線=1/曲線=2,1", 3);
        assert_eq!(p.kind, ParamKind::Select(vec![("直線".into(), 1.0), ("曲線".into(), 2.0)]));
        assert_eq!(p.default, 1.0);
    }

    #[test]
    fn names_from_files() {
        let multi = movements_in_file(
            Path::new("Script/@Basic_S.tra2"),
            "@4次式\n--speed:0,0\n--param:強さ,2\nlocal a\n@円形\n--twopoint\n",
            false,
        );
        assert_eq!(multi.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["4次式@Basic_S", "円形@Basic_S"]);
        assert!(multi[0].speed && multi[0].params.len() == 1 && multi[1].twopoint);
        // ファイル名が .tra2.tra2 のもの（本番の time_control_assistant）
        let m = movements_in_file(Path::new("x/@time_control_assistant.tra2.tra2"), "@往復\n", false);
        assert_eq!(m[0].name, "往復@time_control_assistant.tra2");
        let single = movements_in_file(Path::new("Script/バウンス.tra2"), "--param:高さ,100\n", false);
        assert_eq!(single[0].name, "バウンス");
        let bundled = movements_in_file(Path::new("script.tra2"), "@反復移動\n--twopoint\n--param:15\n@回転\n--param:360\n", true);
        assert_eq!(bundled.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), vec!["反復移動", "回転"]);
        assert_eq!(bundled[0].params[0].default, 15.0);
        assert_eq!(default_tail(&bundled[0]), "0");
        assert_eq!(default_tail(&Movement::builtin("直線移動")), "0");
        assert!(!BUILTIN.contains(&"移動無し"));
    }

    /// 本番のフォルダで、作った名前がすべて aviutl2.ini にあるか（ini に無い名前は本体が知らない名前）。
    /// `cargo test -- --ignored scan_production`
    #[test]
    #[ignore]
    fn scan_production() {
        let root = Path::new("C:/ProgramData/aviutl2");
        let s = scan(&root.join("Script"), Some(&root.join("AviUtlExEdit2/script.tra2")), None);
        let ini = String::from_utf8_lossy(&std::fs::read(root.join("aviutl2.ini")).unwrap()).to_string();
        let missing: Vec<&str> =
            s.movements.iter().map(|m| m.name.as_str()).filter(|n| !ini.contains(&format!("[Movement.{n}]"))).collect();
        println!("{} 件 / ini に無いもの: {:?} / 外したもの: {:?}", s.movements.len(), missing, s.skipped);
        assert!(missing.is_empty());
        assert!(s.movements.len() > BUILTIN.len() + 100);
    }

    #[test]
    fn scan_dir() {
        let dir = std::env::temp_dir().join(format!("midpoint_table_scan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        std::fs::write(dir.join("@Multi.tra2"), "@A\n@B\n").unwrap();
        std::fs::write(dir.join("Sub").join("単体.tra2"), "--param:1\n").unwrap();
        std::fs::write(dir.join("無関係.anm2"), "@X\n").unwrap();
        let s = scan(&dir, None, None);
        let names: Vec<&str> = s.movements.iter().skip(BUILTIN.len()).map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["A@Multi", "B@Multi", "単体"]);
        assert!(s.find("直線移動").is_some_and(|m| m.builtin));
        // 読み込んだ時刻より新しいファイルは外す
        let past = SystemTime::now() - std::time::Duration::from_secs(3600);
        let s = scan(&dir, None, Some(past));
        assert_eq!(s.movements.len(), BUILTIN.len());
        assert_eq!(s.skipped.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
